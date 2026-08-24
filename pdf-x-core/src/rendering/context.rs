//! Rendering context for processing PDF content streams.
//!
//! This module provides the RenderingContext, which coordinates:
//! - Graphics state stack (save/restore)
//! - Current path being constructed
//! - Device for rendering operations
//! - Processing of content stream operators

use super::device::Device;
use super::graphics_state::{Color, FillRule, GraphicsState};
use super::path::{Path, PathElement};
use super::{Paint, PathDrawMode};
use crate::core::cmap::CMap;
use crate::core::content_stream::{ContentStreamEvaluator, OpCode, Operation};
use crate::core::encoding::Encoding;
use crate::core::error::{PDFError, PDFResult};
use crate::core::page::PageResolver;
use crate::core::parser::{PDFObject, Parser};
use crate::core::{BaseStream, Lexer, Stream};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_PATTERN_TILES: usize = 4096;
const MAX_PATTERN_DEPTH: usize = 8;

#[derive(Debug, Clone)]
struct TilingPatternPaint {
    bbox: [f64; 4],
    x_step: f64,
    y_step: f64,
    matrix: [f64; 6],
    data: Vec<u8>,
    resources: Option<PDFObject>,
    paint_type: i32,
    stencil_color: Option<Color>,
}

#[derive(Debug, Clone)]
struct Type3Glyph {
    data: Vec<u8>,
    is_shape_glyph: bool,
    resources: Option<PDFObject>,
}

#[derive(Debug, Clone)]
struct Type3Font {
    encoding: Type3Encoding,
    char_procs: HashMap<String, Type3Glyph>,
    resources: Option<PDFObject>,
    font_matrix: [f64; 6],
    first_char: u32,
    widths: Vec<f64>,
    default_width: f64,
}

#[derive(Debug, Clone)]
enum Type3Encoding {
    Names(Vec<Option<String>>),
    Text(Encoding),
}

impl Type3Encoding {
    fn glyph_name(&self, code: u8) -> Option<String> {
        match self {
            Type3Encoding::Names(names) => names.get(code as usize).and_then(Clone::clone),
            Type3Encoding::Text(encoding) => Some(encoding.char_to_unicode(code).to_string()),
        }
    }
}

/// Runtime toggle for verbose text spacing traces (Tw/Tc/TJ/Tj).
///
/// This is intentionally global so frontends (like Tauri) can enable tracing
/// around a render call without rebuilding with special features.
static TEXT_TRACE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable or disable runtime text tracing.
///
/// Returns the previous value so callers can restore it after a scoped render.
pub fn set_text_trace_enabled(enabled: bool) -> bool {
    TEXT_TRACE_ENABLED.swap(enabled, Ordering::Relaxed)
}

/// Returns whether runtime text tracing is currently enabled.
pub fn is_text_trace_enabled() -> bool {
    TEXT_TRACE_ENABLED.load(Ordering::Relaxed)
}

/// Rendering context for processing PDF content streams.
///
/// The context maintains the graphics state stack, current path, and device
/// for rendering. It processes content stream operations and forwards them
/// to the device.
///
/// This follows the same pattern as PDF.js's CanvasGraphics and hayro's Context.
pub struct RenderingContext<'a, D: Device> {
    /// The device for rendering
    device: &'a mut D,

    /// Graphics state stack
    state_stack: Vec<GraphicsState>,

    /// Root transform stack used to anchor patterns at page/form boundaries.
    root_ctm_stack: Vec<[f64; 6]>,

    /// Current path being constructed
    current_path: Path,

    /// Pending clipping rule set by W/W*. PDF applies it when the current path
    /// is ended or painted, not when the clipping operator is encountered.
    pending_clip: Option<FillRule>,

    /// Clip path stack (for nesting)
    clip_stack: Vec<FillRule>,

    /// Whether we're in a text object (BT...ET)
    in_text_object: bool,

    /// Object resolver for fetching resources (delta-aware when a `DocStore` is supplied).
    resolver: Option<&'a mut dyn PageResolver>,

    /// Active resources dictionary (for looking up XObjects, fonts, etc.).
    ///
    /// Stored as an owned object so Form XObjects can temporarily override it
    /// with their own `/Resources` dictionaries.
    resources: Option<PDFObject>,

    /// Operation counter for debug logging
    operation_count: usize,

    /// Optional /ToUnicode CMaps keyed by active PDF font resource name.
    font_to_unicode: HashMap<String, CMap>,

    /// Whether a font should be remapped through `/ToUnicode` before drawing.
    ///
    /// We only do this for composite/CID-style fonts where raw shown bytes are
    /// not directly drawable by the simple Type1/TrueType code paths.
    font_requires_cmap_remap: HashMap<String, bool>,

    /// Type3 font programs keyed by active PDF font resource name.
    type3_fonts: HashMap<String, Type3Font>,

    /// Paint override used while interpreting uncolored Type3 shape glyphs.
    type3_shape_color: Option<Color>,

    /// Current nonstroking color space selected by `cs`.
    fill_color_space: Option<PDFObject>,

    /// Current stroking color space selected by `CS`.
    stroke_color_space: Option<PDFObject>,

    /// Color-space graphics state stack parallel to `state_stack`.
    color_space_stack: Vec<(Option<PDFObject>, Option<PDFObject>)>,

    /// Pending fill shading (set by `scn` with a Pattern color space).
    /// Sticky until the fill colour is replaced; also saved/restored by `q`/`Q`
    /// because pattern colour is part of the graphics state (PDF §8.4.1).
    pending_fill_shading: Option<super::shading::Shading>,

    /// Pending fill tiling pattern (set by `scn` with a Pattern color space).
    /// Same stickiness as [`Self::pending_fill_shading`].
    pending_fill_tiling: Option<TilingPatternPaint>,

    /// Pending stroke shading (set by `SCN` with a Pattern color space).
    pending_stroke_shading: Option<super::shading::Shading>,

    /// Pattern-paint stack parallel to `state_stack` (`q`/`Q`).
    pattern_paint_stack: Vec<(
        Option<super::shading::Shading>,
        Option<TilingPatternPaint>,
        Option<super::shading::Shading>,
    )>,

    /// Soft mask group reference stack (prevents only true self-recursion).
    soft_mask_group_stack: Vec<(u32, u32)>,

    /// Soft mask rendering depth; Form transparency groups inside mask rendering
    /// are rendered as mask content, not composited as page groups.
    soft_mask_render_depth: usize,

    /// Pattern stream recursion depth guard.
    pattern_depth: usize,

    /// Replacement paint for uncolored tiling pattern streams.
    stencil_pattern_color: Option<Color>,
}

impl<'a, D: Device> RenderingContext<'a, D> {
    /// Create a new rendering context.
    ///
    /// # Arguments
    /// * `device` - The rendering device to use
    pub fn new(device: &'a mut D) -> Self {
        RenderingContext {
            device,
            state_stack: vec![GraphicsState::default()],
            root_ctm_stack: vec![GraphicsState::default().ctm],
            current_path: Path::new(),
            pending_clip: None,
            clip_stack: Vec::new(),
            in_text_object: false,
            resolver: None,
            resources: None,
            operation_count: 0,
            font_to_unicode: HashMap::new(),
            font_requires_cmap_remap: HashMap::new(),
            type3_fonts: HashMap::new(),
            type3_shape_color: None,
            fill_color_space: None,
            stroke_color_space: None,
            color_space_stack: Vec::new(),
            pending_fill_shading: None,
            pending_fill_tiling: None,
            pending_stroke_shading: None,
            pattern_paint_stack: Vec::new(),
            soft_mask_group_stack: Vec::new(),
            soft_mask_render_depth: 0,
            pattern_depth: 0,
            stencil_pattern_color: None,
        }
    }

    /// Set the object resolver and page resources for XObject/font/ext-gstate rendering.
    ///
    /// The resolver is delta-aware when a `DocStore` is supplied, so resource fetches honor
    /// delta overrides. Pass `&mut XRef` for the legacy base-only path.
    ///
    /// # Arguments
    /// * `resolver` - The object resolver for fetching resources
    /// * `resources` - The page's resources dictionary
    pub fn set_xobject_resources(
        &mut self,
        resolver: &'a mut dyn PageResolver,
        resources: &'a PDFObject,
    ) {
        self.resolver = Some(resolver);
        self.resources = Some(resources.clone());
    }

    /// Get the current graphics state.
    pub fn current_state(&self) -> &GraphicsState {
        self.state_stack
            .last()
            .expect("Graphics state stack underflow")
    }

    /// Get mutable reference to the current graphics state.
    pub fn current_state_mut(&mut self) -> &mut GraphicsState {
        self.state_stack
            .last_mut()
            .expect("Graphics state stack underflow")
    }

    /// Set the root CTM used to anchor patterns independently of content-stream CTM changes.
    pub fn set_root_ctm(&mut self, matrix: [f64; 6]) {
        if let Some(root) = self.root_ctm_stack.last_mut() {
            *root = matrix;
        } else {
            self.root_ctm_stack.push(matrix);
        }
    }

    fn root_ctm(&self) -> [f64; 6] {
        self.root_ctm_stack
            .last()
            .copied()
            .unwrap_or_else(|| GraphicsState::default().ctm)
    }

    fn push_root_ctm(&mut self) {
        self.root_ctm_stack.push(self.current_state().ctm);
    }

    fn pop_root_ctm(&mut self) {
        if self.root_ctm_stack.len() > 1 {
            self.root_ctm_stack.pop();
        }
    }

    /// Get the device.
    pub fn device(&mut self) -> &mut D {
        &mut *self.device
    }

    /// Process a content stream operation.
    ///
    /// This is the main entry point for interpreting PDF content streams.
    /// It dispatches to appropriate handler methods based on the operator.
    pub fn process_operation(&mut self, op: &Operation) -> PDFResult<()> {
        // Debug: Log first 20 operations to understand what's happening
        #[cfg(feature = "debug-logging")]
        {
            if self.operation_count < 20 {
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Operation #{}: {:?} with {} args",
                    self.operation_count,
                    op.op,
                    op.args.len()
                );
                self.operation_count += 1;
            }
        }

        if is_text_trace_enabled() {
            match op.op {
                OpCode::SetCharSpacing
                | OpCode::SetWordSpacing
                | OpCode::ShowText
                | OpCode::ShowSpacedText
                | OpCode::NextLineShowText
                | OpCode::NextLineSetSpacingShowText => {
                    eprintln!("[text-trace] op={:?} args={:?}", op.op, op.args);
                }
                _ => {}
            }
        }

        match op.op {
            // Graphics state operators
            OpCode::Save => self.save()?,
            OpCode::Restore => self.restore()?,
            OpCode::Transform => self.transform(&op.args)?,

            // Path construction operators
            OpCode::MoveTo => self.move_to(&op.args)?,
            OpCode::LineTo => self.line_to(&op.args)?,
            OpCode::CurveTo => self.curve_to(&op.args)?,
            OpCode::CurveTo2 => self.curve_to_2(&op.args)?,
            OpCode::CurveTo3 => self.curve_to_3(&op.args)?,
            OpCode::ClosePath => self.close_path()?,
            OpCode::Rectangle => self.rectangle(&op.args)?,

            // Path painting operators
            OpCode::Stroke => self.stroke()?,
            OpCode::CloseStroke => self.close_and_stroke()?,
            OpCode::Fill => self.fill(FillRule::NonZero)?,
            OpCode::EOFill => self.fill(FillRule::EvenOdd)?,
            OpCode::FillStroke => self.fill_and_stroke(FillRule::NonZero)?,
            OpCode::EOFillStroke => self.fill_and_stroke(FillRule::EvenOdd)?,
            OpCode::CloseFillStroke => self.close_fill_stroke(FillRule::NonZero)?,
            OpCode::CloseEOFillStroke => self.close_fill_stroke(FillRule::EvenOdd)?,
            OpCode::EndPath => self.end_path()?,

            // Clipping operators
            OpCode::Clip => self.clip(FillRule::NonZero)?,
            OpCode::EOClip => self.clip(FillRule::EvenOdd)?,

            // Text object operators
            OpCode::BeginText => self.begin_text()?,
            OpCode::EndText => self.end_text()?,

            // Text showing operators
            OpCode::ShowText => self.show_text(&op.args)?,
            OpCode::ShowSpacedText => self.show_spaced_text(&op.args)?,
            OpCode::NextLineShowText => self.next_line_show_text(&op.args)?,
            OpCode::NextLineSetSpacingShowText => self.next_line_set_spacing_show_text(&op.args)?,

            // Text positioning operators
            OpCode::MoveText => self.move_text(&op.args)?,
            OpCode::SetLeadingMoveText => self.set_leading_move_text(&op.args)?,
            OpCode::SetTextMatrix => self.set_text_matrix(&op.args)?,
            OpCode::NextLine => self.next_line()?,

            // Text state operators
            OpCode::SetFont => self.set_font(&op.args)?,
            OpCode::SetCharSpacing => self.set_char_spacing(&op.args)?,
            OpCode::SetWordSpacing => self.set_word_spacing(&op.args)?,
            OpCode::SetHScale => self.set_horizontal_scaling(&op.args)?,
            OpCode::SetLeading => self.set_leading(&op.args)?,
            OpCode::SetTextRenderingMode => self.set_text_rendering_mode(&op.args)?,
            OpCode::SetTextRise => self.set_text_rise(&op.args)?,

            // Color operators
            OpCode::SetStrokeGray => self.set_stroke_gray(&op.args)?,
            OpCode::SetFillGray => self.set_fill_gray(&op.args)?,
            OpCode::SetStrokeRGBColor => self.set_stroke_rgb(&op.args)?,
            OpCode::SetFillRGBColor => self.set_fill_rgb(&op.args)?,
            OpCode::SetStrokeCMYKColor => self.set_stroke_cmyk(&op.args)?,
            OpCode::SetFillCMYKColor => self.set_fill_cmyk(&op.args)?,
            OpCode::SetStrokeColorSpace => self.set_stroke_color_space(&op.args)?,
            OpCode::SetFillColorSpace => self.set_fill_color_space(&op.args)?,
            OpCode::SetStrokeColor | OpCode::SetStrokeColorN => self.set_stroke_color(&op.args)?,
            OpCode::SetFillColor | OpCode::SetFillColorN => self.set_fill_color(&op.args)?,

            // Line property operators
            OpCode::SetLineWidth => self.set_line_width(&op.args)?,
            OpCode::SetLineCap => self.set_line_cap(&op.args)?,
            OpCode::SetLineJoin => self.set_line_join(&op.args)?,
            OpCode::SetMiterLimit => self.set_miter_limit(&op.args)?,
            OpCode::SetDash => self.set_dash(&op.args)?,
            OpCode::SetGState => self.set_graphics_state(&op.args)?,

            // XObject operator
            OpCode::PaintXObject => self.paint_xobject(&op.args)?,
            OpCode::BeginInlineImage => self.paint_inline_image(&op.args)?,

            // Shading operator
            OpCode::ShadingFill => self.handle_shading_fill(&op.args)?,

            _ => {
                // Other operators not yet implemented
                // Silently skip to avoid spamming logs
                // These are typically non-critical operators like marked-content tags.
            }
        }

        Ok(())
    }

    // === Graphics State Operators ===

    fn save(&mut self) -> PDFResult<()> {
        // Save current state
        let saved = self.current_state().save();
        self.state_stack.push(saved);
        self.color_space_stack.push((
            self.fill_color_space.clone(),
            self.stroke_color_space.clone(),
        ));
        // Pattern colour is part of the graphics state (PDF §8.4.1 / §8.7.3).
        self.pattern_paint_stack.push((
            self.pending_fill_shading.clone(),
            self.pending_fill_tiling.clone(),
            self.pending_stroke_shading.clone(),
        ));
        self.device.save_state();
        Ok(())
    }

    fn restore(&mut self) -> PDFResult<()> {
        // Restore any clipping paths
        while self.clip_stack.len() >= self.state_stack.len() {
            self.clip_stack.pop();
            // Note: Device doesn't have a pop_clip method in our trait
            // In a full implementation, we'd pop the clip here
        }

        // Restore graphics state
        if self.state_stack.len() > 1 {
            self.state_stack.pop();
            if let Some((fill, stroke)) = self.color_space_stack.pop() {
                self.fill_color_space = fill;
                self.stroke_color_space = stroke;
            }
            if let Some((fill_sh, fill_tile, stroke_sh)) = self.pattern_paint_stack.pop() {
                self.pending_fill_shading = fill_sh;
                self.pending_fill_tiling = fill_tile;
                self.pending_stroke_shading = stroke_sh;
            }
            self.device.restore_state();
        }
        Ok(())
    }

    fn transform(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 6 {
            return Err(PDFError::content_stream_error(
                "cm operator requires 6 arguments".to_string(),
            ));
        }

        let mut matrix = [0.0; 6];
        for i in 0..6 {
            if let crate::core::parser::PDFObject::Number(n) = args[i] {
                matrix[i] = n;
            } else {
                return Err(PDFError::content_stream_error(
                    "cm operator arguments must be numbers".to_string(),
                ));
            }
        }

        self.current_state_mut().concat_matrix(&matrix);
        self.device.concat_matrix(&matrix);
        Ok(())
    }

    // === Path Construction Operators ===

    fn move_to(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "m operator requires 2 arguments".to_string(),
            ));
        }

        let x = extract_number(args, 0)?;
        let y = extract_number(args, 1)?;

        self.current_path.move_to(x, y);
        self.device.move_to(x, y);
        Ok(())
    }

    fn line_to(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "l operator requires 2 arguments".to_string(),
            ));
        }

        let x = extract_number(args, 0)?;
        let y = extract_number(args, 1)?;

        self.current_path.line_to(x, y);
        self.device.line_to(x, y);
        Ok(())
    }

    fn curve_to(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 6 {
            return Err(PDFError::content_stream_error(
                "c operator requires 6 arguments".to_string(),
            ));
        }

        let cp1x = extract_number(args, 0)?;
        let cp1y = extract_number(args, 1)?;
        let cp2x = extract_number(args, 2)?;
        let cp2y = extract_number(args, 3)?;
        let x = extract_number(args, 4)?;
        let y = extract_number(args, 5)?;

        self.current_path.curve_to(cp1x, cp1y, cp2x, cp2y, x, y);
        self.device.curve_to(cp1x, cp1y, cp2x, cp2y, x, y);
        Ok(())
    }

    fn curve_to_2(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        // v - CurveTo2: initial point replicated
        if args.len() < 4 {
            return Err(PDFError::content_stream_error(
                "v operator requires 4 arguments".to_string(),
            ));
        }

        let current = self.current_path.current_point().unwrap_or((0.0, 0.0));
        let cp2x = extract_number(args, 0)?;
        let cp2y = extract_number(args, 1)?;
        let x = extract_number(args, 2)?;
        let y = extract_number(args, 3)?;

        self.current_path
            .curve_to(current.0, current.1, cp2x, cp2y, x, y);
        self.device.curve_to(current.0, current.1, cp2x, cp2y, x, y);
        Ok(())
    }

    fn curve_to_3(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        // y - CurveTo3: final point replicated
        if args.len() < 4 {
            return Err(PDFError::content_stream_error(
                "y operator requires 4 arguments".to_string(),
            ));
        }

        let cp1x = extract_number(args, 0)?;
        let cp1y = extract_number(args, 1)?;
        let x = extract_number(args, 2)?;
        let y = extract_number(args, 3)?;

        self.current_path.curve_to(cp1x, cp1y, x, y, x, y);
        self.device.curve_to(cp1x, cp1y, x, y, x, y);
        Ok(())
    }

    fn close_path(&mut self) -> PDFResult<()> {
        self.current_path.close_path();
        self.device.close_path();
        Ok(())
    }

    fn rectangle(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 4 {
            return Err(PDFError::content_stream_error(
                "re operator requires 4 arguments".to_string(),
            ));
        }

        let x = extract_number(args, 0)?;
        let y = extract_number(args, 1)?;
        let width = extract_number(args, 2)?;
        let height = extract_number(args, 3)?;

        self.current_path.rect(x, y, width, height);
        self.device.rect(x, y, width, height);
        Ok(())
    }

    // === Path Painting Operators ===

    fn stroke(&mut self) -> PDFResult<()> {
        self.apply_pending_clip()?;

        // Pattern Type 2 stroke: rasterize the stroke into an alpha mask, then
        // paint the shading through that mask (PDF 1.7 §8.7.3). Used by
        // text_stroked_complex_paint.pdf (`/Pattern CS /p0 SCN` + `S`).
        if let Some(shading) = self.pending_stroke_shading.clone() {
            let stroke_props = self.current_state().stroke_props.clone();
            let stroke_alpha = self.current_state().stroke_alpha;
            let fill_alpha = self.current_state().fill_alpha;
            let saved_path = self.current_path.clone();

            // White stroke on transparent offscreen → alpha coverage of the stroke.
            if self.device.begin_offscreen().is_ok() {
                self.device.set_opacity(1.0);
                self.replay_path_to_device(&saved_path);
                let white = Paint::from_color(Color::RGB(1.0, 1.0, 1.0));
                if self
                    .device
                    .draw_path(PathDrawMode::Stroke, &white, &stroke_props)
                    .is_ok()
                {
                    if let Some((mask, mw, mh)) = self.device.end_offscreen_as_mask(false, None) {
                        let _ = self.device.set_soft_mask(Some((&mask, mw, mh)));
                        let base_ctm = if shading.use_root_ctm {
                            self.root_ctm()
                        } else {
                            self.current_state().ctm
                        };
                        let combined = post_concat_matrix(base_ctm, shading.matrix);
                        self.device.set_opacity(stroke_alpha);
                        let _ = self.device.draw_shading(&shading, &combined);
                        let _ = self.device.set_soft_mask(None);
                        self.device.set_opacity(fill_alpha);
                        self.current_path.begin();
                        self.device.begin_path();
                        return Ok(());
                    }
                } else {
                    // draw_path failed: still restore offscreen pixels.
                    let _ = self.device.end_offscreen_as_mask(false, None);
                }
            }

            // Fallback: solid sample colour if offscreen path is unavailable.
            self.replay_path_to_device(&saved_path);
            self.current_path = saved_path;
            let (r, g, b, _) = shading
                .sample(0.0, 0.0)
                .or_else(|| shading.sample(0.5, 0.5))
                .unwrap_or((0, 0, 0, 255));
            let paint = Paint::from_color(Color::RGB(
                r as f64 / 255.0,
                g as f64 / 255.0,
                b as f64 / 255.0,
            ));
            self.device.set_opacity(stroke_alpha);
            self.device
                .draw_path(PathDrawMode::Stroke, &paint, &stroke_props)?;
            self.device.set_opacity(fill_alpha);
            self.current_path.begin();
            return Ok(());
        }

        let state = self.current_state();
        let paint = Paint::from_color(self.effective_stroke_color());
        let stroke_props = state.stroke_props.clone();
        let stroke_alpha = state.stroke_alpha;
        let fill_alpha = state.fill_alpha;
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Stroke path with color {:?}", state.stroke_color);
        self.device.set_opacity(stroke_alpha);
        self.device
            .draw_path(PathDrawMode::Stroke, &paint, &stroke_props)?;
        self.device.set_opacity(fill_alpha);
        self.current_path.begin();
        Ok(())
    }

    fn close_and_stroke(&mut self) -> PDFResult<()> {
        self.close_path()?;
        self.stroke()
    }

    fn fill(&mut self, rule: FillRule) -> PDFResult<()> {
        // Pattern colours are sticky (PDF §8.7.3): keep pending across paints
        // until the fill colour is replaced. Source: text_stroked_complex_paint
        // applies one `scn` then many `f` for successive glyph outlines.
        if let Some(shading) = self.pending_fill_shading.clone() {
            self.apply_pending_clip()?;
            self.device.save_state();
            self.device.clip_path(rule)?;
            let base_ctm = if shading.use_root_ctm {
                self.root_ctm()
            } else {
                self.current_state().ctm
            };
            let combined = post_concat_matrix(base_ctm, shading.matrix);
            self.device.draw_shading(&shading, &combined)?;
            self.device.restore_state();
            self.current_path.begin();
            return Ok(());
        }
        if let Some(pattern) = self.pending_fill_tiling.clone() {
            self.apply_pending_clip()?;
            // Temporarily clear sticky tiling so nested fills inside the
            // pattern cell stream do not re-enter the same pattern.
            self.pending_fill_tiling = None;
            let result = self.fill_with_tiling_pattern(rule, pattern.clone());
            self.pending_fill_tiling = Some(pattern);
            return result;
        }
        self.apply_pending_clip()?;
        let state = self.current_state();
        let paint = Paint::from_color(self.effective_fill_color());
        let stroke_props = state.stroke_props.clone();
        let fill_alpha = state.fill_alpha;
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Fill path with color {:?}", state.fill_color);
        self.device.set_opacity(fill_alpha);
        self.device
            .draw_path(PathDrawMode::Fill(rule), &paint, &stroke_props)?;
        self.current_path.begin();
        Ok(())
    }

    fn fill_and_stroke(&mut self, rule: FillRule) -> PDFResult<()> {
        if self.pending_fill_tiling.is_some() || self.pending_fill_shading.is_some() {
            let saved_path = self.current_path.clone();
            self.fill(rule)?;
            self.replay_path_to_device(&saved_path);
            self.current_path = saved_path;
            return self.stroke();
        }

        self.apply_pending_clip()?;
        let state = self.current_state();
        let fill_color = self.effective_fill_color();
        let stroke_color = self.effective_stroke_color();
        if (state.fill_alpha - state.stroke_alpha).abs() > f64::EPSILON
            || fill_color != stroke_color
        {
            let saved_path = self.current_path.clone();
            self.fill(rule)?;
            self.replay_path_to_device(&saved_path);
            self.current_path = saved_path;
            return self.stroke();
        }

        // For fill and stroke, we use fill color for fill, stroke color for stroke
        // But our Device trait only takes one paint, so we use fill color
        let paint = Paint::from_color(fill_color);
        let stroke_props = state.stroke_props.clone();
        let fill_alpha = state.fill_alpha;
        self.device.set_opacity(fill_alpha);
        self.device
            .draw_path(PathDrawMode::FillStroke(rule), &paint, &stroke_props)?;
        self.current_path.begin();
        Ok(())
    }

    fn close_fill_stroke(&mut self, rule: FillRule) -> PDFResult<()> {
        self.close_path()?;
        self.fill_and_stroke(rule)
    }

    fn end_path(&mut self) -> PDFResult<()> {
        self.apply_pending_clip()?;
        self.current_path.begin();
        self.device.begin_path();
        Ok(())
    }

    fn effective_fill_color(&self) -> Color {
        self.type3_shape_color
            .or(self.stencil_pattern_color)
            .unwrap_or_else(|| self.current_state().fill_color)
    }

    fn effective_stroke_color(&self) -> Color {
        self.type3_shape_color
            .or(self.stencil_pattern_color)
            .unwrap_or_else(|| self.current_state().stroke_color)
    }

    fn replay_path_to_device(&mut self, path: &Path) {
        self.device.begin_path();
        for element in path.elements() {
            match *element {
                PathElement::MoveTo(x, y) => self.device.move_to(x, y),
                PathElement::LineTo(x, y) => self.device.line_to(x, y),
                PathElement::CurveTo(cp1x, cp1y, cp2x, cp2y, x, y) => {
                    self.device.curve_to(cp1x, cp1y, cp2x, cp2y, x, y)
                }
                PathElement::ClosePath => self.device.close_path(),
            }
        }
    }

    // === Clipping Operators ===

    fn clip(&mut self, rule: FillRule) -> PDFResult<()> {
        self.pending_clip = Some(rule);
        Ok(())
    }

    fn apply_pending_clip(&mut self) -> PDFResult<()> {
        let Some(rule) = self.pending_clip.take() else {
            return Ok(());
        };
        if self.current_path.is_empty() {
            return Ok(());
        }
        self.device.clip_path(rule)?;
        self.clip_stack.push(rule);
        self.replay_path_to_device(&self.current_path.clone());
        Ok(())
    }

    // === Text Object Operators ===

    fn begin_text(&mut self) -> PDFResult<()> {
        self.in_text_object = true;
        // Reset text matrices
        self.current_state_mut().text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        self.current_state_mut().text_line_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        Ok(())
    }

    fn end_text(&mut self) -> PDFResult<()> {
        // PDF 1.7 §9.3.6: glyph outlines accumulated in a clipping text mode
        // (Tr 4–7) are combined into a single path and intersected with the
        // current clipping path when the text object ends (ET).
        self.device.flush_text_clip()?;
        self.in_text_object = false;
        Ok(())
    }

    // === Text Operators ===

    fn show_text(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if !self.in_text_object {
            return Err(PDFError::content_stream_error(
                "Tj operator outside text object".to_string(),
            ));
        }

        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tj operator requires 1 argument".to_string(),
            ));
        }

        let state = self.current_state();
        let font_name = state
            .font_name
            .clone()
            .unwrap_or_else(|| "Default".to_string());
        let font_size = state.font_size.unwrap_or(12.0);
        let rendering_mode = state.text_rendering_mode;
        // Fill/stroke paints must both be available for Tr modes 1/2/5/6 (stroke /
        // fill-stroke ± clip). Backends with outline access paint via
        // draw_text_advance_with_mode; fill-only backends fall back to fill paint.
        let fill_paint = text_paint_for_rendering_mode(rendering_mode, self.effective_fill_color());
        let stroke_paint =
            text_stroke_paint_for_rendering_mode(rendering_mode, self.effective_stroke_color());
        let stroke_props = state.stroke_props.clone();
        let text_matrix = state.text_matrix;
        let horizontal_scaling = state.text_horizontal_scaling;
        let text_rise = state.text_rise;
        let character_spacing = state.character_spacing;
        let word_spacing = state.word_spacing;

        // Extract text bytes (using font's encoding, NOT UTF-8).
        // PDF text strings can be literal strings (...) or hex strings <...>.
        if let Some(bytes) = text_bytes(&args[0]) {
            if self.type3_fonts.get(&font_name).is_some() {
                let rendered_width = self.show_type3_text(
                    &font_name,
                    bytes,
                    font_size,
                    &text_matrix,
                    horizontal_scaling,
                    text_rise,
                    character_spacing,
                    word_spacing,
                )?;

                let state = self.current_state_mut();
                state.text_matrix = post_concat_translate(state.text_matrix, rendered_width, 0.0);
                return Ok(());
            }

            let mapped_bytes = self.map_text_bytes_for_device(&font_name, bytes);
            let (rendered_width, rendered_height) = self.draw_text_run(
                mapped_bytes.as_ref(),
                &font_name,
                font_size,
                &fill_paint,
                &stroke_paint,
                &stroke_props,
                rendering_mode,
                &text_matrix,
                horizontal_scaling,
                text_rise,
                character_spacing,
                word_spacing,
            )?;

            if is_text_trace_enabled() {
                eprintln!(
                    "[text-trace] Tj bytes={:?} advance=({}, {}) Tc={} Tw={} hscale={}",
                    bytes,
                    rendered_width,
                    rendered_height,
                    character_spacing,
                    word_spacing,
                    horizontal_scaling
                );
            }

            // Advance the current text matrix (Tm = Tm * T(tx, ty)).
            // Horizontal text uses ty=0; Identity-V vertical uses tx=0.
            // The text line matrix (Tlm) must stay unchanged for T*/' operators.
            let state = self.current_state_mut();
            state.text_matrix =
                post_concat_translate(state.text_matrix, rendered_width, rendered_height);
        }

        Ok(())
    }

    /// Draw a simple-font / Type0 glyph run, applying Pattern Type 2 fill
    /// shading when the current fill colour is a pattern (sticky `scn`).
    ///
    /// Source: PDF 1.7 §8.7.3 / §9.3.6; fixture `pattern_shading_on_text.pdf`
    /// fills text with `/Pattern cs /p0 scn` + `0 Tr`.
    fn draw_text_run(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        fill_paint: &Paint,
        stroke_paint: &Paint,
        stroke_props: &super::graphics_state::StrokeProps,
        rendering_mode: super::graphics_state::TextRenderingMode,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        character_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<(f64, f64)> {
        use super::graphics_state::TextRenderingMode;

        let pattern_fill = mode_fills(rendering_mode) && self.pending_fill_shading.is_some();

        if pattern_fill {
            if let Some(shading) = self.pending_fill_shading.clone() {
                // Clip to glyph interiors, paint the shading through the clip,
                // then restore so subsequent drawing is not clipped.
                self.device.save_state();
                self.device.clip_text(
                    text_bytes,
                    font_name,
                    font_size,
                    text_matrix,
                    horizontal_scaling,
                    text_rise,
                    character_spacing,
                    word_spacing,
                );
                self.device.flush_text_clip()?;
                let base_ctm = if shading.use_root_ctm {
                    self.root_ctm()
                } else {
                    self.current_state().ctm
                };
                let combined = post_concat_matrix(base_ctm, shading.matrix);
                let fill_alpha = self.current_state().fill_alpha;
                self.device.set_opacity(fill_alpha);
                self.device.draw_shading(&shading, &combined)?;
                self.device.restore_state();
            }

            // Advance (and optional stroke) without a solid fill.
            let advance_mode = if mode_strokes(rendering_mode) {
                if mode_clips(rendering_mode) {
                    TextRenderingMode::StrokeClip
                } else {
                    TextRenderingMode::Stroke
                }
            } else if mode_clips(rendering_mode) {
                TextRenderingMode::Clip
            } else {
                TextRenderingMode::Invisible
            };

            let (w, h) = self.device.draw_text_advance_with_mode(
                text_bytes,
                font_name,
                font_size,
                &Paint::Transparent,
                stroke_paint,
                stroke_props,
                advance_mode,
                text_matrix,
                horizontal_scaling,
                text_rise,
                character_spacing,
                word_spacing,
            )?;

            // Clip modes still accumulate glyph outlines for ET flush.
            if mode_clips(rendering_mode) {
                self.device.clip_text(
                    text_bytes,
                    font_name,
                    font_size,
                    text_matrix,
                    horizontal_scaling,
                    text_rise,
                    character_spacing,
                    word_spacing,
                );
            }

            return Ok((w, h));
        }

        let (w, h) = self.device.draw_text_advance_with_mode(
            text_bytes,
            font_name,
            font_size,
            fill_paint,
            stroke_paint,
            stroke_props,
            rendering_mode,
            text_matrix,
            horizontal_scaling,
            text_rise,
            character_spacing,
            word_spacing,
        )?;

        if mode_clips(rendering_mode) {
            self.device.clip_text(
                text_bytes,
                font_name,
                font_size,
                text_matrix,
                horizontal_scaling,
                text_rise,
                character_spacing,
                word_spacing,
            );
        }

        Ok((w, h))
    }

    fn show_spaced_text(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        // TJ operator - array of strings and numbers for individual glyph positioning
        // Strings are rendered text, numbers are negative adjustments (in 1/1000 em units)
        if !self.in_text_object {
            return Err(PDFError::content_stream_error(
                "TJ operator outside text object".to_string(),
            ));
        }

        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "TJ operator requires 1 argument (array)".to_string(),
            ));
        }

        let state = self.current_state();
        let font_name = state
            .font_name
            .clone()
            .unwrap_or_else(|| "Default".to_string());
        let font_size = state.font_size.unwrap_or(12.0);
        let rendering_mode = state.text_rendering_mode;
        let fill_paint = text_paint_for_rendering_mode(rendering_mode, self.effective_fill_color());
        let stroke_paint =
            text_stroke_paint_for_rendering_mode(rendering_mode, self.effective_stroke_color());
        let stroke_props = state.stroke_props.clone();
        let horizontal_scaling = state.text_horizontal_scaling;
        let text_rise = state.text_rise;
        let character_spacing = state.character_spacing;
        let word_spacing = state.word_spacing;

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: show_spaced_text: fill_color={:?}, stroke_color={:?}, mode={:?}",
            self.effective_fill_color(),
            self.effective_stroke_color(),
            rendering_mode
        );
        // We'll get text_matrix fresh for each element since it changes as we render

        // Extract the array
        let array = match &args[0] {
            PDFObject::Array(arr) => arr,
            _ => {
                return Err(PDFError::content_stream_error(
                    "TJ operator argument must be an array".to_string(),
                ));
            }
        };

        // Process each element in the array
        for element in array {
            match element.as_ref() {
                PDFObject::String(bytes) | PDFObject::HexString(bytes) => {
                    // Render this text string (using font encoding, NOT UTF-8)
                    if !bytes.is_empty() {
                        if self.type3_fonts.get(&font_name).is_some() {
                            let text_matrix = self.current_state().text_matrix;
                            let rendered_width = self.show_type3_text(
                                &font_name,
                                bytes,
                                font_size,
                                &text_matrix,
                                horizontal_scaling,
                                text_rise,
                                character_spacing,
                                word_spacing,
                            )?;

                            let state = self.current_state_mut();
                            state.text_matrix =
                                post_concat_translate(state.text_matrix, rendered_width, 0.0);
                            continue;
                        }

                        let mapped_bytes = self.map_text_bytes_for_device(&font_name, bytes);
                        let text_matrix = self.current_state().text_matrix;
                        let (rendered_width, rendered_height) = self.draw_text_run(
                            mapped_bytes.as_ref(),
                            &font_name,
                            font_size,
                            &fill_paint,
                            &stroke_paint,
                            &stroke_props,
                            rendering_mode,
                            &text_matrix,
                            horizontal_scaling,
                            text_rise,
                            character_spacing,
                            word_spacing,
                        )?;

                        if is_text_trace_enabled() {
                            eprintln!(
                                "[text-trace] TJ text bytes={:?} advance=({}, {}) Tc={} Tw={} hscale={}",
                                bytes,
                                rendered_width,
                                rendered_height,
                                character_spacing,
                                word_spacing,
                                horizontal_scaling
                            );
                        }

                        // Advance text matrix by the actual rendered advance.
                        let state = self.current_state_mut();
                        state.text_matrix = post_concat_translate(
                            state.text_matrix,
                            rendered_width,
                            rendered_height,
                        );
                    }
                }
                PDFObject::Number(offset) => {
                    // TJ numbers are spacing adjustments in thousandths of an em.
                    // Positive numbers tighten spacing, so the translation is negative.
                    // Vertical writing mode (Identity-V): advance along ty and do not
                    // apply Tz — matches PDF 32000-1:2008 §9.4.4 and hayro
                    // `TextState::apply_adjustment`.
                    // Source: hayro/hayro-interpret/src/interpret/state.rs
                    let vertical = self.device.font_is_vertical(&font_name);
                    let scale = if vertical {
                        1.0
                    } else {
                        horizontal_scaling / 100.0
                    };
                    let adjust = -(offset * font_size * scale) / 1000.0;
                    if is_text_trace_enabled() {
                        eprintln!(
                            "[text-trace] TJ adjust raw={} translated={} vertical={}",
                            offset, adjust, vertical
                        );
                    }
                    let state = self.current_state_mut();
                    if vertical {
                        state.text_matrix = post_concat_translate(state.text_matrix, 0.0, adjust);
                    } else {
                        state.text_matrix = post_concat_translate(state.text_matrix, adjust, 0.0);
                    }
                }
                PDFObject::Null => {
                    // Explicit null - do nothing
                }
                _ => {
                    // Ignore other types
                }
            }
        }

        Ok(())
    }

    fn next_line_show_text(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        self.next_line()?;
        self.show_text(args)
    }

    fn next_line_set_spacing_show_text(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 3 {
            return Err(PDFError::content_stream_error(
                "\" operator requires 3 arguments".to_string(),
            ));
        }

        let word_spacing = extract_number(args, 0)?;
        let char_spacing = extract_number(args, 1)?;

        self.current_state_mut().word_spacing = word_spacing;
        self.current_state_mut().character_spacing = char_spacing;

        self.next_line()?;
        self.show_text(&args[2..])
    }

    fn show_type3_text(
        &mut self,
        font_name: &str,
        bytes: &[u8],
        font_size: f64,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        character_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<f64> {
        let Some(font) = self.type3_fonts.get(font_name).cloned() else {
            return Ok(0.0);
        };

        let mut cursor = 0.0;
        for &code in bytes {
            let spacing = character_spacing + if code == b' ' { word_spacing } else { 0.0 };
            let glyph_width = font.width_for_code(code);
            let gname = font.encoding.glyph_name(code);
            let has = gname
                .as_ref()
                .map(|n| font.char_procs.contains_key(n))
                .unwrap_or(false);

            if let Some(glyph_name) = font.encoding.glyph_name(code) {
                if let Some(glyph) = font.char_procs.get(&glyph_name) {
                    self.render_type3_glyph(
                        &font,
                        glyph,
                        text_matrix,
                        font_size,
                        horizontal_scaling,
                        text_rise,
                        cursor,
                    )?;
                }
            }

            cursor += (glyph_width * font_size + spacing) * (horizontal_scaling / 100.0);
        }

        Ok(cursor)
    }

    fn render_type3_glyph(
        &mut self,
        font: &Type3Font,
        glyph: &Type3Glyph,
        text_matrix: &[f64; 6],
        font_size: f64,
        horizontal_scaling: f64,
        text_rise: f64,
        cursor: f64,
    ) -> PDFResult<()> {
        let base_ctm = self.current_state().ctm;
        let glyph_text_matrix = post_concat_translate(*text_matrix, cursor, 0.0);
        let text_transform = compose_type3_text_transform(
            glyph_text_matrix,
            font_size,
            horizontal_scaling,
            text_rise,
        );
        let glyph_ctm = post_concat_matrix(
            post_concat_matrix(base_ctm, text_transform),
            font.font_matrix,
        );
        let previous_resources = self.resources.clone();
        let glyph_resources = {
            let mut resolver_opt = self.resolver.take();
            let merged = if resolver_opt.is_some() {
                let font_resources = merge_pattern_resources(
                    font.resources.clone(),
                    previous_resources.clone(),
                    &mut resolver_opt,
                );
                merge_pattern_resources(glyph.resources.clone(), font_resources, &mut resolver_opt)
            } else {
                glyph
                    .resources
                    .clone()
                    .or_else(|| font.resources.clone())
                    .or_else(|| previous_resources.clone())
            };
            self.resolver = resolver_opt;
            merged
        };

        self.save()?;
        if let Some(resources) = glyph_resources {
            self.resources = Some(resources);
        }
        self.current_state_mut().set_matrix(&glyph_ctm);
        self.device.set_matrix(&glyph_ctm);

        let previous_type3_shape_color = self.type3_shape_color;
        if glyph.is_shape_glyph {
            self.type3_shape_color = Some(self.current_state().fill_color);
        }
        self.device.begin_type3_glyph();
        let result = self.process_content_stream_bytes(glyph.data.clone());
        self.device.end_type3_glyph();
        self.type3_shape_color = previous_type3_shape_color;

        self.resources = previous_resources;
        let restore_result = self.restore();
        result?;
        restore_result
    }

    fn move_text(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "Td operator requires 2 arguments".to_string(),
            ));
        }

        let tx = extract_number(args, 0)?;
        let ty = extract_number(args, 1)?;

        let state = self.current_state_mut();
        state.text_line_matrix = post_concat_translate(state.text_line_matrix, tx, ty);
        state.text_matrix = state.text_line_matrix;

        Ok(())
    }

    fn set_leading_move_text(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "TD operator requires 2 arguments".to_string(),
            ));
        }

        let ty = extract_number(args, 1)?;
        self.current_state_mut().text_leading = -ty; // Leading is negative of ty

        self.move_text(args)
    }

    fn set_text_matrix(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 6 {
            return Err(PDFError::content_stream_error(
                "Tm operator requires 6 arguments".to_string(),
            ));
        }

        let mut matrix = [0.0; 6];
        for i in 0..6 {
            matrix[i] = extract_number(args, i)?;
        }

        self.current_state_mut().set_text_matrix(&matrix);
        Ok(())
    }

    fn next_line(&mut self) -> PDFResult<()> {
        let state = self.current_state_mut();
        state.text_line_matrix =
            post_concat_translate(state.text_line_matrix, 0.0, -state.text_leading);
        state.text_matrix = state.text_line_matrix;
        Ok(())
    }

    fn set_font(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "Tf operator requires 2 arguments".to_string(),
            ));
        }

        if let crate::core::parser::PDFObject::Name(name) = &args[0] {
            self.current_state_mut().font_name = Some(name.clone());

            // Skip font loading entirely if the device already has it (from a
            // previous render or earlier in this render). This is the single most
            // impactful render optimization: Type1/CFF parsing is ~2ms per font
            // and a page may switch fonts dozens of times.
            if self.device.has_font(name.as_str()) {
                self.current_state_mut().font_size = Some(extract_number(args, 1)?);
                return Ok(());
            }

            // Font not yet in device cache — load from resources.
            let (resolver_opt, resources_opt) = (self.resolver.take(), self.resources.clone());
            if let (Some(mut resolver), Some(resources)) = (resolver_opt, resources_opt) {
                let result =
                    self.load_font_from_resources(name.as_str(), &resources, &mut *resolver);
                self.resolver = Some(resolver);
                result?;
            }
        }

        self.current_state_mut().font_size = Some(extract_number(args, 1)?);
        Ok(())
    }

    fn map_text_bytes_for_device<'b>(&self, font_name: &str, bytes: &'b [u8]) -> Cow<'b, [u8]> {
        if !self
            .font_requires_cmap_remap
            .get(font_name)
            .copied()
            .unwrap_or(false)
        {
            return Cow::Borrowed(bytes);
        }

        let Some(cmap) = self.font_to_unicode.get(font_name) else {
            return Cow::Borrowed(bytes);
        };

        if let Some(mapped) = remap_text_bytes_with_cmap(bytes, cmap) {
            Cow::Owned(mapped)
        } else {
            Cow::Borrowed(bytes)
        }
    }

    fn load_to_unicode_cmap(
        &mut self,
        font_name: &str,
        font_dict_info: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) {
        self.font_to_unicode.remove(font_name);

        let Some(to_unicode_obj) = font_dict_info.to_unicode.as_ref() else {
            return;
        };

        let fetched = match resolver.resolve_if_ref(to_unicode_obj) {
            Ok(obj) => obj,
            Err(_) => return,
        };

        let (dict, data) = match &*fetched {
            PDFObject::Stream { dict, data } => (dict.clone(), data.clone()),
            _ => return,
        };

        let decoded = if let Some(filters) = dict.get("Filter") {
            crate::core::decode::apply_filters(&data, filters).unwrap_or(data)
        } else {
            data
        };

        if let Ok(cmap) = CMap::parse(&decoded) {
            self.font_to_unicode.insert(font_name.to_string(), cmap);
        }
    }

    /// Load a font from the page's resources dictionary.
    ///
    /// This looks up the font in the Resources/Font dictionary, extracts the font data,
    /// and loads it into the rendering device.
    fn load_font_from_resources(
        &mut self,
        font_name: &str,
        resources: &PDFObject,
        resolver: &mut dyn PageResolver,
    ) -> PDFResult<()> {
        // Get the Font dictionary from resources
        let resources_dict = match resources {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(()), // No resources dictionary
        };

        let font_dict_value = match resources_dict.get("Font") {
            Some(f) => f,
            None => return Ok(()), // No Font dictionary
        };

        // Fetch the Font dictionary (might be a reference)
        let _fetched_font_dict;
        let font_dict = match font_dict_value {
            PDFObject::Dictionary(d) => d,
            PDFObject::Ref(ref_obj) => {
                _fetched_font_dict = resolver.resolve_ref(ref_obj.num, ref_obj.generation)?;
                match &*_fetched_font_dict {
                    PDFObject::Dictionary(d) => d,
                    _ => return Ok(()),
                }
            }
            _ => return Ok(()),
        };

        // Get the specific font object
        let font_obj_ref = match font_dict.get(font_name) {
            Some(f) => f,
            None => {
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!("DEBUG: Font '{}' not found in Font dictionary", font_name);
                return Ok(());
            }
        };

        // Fetch the font object (might be a reference)
        let font_obj = resolver.resolve_if_ref(font_obj_ref)?;

        // Parse the font dictionary
        let mut font_dict_info = match crate::core::font::FontDict::from_pdf_object(&*font_obj) {
            Ok(f) => f,
            Err(e) => {
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Failed to parse font dictionary for '{}': {:?}",
                    font_name, e
                );
                return Ok(()); // Don't fail rendering just because font parsing failed
            }
        };

        // `/Widths` is frequently an indirect object (e.g. pdfTeX
        // `font_type1_1.pdf`: `/Widths 7 0 R`). `FontDict::from_pdf_object`
        // only accepts an immediate array, so resolve the ref here before
        // `set_font_widths`. Without this, Type1 advances fall back to a flat
        // 500 and letterspaced titles compress vs mutool/hayro.
        if font_dict_info.widths.is_none() {
            if let PDFObject::Dictionary(dict) = font_obj.as_ref() {
                if let Some(widths_obj) = dict.get("Widths") {
                    if let Ok(resolved) = resolver.resolve_if_ref(widths_obj) {
                        if let PDFObject::Array(arr) = resolved.as_ref() {
                            let mut widths = Vec::with_capacity(arr.len());
                            for item in arr {
                                if let PDFObject::Number(w) = item.as_ref() {
                                    widths.push(*w);
                                }
                            }
                            if !widths.is_empty() {
                                font_dict_info.widths = Some(widths);
                            }
                        }
                    }
                }
            }
        }

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Font '{}' - BaseFont: {}, Type: {:?}",
            font_name, font_dict_info.base_font, font_dict_info.font_type
        );

        let requires_cmap_remap = font_needs_to_unicode_remap(&font_dict_info);
        self.font_requires_cmap_remap
            .insert(font_name.to_string(), requires_cmap_remap);
        if requires_cmap_remap {
            // Cache /ToUnicode mapping for composite/CID-like fonts where raw
            // shown bytes are not directly usable by the drawing backend.
            self.load_to_unicode_cmap(font_name, &font_dict_info, resolver);
        } else {
            self.font_to_unicode.remove(font_name);
        }

        if matches!(font_dict_info.font_type, crate::core::font::FontType::Type3) {
            if let Some(type3) = build_type3_font(&font_obj, &font_dict_info, resolver) {
                self.type3_fonts.insert(font_name.to_string(), type3);
                return Ok(());
            }
            self.type3_fonts.remove(font_name);
        } else {
            self.type3_fonts.remove(font_name);
        }

        // Try to get font data
        let font_data = self.extract_font_data(&font_dict_info, resolver);

        if let Some(data) = font_data {
            #[cfg(feature = "debug-logging")]
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Loading font '{}' with {} bytes of data",
                font_name,
                data.len()
            );
            // Try to load the font data with encoding, but fall back to bundled standard fonts if it fails
            // (e.g., CFF fonts can't be parsed by ttf_parser)
            // Dereference encoding if it's a reference
            let encoding_obj = if let Some(enc) = &font_dict_info.encoding {
                match resolver.resolve_if_ref(enc) {
                    Ok(fetched) => Some(fetched),
                    Err(_) => Some(Arc::new(enc.clone())),
                }
            } else {
                None
            };
            let encoding_ref = encoding_obj.as_ref();
            match self
                .device
                .load_font_data(font_name, data, encoding_ref.map(|v| &**v))
            {
                Ok(_) => {
                    if let (Some(first_char), Some(widths)) =
                        (font_dict_info.first_char, font_dict_info.widths.as_ref())
                    {
                        let _ = self.device.set_font_widths(
                            font_name,
                            first_char,
                            widths,
                            font_dict_info.default_width,
                        );
                    }

                    if font_uses_identity_cid_to_gid(&font_obj, &font_dict_info, resolver) {
                        let _ = self.device.set_font_raw_cid_codes(font_name, true);
                    }

                    if self.device.font_wants_raw_cid_codes(font_name) {
                        self.font_requires_cmap_remap
                            .insert(font_name.to_string(), false);

                        if font_dict_info.widths.is_none() {
                            if let Some((first_cid, widths, default_width)) =
                                self.extract_descendant_cid_widths(&font_dict_info, resolver)
                            {
                                let _ = self.device.set_font_widths(
                                    font_name,
                                    first_cid,
                                    &widths,
                                    default_width,
                                );
                            } else if let Some(dw) =
                                self.extract_descendant_cid_dw(&font_dict_info, resolver)
                            {
                                // Identity CIDFont with only `/DW` (no `/W`).
                                let _ = self.device.set_font_widths(font_name, 0, &[], dw);
                            }
                        }

                        // Identity-V: vertical writing mode uses `/DW2` + `/W2`.
                        // PDF 32000-1:2008 §9.7.4.3.
                        if font_encoding_is_identity_v(&font_dict_info) {
                            let (dw2, widths2) = self
                                .extract_descendant_cid_vertical_metrics(&font_dict_info, resolver)
                                .unwrap_or(((880.0, -1000.0), Vec::new()));
                            let _ = self
                                .device
                                .set_font_cid_vertical_metrics(font_name, true, dw2, &widths2);
                        }
                    }

                    #[cfg(feature = "debug-logging")]
                    #[cfg(feature = "debug-logging")]
                    eprintln!(
                        "DEBUG: Successfully loaded font '{}' into device",
                        font_name
                    );
                    return Ok(());
                }
                Err(_e) => {
                    // Fall through to try bundled standard fonts.
                }
            }
        } else {
            #[cfg(feature = "debug-logging")]
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: No font data available for '{}', attempting standard font",
                font_name
            );
        }

        // Try to load as a bundled standard font.
        // Pass font_name as the cache key (e.g., "F0") and base_font for Base14 mapping.
        // Encoding is often an indirect ref (e.g. Distiller `/Encoding 40 0 R` with
        // Differences); resolve before load so WinAnsi+Differences apply to substitutes.
        let encoding_resolved = if let Some(enc) = &font_dict_info.encoding {
            match resolver.resolve_if_ref(enc) {
                Ok(fetched) => Some(fetched),
                Err(_) => Some(std::sync::Arc::new(enc.clone())),
            }
        } else {
            None
        };
        self.load_standard_font(
            font_name,
            &font_dict_info.base_font,
            encoding_resolved.as_ref().map(|a| a.as_ref()),
        )?;

        // Type0 Identity-H may still need raw-CID mode when we only reached
        // the standard-font fallback (embedded CFF load failed earlier).
        if font_uses_identity_cid_to_gid(&font_obj, &font_dict_info, resolver) {
            let _ = self.device.set_font_raw_cid_codes(font_name, true);
            if let Some((first_cid, widths, default_width)) =
                self.extract_descendant_cid_widths(&font_dict_info, resolver)
            {
                let _ = self
                    .device
                    .set_font_widths(font_name, first_cid, &widths, default_width);
            }
            self.font_requires_cmap_remap
                .insert(font_name.to_string(), false);
        }

        if let (Some(first_char), Some(widths)) =
            (font_dict_info.first_char, font_dict_info.widths.as_ref())
        {
            let _ = self.device.set_font_widths(
                font_name,
                first_char,
                widths,
                font_dict_info.default_width,
            );
        }

        Ok(())
    }

    /// Resolve Type0 `/DescendantFonts` to the first CIDFont dictionary.
    ///
    /// The array is often an indirect ref (`/DescendantFonts 41 0 R`).
    fn resolve_first_descendant_cid_dict(
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<std::sync::Arc<PDFObject>> {
        let desc_fonts = font_dict.descendant_fonts.as_ref()?;
        let resolved = resolver.resolve_if_ref(desc_fonts).ok()?;
        let PDFObject::Array(descendants) = &*resolved else {
            return None;
        };
        let cid_ref = descendants.first()?;
        let cid_obj = resolver.resolve_if_ref(cid_ref).ok()?;
        match cid_obj.as_ref() {
            PDFObject::Dictionary(_) | PDFObject::Stream { .. } => Some(cid_obj),
            _ => None,
        }
    }

    fn extract_descendant_cid_widths(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<(u32, Vec<f64>, f64)> {
        let cid_obj = Self::resolve_first_descendant_cid_dict(font_dict, resolver)?;
        let cid_dict = match cid_obj.as_ref() {
            PDFObject::Dictionary(dict) => dict,
            PDFObject::Stream { dict, .. } => dict,
            _ => return None,
        };

        let default_width = cid_dict
            .get("DW")
            .and_then(PDFObject::as_number)
            .unwrap_or(1000.0);
        // `/W` is often an indirect reference (e.g. xdvipdfmx); resolve before parse.
        // PDF 32000-1:2008 §9.7.4.3.
        let w_obj = cid_dict.get("W")?;
        let w_resolved = resolver.resolve_if_ref(w_obj).ok()?;
        let width_entries = match &*w_resolved {
            PDFObject::Array(arr) => arr,
            _ => return None,
        };

        let mut table = Vec::new();
        let mut saw_width = false;
        let mut index = 0;

        while index < width_entries.len() {
            let Some(first_cid) = width_entries[index].as_number().map(|n| n as u32) else {
                break;
            };
            index += 1;

            let Some(entry) = width_entries.get(index) else {
                break;
            };

            match &**entry {
                PDFObject::Array(widths) => {
                    for (offset, width_obj) in widths.iter().enumerate() {
                        let cid = first_cid.saturating_add(offset as u32);
                        if cid as usize >= table.len() {
                            table.resize(cid as usize + 1, default_width);
                        }
                        if let Some(width) = width_obj.as_number() {
                            table[cid as usize] = width;
                            saw_width = true;
                        }
                    }
                    index += 1;
                }
                PDFObject::Number(last_cid) => {
                    let last_cid = *last_cid as u32;
                    let Some(width) = width_entries.get(index + 1).and_then(|v| v.as_number())
                    else {
                        break;
                    };
                    if last_cid as usize >= table.len() {
                        table.resize(last_cid as usize + 1, default_width);
                    }
                    for cid in first_cid..=last_cid {
                        table[cid as usize] = width;
                        saw_width = true;
                    }
                    index += 2;
                }
                _ => break,
            }
        }

        saw_width.then_some((0, table, default_width))
    }

    /// `/DW` alone from the descendant CIDFont (when `/W` is absent).
    fn extract_descendant_cid_dw(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<f64> {
        let cid_obj = Self::resolve_first_descendant_cid_dict(font_dict, resolver)?;
        let cid_dict = match cid_obj.as_ref() {
            PDFObject::Dictionary(dict) => dict,
            PDFObject::Stream { dict, .. } => dict,
            _ => return None,
        };
        cid_dict.get("DW").and_then(PDFObject::as_number)
    }

    /// Vertical metrics from descendant CIDFont `/DW2` and `/W2`.
    ///
    /// Returns `((v1, w1), entries)` where each entry is `(cid, w1, v1, v2)`.
    /// PDF 32000-1:2008 §9.7.4.3.
    fn extract_descendant_cid_vertical_metrics(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<((f64, f64), Vec<(u32, f64, f64, f64)>)> {
        let cid_obj = Self::resolve_first_descendant_cid_dict(font_dict, resolver)?;
        let cid_dict = match cid_obj.as_ref() {
            PDFObject::Dictionary(dict) => dict,
            PDFObject::Stream { dict, .. } => dict,
            _ => return None,
        };

        // DW2 = [v1 w1]; default [880 -1000].
        let dw2 = match cid_dict.get("DW2") {
            Some(PDFObject::Array(arr)) if arr.len() >= 2 => {
                let v1 = arr[0].as_number().unwrap_or(880.0);
                let w1 = arr[1].as_number().unwrap_or(-1000.0);
                (v1, w1)
            }
            _ => (880.0, -1000.0),
        };

        let mut widths2 = Vec::new();
        if let Some(w2_obj) = cid_dict.get("W2") {
            if let Ok(w2_resolved) = resolver.resolve_if_ref(w2_obj) {
                if let PDFObject::Array(entries) = &*w2_resolved {
                    let mut index = 0;
                    while index < entries.len() {
                        let Some(first_cid) = entries[index].as_number().map(|n| n as u32) else {
                            break;
                        };
                        index += 1;
                        let Some(entry) = entries.get(index) else {
                            break;
                        };
                        match &**entry {
                            // c [w1 v1 v2 w1 v1 v2 ...]
                            PDFObject::Array(metrics) => {
                                let mut m = 0;
                                let mut cid = first_cid;
                                while m + 2 < metrics.len() {
                                    let w1 = metrics[m].as_number().unwrap_or(dw2.1);
                                    let v1 = metrics[m + 1].as_number().unwrap_or(dw2.0);
                                    let v2 = metrics[m + 2].as_number().unwrap_or(0.0);
                                    widths2.push((cid, w1, v1, v2));
                                    cid = cid.saturating_add(1);
                                    m += 3;
                                }
                                index += 1;
                            }
                            // cfirst clast w1 v1 v2
                            PDFObject::Number(last_cid) => {
                                let last_cid = *last_cid as u32;
                                let w1 = entries
                                    .get(index + 1)
                                    .and_then(|v| v.as_number())
                                    .unwrap_or(dw2.1);
                                let v1 = entries
                                    .get(index + 2)
                                    .and_then(|v| v.as_number())
                                    .unwrap_or(dw2.0);
                                let v2 = entries
                                    .get(index + 3)
                                    .and_then(|v| v.as_number())
                                    .unwrap_or(0.0);
                                for cid in first_cid..=last_cid {
                                    widths2.push((cid, w1, v1, v2));
                                }
                                index += 4;
                            }
                            _ => break,
                        }
                    }
                }
            }
        }

        Some((dw2, widths2))
    }

    /// Extract font data from a FontDict.
    ///
    /// Returns the raw font data (TrueType, CFF, etc.) if available.
    fn extract_font_data(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<Vec<u8>> {
        // For Type0 fonts, FontDescriptor is in the DescendantFonts → CIDFont dict.
        // `/DescendantFonts` is frequently an indirect array ref (e.g. `41 0 R`
        // → `[42 0 R]`); resolve before matching Array.
        let descriptor_ref = if let Some(desc) = &font_dict.font_descriptor {
            desc.clone()
        } else if let Some(desc_fonts) = &font_dict.descendant_fonts {
            let resolved_arr = match resolver.resolve_if_ref(desc_fonts) {
                Ok(obj) => obj,
                Err(_) => return None,
            };
            let PDFObject::Array(descendants) = &*resolved_arr else {
                return None;
            };
            // Type0: traverse DescendantFonts → CIDFont → FontDescriptor
            let cid_ref = descendants.first()?;
            let cid_obj = resolver.resolve_if_ref(cid_ref).ok()?;
            match &*cid_obj {
                PDFObject::Dictionary(cid_dict) => cid_dict.get("FontDescriptor")?.clone(),
                PDFObject::Stream { dict, .. } => dict.get("FontDescriptor")?.clone(),
                _ => return None,
            }
        } else {
            return None;
        };

        // Fetch the descriptor (might be a reference)
        let _fetched_descriptor;
        let descriptor = match &descriptor_ref {
            PDFObject::Dictionary(d) => d,
            PDFObject::Ref(r) => {
                _fetched_descriptor = resolver.resolve_ref(r.num, r.generation).ok()?;
                match &*_fetched_descriptor {
                    PDFObject::Dictionary(d) => d,
                    _ => return None,
                }
            }
            _ => return None,
        };

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: FontDescriptor keys: {:?}",
            descriptor.keys().collect::<Vec<_>>()
        );

        // Try to get font file from FontDescriptor
        // The key depends on font type:
        // - TrueType: /FontFile2
        // - Type1: /FontFile
        // - Type1C (CFF): /FontFile3
        //
        // NOTE: Some PDFs mislabel Type1C as Type1, so we check multiple keys
        let font_file_keys = match font_dict.font_type {
            crate::core::font::FontType::TrueType => vec!["FontFile2"],
            crate::core::font::FontType::Type1 => {
                // Type1 might actually be Type1C (CFF), so check both
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!("DEBUG: Type1 font, checking FontFile and FontFile3");
                vec!["FontFile", "FontFile3"]
            }
            crate::core::font::FontType::Type1C => vec!["FontFile3"],
            crate::core::font::FontType::CIDFontType0 => vec!["FontFile3"],
            crate::core::font::FontType::CIDFontType2 => vec!["FontFile2"],
            _ => vec!["FontFile2", "FontFile3", "FontFile"],
        };

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Looking for font file keys: {:?}", font_file_keys);

        for key in font_file_keys {
            if let Some(font_file_ref) = descriptor.get(key) {
                let font_file_obj = resolver.resolve_if_ref(font_file_ref).ok()?;
                if let PDFObject::Stream { dict, data } = &*font_file_obj {
                    let decoded = if let Some(filter) = dict.get("Filter") {
                        crate::core::decode::apply_filters(data, filter)
                            .unwrap_or_else(|_| data.clone())
                    } else {
                        data.clone()
                    };
                    return Some(decoded);
                }
            }
        }

        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: No font data found in extract_font_data");
        None
    }

    /// Load a standard PDF font by name (e.g., "Helvetica", "Times-Roman").
    ///
    /// This uses deterministic bundled fonts as a fallback for standard PDF fonts.
    /// It also handles subset prefixes (e.g., "AKXEFC+DogmaBold" -> "DogmaBold").
    ///
    /// # Arguments
    /// * `cache_key` - The PDF font resource name to use as cache key (e.g., "F0")
    /// * `base_font` - The BaseFont name from the PDF for bundled standard-font mapping
    /// * `encoding` - Optional PDF Encoding (MacRoman/WinAnsi/…) for substitute faces
    fn load_standard_font(
        &mut self,
        cache_key: &str,
        base_font: &str,
        encoding: Option<&PDFObject>,
    ) -> PDFResult<()> {
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Attempting to load bundled standard font for cache_key='{}', base_font='{}'",
            cache_key, base_font
        );

        // Strip subset prefix if present (e.g., "AKXEFC+DogmaBold" -> "DogmaBold")
        let clean_font = if let Some(idx) = base_font.find('+') {
            &base_font[idx + 1..]
        } else {
            base_font
        };

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Clean font name: '{}'", clean_font);

        let resolved = self
            .device
            .resolve_standard_font_data(clean_font)
            .or_else(|| {
                if clean_font != base_font {
                    self.device.resolve_standard_font_data(base_font)
                } else {
                    None
                }
            });

        if let Some(data) = resolved {
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Loaded adapter standard font for cache_key='{}', base_font='{}'",
                cache_key, base_font
            );
            return self.device.load_font_data(cache_key, data, encoding);
        }

        if let Some(data) = bundled_standard_font_data(clean_font) {
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Loaded bundled standard font for cache_key='{}', base_font='{}'",
                cache_key, base_font
            );
            return self.device.load_font_data(cache_key, data, encoding);
        }

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Could not find bundled standard font for cache_key='{}', base_font='{}'",
            cache_key, base_font
        );
        Ok(())
    }

    fn set_char_spacing(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tc operator requires 1 argument".to_string(),
            ));
        }

        let value = extract_number(args, 0)?;
        self.current_state_mut().character_spacing = value;
        if is_text_trace_enabled() {
            eprintln!("[text-trace] Tc set to {}", value);
        }
        Ok(())
    }

    fn set_word_spacing(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tw operator requires 1 argument".to_string(),
            ));
        }

        let value = extract_number(args, 0)?;
        self.current_state_mut().word_spacing = value;
        if is_text_trace_enabled() {
            eprintln!("[text-trace] Tw set to {}", value);
        }
        Ok(())
    }

    fn set_horizontal_scaling(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tz operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().text_horizontal_scaling = extract_number(args, 0)?;
        Ok(())
    }

    fn set_leading(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "TL operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().text_leading = extract_number(args, 0)?;
        Ok(())
    }

    fn set_text_rendering_mode(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tr operator requires 1 argument".to_string(),
            ));
        }

        let mode = extract_number(args, 0)? as i32;
        self.current_state_mut().text_rendering_mode = match mode {
            0 => super::graphics_state::TextRenderingMode::Fill,
            1 => super::graphics_state::TextRenderingMode::Stroke,
            2 => super::graphics_state::TextRenderingMode::FillStroke,
            3 => super::graphics_state::TextRenderingMode::Invisible,
            4 => super::graphics_state::TextRenderingMode::FillClip,
            5 => super::graphics_state::TextRenderingMode::StrokeClip,
            6 => super::graphics_state::TextRenderingMode::FillStrokeClip,
            7 => super::graphics_state::TextRenderingMode::Clip,
            _ => {
                return Err(PDFError::content_stream_error(format!(
                    "Invalid text rendering mode: {}",
                    mode
                )));
            }
        };
        Ok(())
    }

    fn set_text_rise(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Ts operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().text_rise = extract_number(args, 0)?;
        Ok(())
    }

    // === Color Operators ===

    fn set_stroke_gray(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "G operator requires 1 argument".to_string(),
            ));
        }

        let gray = extract_number(args, 0)?;
        // G implicitly selects DeviceGray and replaces any Pattern stroke colour.
        self.pending_stroke_shading = None;
        self.stroke_color_space = Some(PDFObject::Name("DeviceGray".to_string()));
        self.current_state_mut().stroke_color = Color::Gray(gray);
        Ok(())
    }

    fn set_fill_gray(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "g operator requires 1 argument".to_string(),
            ));
        }

        let gray = extract_number(args, 0)?;
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: set_fill_gray: gray={:.3}", gray);
        self.pending_fill_shading = None;
        self.pending_fill_tiling = None;
        self.fill_color_space = Some(PDFObject::Name("DeviceGray".to_string()));
        self.current_state_mut().fill_color = Color::Gray(gray);
        Ok(())
    }

    fn set_stroke_rgb(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 3 {
            return Err(PDFError::content_stream_error(
                "RG operator requires 3 arguments".to_string(),
            ));
        }

        let r = extract_number(args, 0)?;
        let g = extract_number(args, 1)?;
        let b = extract_number(args, 2)?;
        // RG implicitly selects DeviceRGB and replaces any Pattern stroke colour.
        self.pending_stroke_shading = None;
        self.stroke_color_space = Some(PDFObject::Name("DeviceRGB".to_string()));
        self.current_state_mut().stroke_color = Color::RGB(r, g, b);
        Ok(())
    }

    fn set_fill_rgb(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 3 {
            return Err(PDFError::content_stream_error(
                "rg operator requires 3 arguments".to_string(),
            ));
        }

        let r = extract_number(args, 0)?;
        let g = extract_number(args, 1)?;
        let b = extract_number(args, 2)?;
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: set_fill_rgb: r={}, g={}, b={}, args={:?}",
            r, g, b, args
        );
        self.pending_fill_shading = None;
        self.pending_fill_tiling = None;
        self.fill_color_space = Some(PDFObject::Name("DeviceRGB".to_string()));
        self.current_state_mut().fill_color = Color::RGB(r, g, b);
        Ok(())
    }

    fn set_stroke_cmyk(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 4 {
            return Err(PDFError::content_stream_error(
                "K operator requires 4 arguments".to_string(),
            ));
        }

        let c = extract_number(args, 0)?;
        let m = extract_number(args, 1)?;
        let y = extract_number(args, 2)?;
        let k = extract_number(args, 3)?;
        self.pending_stroke_shading = None;
        self.stroke_color_space = Some(PDFObject::Name("DeviceCMYK".to_string()));
        self.current_state_mut().stroke_color = Color::CMYK(c, m, y, k);
        Ok(())
    }

    fn set_fill_cmyk(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 4 {
            return Err(PDFError::content_stream_error(
                "k operator requires 4 arguments".to_string(),
            ));
        }

        let c = extract_number(args, 0)?;
        let m = extract_number(args, 1)?;
        let y = extract_number(args, 2)?;
        let k = extract_number(args, 3)?;
        self.pending_fill_shading = None;
        self.pending_fill_tiling = None;
        self.fill_color_space = Some(PDFObject::Name("DeviceCMYK".to_string()));
        self.current_state_mut().fill_color = Color::CMYK(c, m, y, k);
        Ok(())
    }

    fn set_stroke_color_space(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        self.stroke_color_space = self.resolve_color_space_arg(args)?;
        // Leaving a Pattern stroke colour space clears any sticky stroke pattern.
        if !self
            .stroke_color_space
            .as_ref()
            .is_some_and(is_pattern_color_space)
        {
            self.pending_stroke_shading = None;
        }
        if let Some(color) =
            initial_color_for_color_space(self.stroke_color_space.clone(), &mut self.resolver)
        {
            self.current_state_mut().stroke_color = color;
        }
        Ok(())
    }

    fn set_fill_color_space(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        self.fill_color_space = self.resolve_color_space_arg(args)?;
        if !self
            .fill_color_space
            .as_ref()
            .is_some_and(is_pattern_color_space)
        {
            self.pending_fill_shading = None;
            self.pending_fill_tiling = None;
        }
        if let Some(color) =
            initial_color_for_color_space(self.fill_color_space.clone(), &mut self.resolver)
        {
            self.current_state_mut().fill_color = color;
        }
        Ok(())
    }

    fn set_stroke_color(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        if let Some(cs) = self.stroke_color_space.clone() {
            if is_pattern_color_space(&cs) {
                return self.set_pattern_stroke(args);
            }
            if let Some(color) = self.color_from_current_space(args, Some(cs))? {
                self.pending_stroke_shading = None;
                self.current_state_mut().stroke_color = color;
            }
            return Ok(());
        }
        self.pending_stroke_shading = None;
        self.set_color_by_component_count(args, false)
    }

    fn set_fill_color(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        if let Some(color) = self.stencil_pattern_color {
            self.current_state_mut().fill_color = color;
            return Ok(());
        }

        // Check for Pattern color space
        if let Some(cs) = self.fill_color_space.clone() {
            if is_pattern_color_space(&cs) {
                return self.set_pattern_fill(args);
            }
        }
        if self.fill_color_space.is_some() {
            if let Some(color) =
                self.color_from_current_space(args, self.fill_color_space.clone())?
            {
                self.pending_fill_shading = None;
                self.pending_fill_tiling = None;
                self.current_state_mut().fill_color = color;
            }
            return Ok(());
        }
        self.pending_fill_shading = None;
        self.pending_fill_tiling = None;
        self.set_color_by_component_count(args, true)
    }

    fn set_pattern_stroke(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        // Mirror set_pattern_fill for the stroke colour operators (CS/SCN).
        // Source: PDF 1.7 §8.7.3 / text_stroked_complex_paint.pdf.
        if let Some(shading) = self.resolve_pattern_type2_shading(args) {
            self.pending_stroke_shading = Some(shading);
        }
        Ok(())
    }

    fn set_pattern_fill(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        let pattern_name = match args.last() {
            Some(PDFObject::Name(n)) => n.clone(),
            _ => return Ok(()),
        };

        let resources = match self.resources.clone() {
            Some(r) => r,
            None => return Ok(()),
        };

        let pattern_obj = {
            let resolved = resolve_object_clone(resources, &mut self.resolver);
            match resolved {
                Some(PDFObject::Dictionary(d)) => d.get("Pattern").cloned(),
                _ => None,
            }
        };

        let pattern_ref = match pattern_obj {
            Some(p) => p,
            None => return Ok(()),
        };

        let pattern_dict = resolve_object_clone(pattern_ref, &mut self.resolver);
        let pattern_entry = match pattern_dict {
            Some(PDFObject::Dictionary(d)) => d.get(&pattern_name).cloned(),
            _ => None,
        };

        let pattern_entry = match pattern_entry {
            Some(p) => p,
            None => return Ok(()),
        };

        let resolved_pattern =
            resolve_object_clone(pattern_entry, &mut self.resolver).unwrap_or(PDFObject::Null);
        let pattern_dict = match &resolved_pattern {
            PDFObject::Dictionary(d) => d,
            PDFObject::Stream { dict, .. } => dict,
            _ => return Ok(()),
        };

        let pattern_type = pattern_dict
            .get("PatternType")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;

        if pattern_type == 1 {
            self.pending_fill_shading = None;
            return self.set_tiling_pattern_fill(pattern_dict, &resolved_pattern, args);
        }

        if pattern_type != 2 {
            return Ok(());
        }

        if let Some(shading) = self.build_type2_pattern_shading(pattern_dict) {
            self.pending_fill_tiling = None;
            self.pending_fill_shading = Some(shading);
        }

        Ok(())
    }

    /// Resolve a Pattern Type 2 shading for stroke (`SCN`) from resource name args.
    fn resolve_pattern_type2_shading(
        &mut self,
        args: &[PDFObject],
    ) -> Option<super::shading::Shading> {
        let pattern_name = match args.last() {
            Some(PDFObject::Name(n)) => n.clone(),
            _ => return None,
        };
        let resources = self.resources.clone()?;
        let pattern_res = {
            let resolved = resolve_object_clone(resources, &mut self.resolver)?;
            match resolved {
                PDFObject::Dictionary(d) => d.get("Pattern").cloned()?,
                _ => return None,
            }
        };
        let pattern_dict = resolve_object_clone(pattern_res, &mut self.resolver)?;
        let pattern_entry = match pattern_dict {
            PDFObject::Dictionary(d) => d.get(&pattern_name).cloned()?,
            _ => return None,
        };
        let resolved_pattern =
            resolve_object_clone(pattern_entry, &mut self.resolver).unwrap_or(PDFObject::Null);
        let pattern_dict = match &resolved_pattern {
            PDFObject::Dictionary(d) => d,
            PDFObject::Stream { dict, .. } => dict,
            _ => return None,
        };
        let pattern_type = pattern_dict
            .get("PatternType")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;
        if pattern_type != 2 {
            return None;
        }
        self.build_type2_pattern_shading(pattern_dict)
    }

    fn build_type2_pattern_shading(
        &mut self,
        pattern_dict: &HashMap<String, PDFObject>,
    ) -> Option<super::shading::Shading> {
        let shading_ref = pattern_dict.get("Shading").cloned()?;
        let shading_obj =
            resolve_object_clone(shading_ref, &mut self.resolver).unwrap_or(PDFObject::Null);
        let (shading_dict, shading_data) = match &shading_obj {
            PDFObject::Dictionary(d) => (d, None),
            PDFObject::Stream { dict, data } => (dict, Some(data.as_slice())),
            _ => return None,
        };

        let shading_type = shading_dict
            .get("ShadingType")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;

        if !matches!(shading_type, 1..=4 | 6..=7) {
            return None;
        }

        let mut shading = parse_shading(shading_dict, shading_data, &mut self.resolver)?;
        shading.use_root_ctm = true;

        let pattern_matrix = match pattern_dict.get("Matrix") {
            Some(PDFObject::Array(arr)) if arr.len() >= 6 => {
                let vals: Vec<f64> = arr.iter().filter_map(|v| v.as_number()).collect();
                if vals.len() >= 6 {
                    [vals[0], vals[1], vals[2], vals[3], vals[4], vals[5]]
                } else {
                    [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
                }
            }
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        };
        shading.matrix = post_concat_matrix(pattern_matrix, shading.matrix);
        Some(shading)
    }

    fn set_tiling_pattern_fill(
        &mut self,
        pattern_dict: &HashMap<String, PDFObject>,
        pattern_obj: &PDFObject,
        args: &[PDFObject],
    ) -> PDFResult<()> {
        let PDFObject::Stream { data, .. } = pattern_obj else {
            return Ok(());
        };

        let bbox = match parse_four_number_array(pattern_dict.get("BBox")) {
            Some([x0, y0, x1, y1])
                if (x1 - x0).abs() > f64::EPSILON && (y1 - y0).abs() > f64::EPSILON =>
            {
                [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
            }
            _ => return Ok(()),
        };

        let x_step = pattern_dict
            .get("XStep")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0)
            .abs();
        let y_step = pattern_dict
            .get("YStep")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0)
            .abs();
        if x_step.abs() <= f64::EPSILON || y_step.abs() <= f64::EPSILON {
            return Ok(());
        }

        let paint_type = pattern_dict
            .get("PaintType")
            .and_then(|v| v.as_number())
            .unwrap_or(1.0) as i32;

        let stencil_color = if paint_type == 2 {
            let color_args = &args[..args.len().saturating_sub(1)];
            self.pattern_base_fill_color(color_args)?
                .or_else(|| Some(self.current_state().fill_color))
        } else {
            None
        };

        let decoded = if let Some(filter) = pattern_dict.get("Filter") {
            crate::core::decode::apply_filters(data, filter).unwrap_or_else(|_| data.clone())
        } else {
            data.clone()
        };

        let matrix = parse_six_number_array(pattern_dict.get("Matrix"))
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let parent_resources = self.resources.clone();
        let resources = merge_pattern_resources(
            pattern_dict.get("Resources").cloned(),
            parent_resources,
            &mut self.resolver,
        );

        self.pending_fill_tiling = Some(TilingPatternPaint {
            bbox,
            x_step,
            y_step,
            matrix,
            data: decoded,
            resources,
            paint_type,
            stencil_color,
        });

        Ok(())
    }

    fn pattern_base_fill_color(&mut self, args: &[PDFObject]) -> PDFResult<Option<Color>> {
        let Some(fill_cs) = self.fill_color_space.clone() else {
            return Ok(None);
        };
        let PDFObject::Array(arr) = fill_cs else {
            return Ok(None);
        };
        if arr.len() < 2 || !matches!(&*arr[0], PDFObject::Name(name) if name == "Pattern") {
            return Ok(None);
        }

        let base = resolve_object_clone(arr[1].as_ref().clone(), &mut self.resolver);
        self.color_from_current_space(args, base)
    }

    fn fill_with_tiling_pattern(
        &mut self,
        rule: FillRule,
        pattern: TilingPatternPaint,
    ) -> PDFResult<()> {
        if self.pattern_depth >= MAX_PATTERN_DEPTH {
            self.current_path.begin();
            self.device.begin_path();
            return Ok(());
        }

        let fill_bbox = self.current_path.bounding_box();
        if self.device.supports_pattern_cell_fill()
            && self.fill_with_tiling_pattern_cell(rule, &pattern, fill_bbox)?
        {
            self.current_path.begin();
            self.device.begin_path();
            return Ok(());
        }

        self.device.save_state();
        self.device.clip_path(rule)?;

        let result = (|| -> PDFResult<()> {
            let ctm = self.current_state().ctm;
            let base = post_concat_matrix(self.root_ctm(), pattern.matrix);
            let bounds =
                pattern_tile_bounds(&pattern, fill_bbox, self.device.page_bounds(), ctm, base);
            trace_tiling_pattern(
                "vector",
                &pattern,
                fill_bbox,
                self.device.page_bounds(),
                ctm,
                base,
                bounds,
            );

            let previous_resources = self.resources.clone();
            let previous_stencil = self.stencil_pattern_color;
            if let Some(resources) = pattern.resources.clone() {
                self.resources = Some(resources);
            }

            if pattern.paint_type == 2 {
                if let Some(color) = pattern.stencil_color {
                    self.current_state_mut().fill_color = color;
                    self.current_state_mut().stroke_color = color;
                    self.stencil_pattern_color = Some(color);
                }
            }

            self.pattern_depth += 1;
            let mut tile_count = 0usize;
            let mut render_result = Ok(());

            'tiles: for ix in bounds.0..=bounds.1 {
                for iy in bounds.2..=bounds.3 {
                    if tile_count >= MAX_PATTERN_TILES {
                        break 'tiles;
                    }
                    tile_count += 1;

                    self.save()?;
                    let mut tile_matrix = post_concat_translate(
                        base,
                        ix as f64 * pattern.x_step,
                        iy as f64 * pattern.y_step,
                    );
                    if pattern_has_overlapping_tiles(&pattern) {
                        // PDFium renders overlapping tiling cells into a cell bitmap and
                        // composites that bitmap at rounded device positions. Replaying
                        // every cell as vector content at fractional positions otherwise
                        // shifts the overlap blend by a pixel on small-step patterns.
                        tile_matrix[4] = tile_matrix[4].round();
                        tile_matrix[5] = tile_matrix[5].round();
                    }
                    self.current_state_mut().set_matrix(&tile_matrix);
                    self.device.set_matrix(&tile_matrix);

                    self.device.begin_path();
                    self.device.move_to(pattern.bbox[0], pattern.bbox[1]);
                    self.device.line_to(pattern.bbox[2], pattern.bbox[1]);
                    self.device.line_to(pattern.bbox[2], pattern.bbox[3]);
                    self.device.line_to(pattern.bbox[0], pattern.bbox[3]);
                    self.device.close_path();
                    self.device.clip_path(FillRule::NonZero)?;

                    render_result = self.process_content_stream_bytes(pattern.data.clone());
                    let restore_result = self.restore();
                    if render_result.is_err() {
                        break 'tiles;
                    }
                    restore_result?;
                }
            }

            self.pattern_depth -= 1;
            self.stencil_pattern_color = previous_stencil;
            self.resources = previous_resources;
            render_result
        })();

        self.device.restore_state();
        self.current_path.begin();
        self.device.begin_path();
        result
    }

    fn fill_with_tiling_pattern_cell(
        &mut self,
        rule: FillRule,
        pattern: &TilingPatternPaint,
        fill_bbox: Option<(f64, f64, f64, f64)>,
    ) -> PDFResult<bool> {
        if !pattern_has_overlapping_tiles(pattern) {
            return Ok(false);
        }

        let bbox_w = (pattern.bbox[2] - pattern.bbox[0]).abs();
        let bbox_h = (pattern.bbox[3] - pattern.bbox[1]).abs();
        if bbox_w <= f64::EPSILON || bbox_h <= f64::EPSILON {
            return Ok(false);
        }

        let base = post_concat_matrix(self.root_ctm(), pattern.matrix);
        let x_scale = (base[0] * base[0] + base[1] * base[1]).sqrt();
        let y_scale = (base[2] * base[2] + base[3] * base[3]).sqrt();
        if !x_scale.is_finite() || !y_scale.is_finite() || x_scale <= 0.0 || y_scale <= 0.0 {
            return Ok(false);
        }

        let step_w = pattern.x_step.abs();
        let step_h = pattern.y_step.abs();
        if step_w <= f64::EPSILON || step_h <= f64::EPSILON {
            return Ok(false);
        }

        let pix_w = (x_scale * bbox_w).ceil().clamp(1.0, 3000.0) as u32;
        let pix_h = (y_scale * bbox_h).ceil().clamp(1.0, 3000.0) as u32;
        if pix_w == 0 || pix_h == 0 {
            return Ok(false);
        }
        let render_w = pix_w;
        let render_h = pix_h;

        let saved_path = self.current_path.clone();
        let cell_scale_x = pix_w as f64 / bbox_w;
        let cell_scale_y = pix_h as f64 / bbox_h;
        let scale = [cell_scale_x, 0.0, 0.0, cell_scale_y, 0.0, 0.0];
        let translate = [1.0, 0.0, 0.0, 1.0, -pattern.bbox[0], -pattern.bbox[1]];
        let cell_transform = post_concat_matrix(scale, translate);
        let Some(cell_to_pattern) = invert_matrix(cell_transform) else {
            return Ok(false);
        };
        let image_to_device = post_concat_matrix(base, cell_to_pattern);
        let tile_bounds = pattern_tile_bounds(
            pattern,
            fill_bbox,
            self.device.page_bounds(),
            self.current_state().ctm,
            base,
        );
        trace_tiling_cell(
            pattern,
            base,
            cell_transform,
            image_to_device,
            pix_w,
            pix_h,
            render_w,
            render_h,
        );

        self.device.begin_offscreen()?;
        let previous_resources = self.resources.clone();
        let previous_stencil = self.stencil_pattern_color;
        if let Some(resources) = pattern.resources.clone() {
            self.resources = Some(resources);
        }

        self.pattern_depth += 1;
        let render_result = (|| -> PDFResult<()> {
            self.save()?;
            self.current_state_mut().set_matrix(&cell_transform);
            self.device.set_matrix(&cell_transform);
            let _ = self.device.set_soft_mask(None);
            self.device.set_opacity(1.0);

            if pattern.paint_type == 2 {
                if let Some(color) = pattern.stencil_color {
                    self.current_state_mut().fill_color = color;
                    self.current_state_mut().stroke_color = color;
                    self.stencil_pattern_color = Some(color);
                }
            }

            self.device.begin_path();
            self.device.move_to(pattern.bbox[0], pattern.bbox[1]);
            self.device.line_to(pattern.bbox[2], pattern.bbox[1]);
            self.device.line_to(pattern.bbox[2], pattern.bbox[3]);
            self.device.line_to(pattern.bbox[0], pattern.bbox[3]);
            self.device.close_path();
            self.device.clip_path(FillRule::NonZero)?;

            let result = self.process_content_stream_bytes(pattern.data.clone());
            let restore_result = self.restore();
            result?;
            restore_result
        })();
        self.pattern_depth -= 1;
        self.stencil_pattern_color = previous_stencil;
        self.resources = previous_resources;

        let Some(rendered_cell) = self.device.end_offscreen_as_rgba_region(render_w, render_h)
        else {
            return Ok(false);
        };
        render_result?;

        self.current_path = saved_path.clone();
        self.replay_path_to_device(&saved_path);
        self.device.fill_path_with_pattern_cell(
            rule,
            &rendered_cell,
            pix_w,
            pix_h,
            &image_to_device,
            &base,
            &pattern.bbox,
            pattern.x_step,
            pattern.y_step,
            tile_bounds,
        )
    }

    fn set_color_by_component_count(&mut self, args: &[PDFObject], fill: bool) -> PDFResult<()> {
        let color = match args.len() {
            1 => Color::Gray(extract_number(args, 0)?),
            3 => Color::RGB(
                extract_number(args, 0)?,
                extract_number(args, 1)?,
                extract_number(args, 2)?,
            ),
            4 => Color::CMYK(
                extract_number(args, 0)?,
                extract_number(args, 1)?,
                extract_number(args, 2)?,
                extract_number(args, 3)?,
            ),
            _ => return Ok(()),
        };
        if fill {
            self.current_state_mut().fill_color = color;
        } else {
            self.current_state_mut().stroke_color = color;
        }
        Ok(())
    }

    fn resolve_color_space_arg(&mut self, args: &[PDFObject]) -> PDFResult<Option<PDFObject>> {
        let Some(PDFObject::Name(name)) = args.first() else {
            return Ok(None);
        };
        Ok(self
            .lookup_color_space_resource(name)
            .or_else(|| Some(PDFObject::Name(name.clone()))))
    }

    fn lookup_color_space_resource(&mut self, name: &str) -> Option<PDFObject> {
        let resources = self.resources.clone()?;
        let resources = resolve_object_clone(resources, &mut self.resolver)?;
        let PDFObject::Dictionary(resources_dict) = resources else {
            return None;
        };
        let color_spaces = resources_dict.get("ColorSpace")?.clone();
        let color_spaces = resolve_object_clone(color_spaces, &mut self.resolver)?;
        let PDFObject::Dictionary(color_spaces_dict) = color_spaces else {
            return None;
        };
        let color_space = color_spaces_dict.get(name)?.clone();
        resolve_object_clone(color_space, &mut self.resolver)
    }

    fn color_from_current_space(
        &mut self,
        args: &[PDFObject],
        color_space: Option<PDFObject>,
    ) -> PDFResult<Option<Color>> {
        let Some(color_space) = color_space else {
            return Ok(None);
        };
        let color_space = match resolve_object_clone(color_space, &mut self.resolver) {
            Some(cs) => cs,
            None => return Ok(None),
        };
        Ok(color_from_color_space(
            &color_space,
            args,
            &mut self.resolver,
        ))
    }

    // === Line Property Operators ===

    fn set_line_width(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "w operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().stroke_props.line_width = extract_number(args, 0)?;
        Ok(())
    }

    fn set_line_cap(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "J operator requires 1 argument".to_string(),
            ));
        }

        let cap = extract_number(args, 0)? as i32;
        self.current_state_mut().stroke_props.line_cap = match cap {
            0 => super::graphics_state::LineCap::Butt,
            1 => super::graphics_state::LineCap::Round,
            2 => super::graphics_state::LineCap::ProjectingSquare,
            _ => {
                return Err(PDFError::content_stream_error(format!(
                    "Invalid line cap: {}",
                    cap
                )));
            }
        };
        Ok(())
    }

    fn set_line_join(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "j operator requires 1 argument".to_string(),
            ));
        }

        let join = extract_number(args, 0)? as i32;
        self.current_state_mut().stroke_props.line_join = match join {
            0 => super::graphics_state::LineJoin::Miter,
            1 => super::graphics_state::LineJoin::Round,
            2 => super::graphics_state::LineJoin::Bevel,
            _ => {
                return Err(PDFError::content_stream_error(format!(
                    "Invalid line join: {}",
                    join
                )));
            }
        };
        Ok(())
    }

    fn set_miter_limit(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "M operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().stroke_props.miter_limit = extract_number(args, 0)?;
        Ok(())
    }

    fn set_dash(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        // d operator: dash_array dash_offset
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "d operator requires 2 arguments".to_string(),
            ));
        }

        let mut dash_array = Vec::new();
        if let crate::core::parser::PDFObject::Array(arr) = &args[0] {
            for obj in arr {
                match &**obj {
                    crate::core::parser::PDFObject::Number(n) => dash_array.push(*n),
                    _ => {
                        return Err(PDFError::content_stream_error(
                            "Dash array must contain only numbers".to_string(),
                        ));
                    }
                }
            }
        }

        let dash_offset = extract_number(args, 1)?;

        self.current_state_mut().stroke_props.dash_array = dash_array;
        self.current_state_mut().stroke_props.dash_offset = dash_offset;
        Ok(())
    }

    fn set_graphics_state(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        let Some(PDFObject::Name(gs_name)) = args.first() else {
            return Ok(());
        };

        let resources = match self.resources.as_ref() {
            Some(r) => r,
            None => return Ok(()),
        };

        let resources_dict = match resources {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(()),
        };

        let ext_gstate_obj = match resources_dict.get("ExtGState") {
            Some(obj) => obj,
            None => return Ok(()),
        };

        let xref_opt = self.resolver.take();
        let resolver = match xref_opt {
            Some(resolver) => resolver,
            None => return Ok(()),
        };

        let ext_gstate_dict_obj = match resolver.resolve_if_ref(ext_gstate_obj) {
            Ok(obj) => obj,
            Err(_) => {
                self.resolver = Some(resolver);
                return Ok(());
            }
        };

        let ext_gstate_dict = match &*ext_gstate_dict_obj {
            PDFObject::Dictionary(dict) => dict,
            _ => {
                self.resolver = Some(resolver);
                return Ok(());
            }
        };

        let Some(gs_obj_ref) = ext_gstate_dict.get(gs_name) else {
            self.resolver = Some(resolver);
            return Ok(());
        };

        let gs_obj = match resolver.resolve_if_ref(gs_obj_ref) {
            Ok(obj) => obj,
            Err(_) => {
                self.resolver = Some(resolver);
                return Ok(());
            }
        };

        // Put resolver back so SMask rendering can use it
        self.resolver = Some(resolver);

        if let PDFObject::Dictionary(gs_dict) = &*gs_obj {
            if let Some(mode) = gs_dict.get("BM").and_then(parse_blend_mode) {
                self.current_state_mut().blend_mode = mode;
                self.device.set_blend_mode(mode);
            }

            // Handle fill/stroke alpha
            if let Some(ca) = gs_dict.get("ca").and_then(|v| v.as_number()) {
                self.current_state_mut().fill_alpha = ca;
                self.device.set_opacity(ca);
            }
            if let Some(ca) = gs_dict.get("CA").and_then(|v| v.as_number()) {
                self.current_state_mut().stroke_alpha = ca;
            }

            // Handle SMask (soft mask)
            let smask_val = gs_dict.get("SMask").cloned();
            let smask_resolved = match &smask_val {
                Some(PDFObject::Ref(_)) => {
                    let mut resolver = self.resolver.take();
                    let r = resolve_object_clone(smask_val.clone().unwrap(), &mut resolver);
                    self.resolver = resolver;
                    r
                }
                Some(_) => smask_val.clone(),
                None => None,
            };
            match smask_resolved.as_ref() {
                Some(PDFObject::Name(n)) if n == "None" => {
                    let _ = self.device.set_soft_mask(None);
                }
                Some(PDFObject::Dictionary(smask_dict)) => {
                    self.render_and_set_soft_mask(smask_dict.clone());
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn render_and_set_soft_mask(&mut self, smask_dict: HashMap<String, PDFObject>) {
        let luminosity = match smask_dict.get("S") {
            Some(PDFObject::Name(n)) => n == "Luminosity",
            _ => false,
        };
        let backdrop = if luminosity {
            soft_mask_backdrop_rgb(smask_dict.get("BC")).or(Some([0, 0, 0]))
        } else {
            None
        };

        let group_ref = match smask_dict.get("G") {
            Some(g) => g.clone(),
            None => return,
        };

        let group_key = match &group_ref {
            PDFObject::Ref(r) => Some((r.num, r.generation)),
            _ => None,
        };
        if group_key.is_some_and(|key| self.soft_mask_group_stack.contains(&key)) {
            return;
        }

        let group_obj = match &mut self.resolver {
            Some(r) => r.resolve_if_ref(&group_ref).ok(),
            None => None,
        };

        let (form_dict, form_data) = match group_obj.as_deref() {
            Some(PDFObject::Stream { dict, data }) => (dict.clone(), data.clone()),
            _ => return,
        };

        let decoded_data = if let Some(filter) = form_dict.get("Filter") {
            crate::core::decode::apply_filters(&form_data, filter).unwrap_or(form_data)
        } else {
            form_data
        };

        let matrix = parse_six_number_array(form_dict.get("Matrix"))
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let bbox = parse_four_number_array(form_dict.get("BBox"));

        // Render the mask content offscreen.
        // Reset opacity to 1.0 — the mask group is independent of the current ca.
        let saved_opacity = self.current_state().fill_alpha;
        self.device.set_opacity(1.0);
        if let Some(key) = group_key {
            self.soft_mask_group_stack.push(key);
        }
        let _ = self.device.begin_offscreen();

        // Save current resources and set form resources if available
        let prev_resources = self.resources.clone();
        let mut resolver = self.resolver.take();
        if let Some(form_res) = form_dict.get("Resources").cloned() {
            let resolved = resolve_object_clone(form_res, &mut resolver);
            if let Some(res) = resolved {
                self.resources = Some(res);
            }
        }
        self.resolver = resolver;

        // Process the mask content stream
        self.soft_mask_render_depth += 1;
        if self.save().is_ok() {
            self.current_state_mut().concat_matrix(&matrix);
            self.device.concat_matrix(&matrix);
            self.push_root_ctm();

            if let Some([x0, y0, x1, y1]) = bbox {
                self.device.begin_path();
                self.device.move_to(x0, y0);
                self.device.line_to(x1, y0);
                self.device.line_to(x1, y1);
                self.device.line_to(x0, y1);
                self.device.close_path();
                let _ = self.device.clip_path(FillRule::NonZero);
            }

            let _ = self.process_content_stream_bytes(decoded_data);
            self.pop_root_ctm();
            let _ = self.restore();
        }
        self.soft_mask_render_depth -= 1;

        // Restore resources
        self.resources = prev_resources;

        // Extract mask and set it
        if let Some((mask_data, mw, mh)) = self.device.end_offscreen_as_mask(luminosity, backdrop) {
            let _ = self.device.set_soft_mask(Some((&mask_data, mw, mh)));
        }
        // Restore the original opacity
        self.device.set_opacity(saved_opacity);
        if group_key.is_some() {
            self.soft_mask_group_stack.pop();
        }
    }

    // === Shading Operator ===

    fn handle_shading_fill(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        let shading_name = match args.first() {
            Some(PDFObject::Name(n)) => n.as_str(),
            _ => return Ok(()),
        };

        let resources = match self.resources.as_ref() {
            Some(PDFObject::Dictionary(d)) => d.clone(),
            _ => return Ok(()),
        };

        let shading_resources = match resources.get("Shading").cloned() {
            Some(s) => s,
            None => return Ok(()),
        };

        let shading_ref = match resolve_object_clone(shading_resources, &mut self.resolver) {
            Some(PDFObject::Dictionary(d)) => d.get(shading_name).cloned(),
            Some(other) => Some(other),
            None => None,
        };

        let shading_ref = match shading_ref {
            Some(s) => s,
            None => return Ok(()),
        };

        let shading_obj =
            resolve_object_clone(shading_ref, &mut self.resolver).unwrap_or(PDFObject::Null);
        let (shading_dict, shading_data) = match &shading_obj {
            PDFObject::Dictionary(d) => (d, None),
            PDFObject::Stream { dict, data } => (dict, Some(data.as_slice())),
            _ => return Ok(()),
        };

        let shading_type = shading_dict
            .get("ShadingType")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;

        if !matches!(shading_type, 1..=4 | 6..=7) {
            return Ok(());
        }

        let shading = match parse_shading(shading_dict, shading_data, &mut self.resolver) {
            Some(shading) => shading,
            None => return Ok(()),
        };

        let ctm = post_concat_matrix(self.current_state().ctm, shading.matrix);
        self.device.draw_shading(&shading, &ctm)
    }

    // === XObject Operator ===

    fn paint_xobject(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        // Do operator - paint an XObject (image, form, etc.)
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Do operator requires 1 argument (XObject name)".to_string(),
            ));
        }

        // Extract XObject name
        let xobject_name = match &args[0] {
            PDFObject::Name(name) => name,
            _ => {
                return Err(PDFError::content_stream_error(
                    "Do operator argument must be a name".to_string(),
                ));
            }
        };

        let resources = match self.resources.as_ref() {
            Some(r) => r,
            None => return Ok(()),
        };

        // Get resolver and resources
        let resolver = match &mut self.resolver {
            Some(x) => x,
            None => return Ok(()),
        };

        // Get the XObject dictionary from resources
        let resources_dict = match resources {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(()), // No resources dictionary
        };

        // Fetch the XObject dictionary (may be a reference)
        let xobject_dict_value = match resources_dict.get("XObject") {
            Some(xo) => xo,
            None => return Ok(()),
        };

        // Store fetched XObject dict to keep it alive during the match
        let _fetched_xobject_dict;

        let xobject_dict = match xobject_dict_value {
            PDFObject::Dictionary(d) => d,
            PDFObject::Ref(ref_obj) => {
                _fetched_xobject_dict = resolver.resolve_ref(ref_obj.num, ref_obj.generation)?;
                match &*_fetched_xobject_dict {
                    PDFObject::Dictionary(d) => d,
                    _ => return Ok(()),
                }
            }
            _ => return Ok(()),
        };

        // Get the specific XObject
        let xobject_ref = match xobject_dict.get(xobject_name) {
            Some(xobj) => xobj,
            None => return Ok(()),
        };

        let xobject = resolver.resolve_if_ref(xobject_ref)?;

        // XObjects are always stream objects.
        let (xobject_dict, xobject_data) = match &*xobject {
            PDFObject::Stream { dict, data } => (dict, data),
            _ => return Ok(()),
        };

        let subtype = match xobject_dict.get("Subtype") {
            Some(PDFObject::Name(name)) => name,
            _ => return Ok(()),
        };

        if subtype == "Form" {
            return self.paint_form_xobject(xobject_name, xobject_dict, xobject_data);
        }

        if subtype != "Image" {
            return Ok(());
        }

        // Check for image mask (1-bpp stencil: 1 = paint with current fill color).
        // PDFium also treats image dictionaries without ColorSpace as image masks.
        let is_image_mask = xobject_dict
            .get("ImageMask")
            .and_then(|v| match v {
                PDFObject::Boolean(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(false)
            || (!xobject_dict.contains_key("ColorSpace")
                && !filter_contains_any(xobject_dict.get("Filter"), &["JPXDecode"]));

        // Extract image properties
        let width = match xobject_dict.get("Width") {
            Some(PDFObject::Number(w)) => *w as u32,
            _ => return Ok(()),
        };

        let height = match xobject_dict.get("Height") {
            Some(PDFObject::Number(h)) => *h as u32,
            _ => return Ok(()),
        };

        let bits_per_component = if is_image_mask {
            1 // Image masks are always 1 bpc
        } else {
            match xobject_dict.get("BitsPerComponent") {
                Some(PDFObject::Number(b)) => *b as u8,
                _ => 8,
            }
        };

        let color_info = {
            let mut resolver = self.resolver.take();
            let info = image_color_space_info(xobject_dict, &mut resolver);
            self.resolver = resolver;
            info
        };
        let color_space_name = color_info.name.or(Some("DeviceRGB"));

        // Determine if image has alpha
        let has_alpha = match color_space_name {
            Some("DeviceRGB") | Some("DeviceGray") | Some("CalRGB") | Some("CalGray")
            | Some("DeviceCMYK") => false,
            _ => true,
        };

        // Get the image data stream
        let image_data = xobject_data;

        // Decode the image data if needed
        // For JPEG, we also need to update the image metadata
        let (decoded_data, decoded_width, decoded_height, decoded_bpc, decoded_has_alpha) =
            if let Some(filter) = xobject_dict.get("Filter") {
                match filter {
                    PDFObject::Name(filter_name) => {
                        use crate::core::decode;
                        match filter_name.as_str() {
                            "FlateDecode" | "Fl" => {
                                let decoded = decode::decode_flate(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                let (decoded, predictor_bpc) =
                                    apply_predictor_from_parms(decoded, xobject_dict, width);
                                (
                                    decoded,
                                    width,
                                    height,
                                    predictor_bpc.unwrap_or(bits_per_component),
                                    has_alpha,
                                )
                            }
                            // PDF §7.4.4 LZWDecode — used by older PageMaker-era
                            // XObject images (e.g. pdftc_900k_0974 Im1 DeviceGray).
                            // Without this arm, compressed bytes are treated as
                            // raw pixels and the image never paints (dual lag
                            // hay−mut − ours−mut ≈ +10% on that fixture).
                            "LZWDecode" | "LZW" => {
                                let decoded = decode::decode_lzw(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                let (decoded, predictor_bpc) =
                                    apply_predictor_from_parms(decoded, xobject_dict, width);
                                (
                                    decoded,
                                    width,
                                    height,
                                    predictor_bpc.unwrap_or(bits_per_component),
                                    has_alpha,
                                )
                            }
                            "ASCIIHexDecode" | "AHx" => {
                                let decoded = decode::decode_ascii_hex(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                            "ASCII85Decode" | "A85" => {
                                let decoded = decode::decode_ascii85(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                            "RunLengthDecode" | "RL" => {
                                let decoded = decode::decode_run_length(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                            "DCTDecode" | "DCT" => {
                                // JPEG data - decode it using zune-jpeg
                                #[cfg(feature = "jpeg-decoding")]
                                {
                                    match crate::core::image::ImageDecoder::decode_pdf_jpeg(
                                        image_data,
                                        jpeg_decode_options(
                                            xobject_dict,
                                            color_info.components,
                                            color_info.name,
                                        ),
                                    ) {
                                        Ok(decoded) => {
                                            // Use decoded image's metadata since JPEG decoder knows best
                                            (
                                                decoded.data,
                                                decoded.metadata.width,
                                                decoded.metadata.height,
                                                decoded.metadata.bits_per_component,
                                                decoded.metadata.has_alpha,
                                            )
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "Warning: Failed to decode JPEG image: {}",
                                                e
                                            );
                                            (
                                                image_data.clone(),
                                                width,
                                                height,
                                                bits_per_component,
                                                has_alpha,
                                            )
                                        }
                                    }
                                }
                                #[cfg(not(feature = "jpeg-decoding"))]
                                {
                                    eprintln!("Warning: JPEG decoding not enabled, skipping image");
                                    // Return empty data to prevent crash
                                    (Vec::new(), width, height, bits_per_component, has_alpha)
                                }
                            }
                            "CCITTFaxDecode" | "CCF" => {
                                #[cfg(feature = "hayro-ccitt")]
                                {
                                    match decode_ccitt(image_data, xobject_dict, width, height) {
                                        Some(data) => (data, width, height, 1, false),
                                        None => (
                                            image_data.clone(),
                                            width,
                                            height,
                                            bits_per_component,
                                            has_alpha,
                                        ),
                                    }
                                }
                                #[cfg(not(feature = "hayro-ccitt"))]
                                {
                                    (
                                        image_data.clone(),
                                        width,
                                        height,
                                        bits_per_component,
                                        has_alpha,
                                    )
                                }
                            }
                            "JPXDecode" => {
                                #[cfg(feature = "advanced-image-formats")]
                                {
                                    match decode_jpx(
                                        image_data,
                                        jpx_decode_options(xobject_dict, color_info.components),
                                    ) {
                                        Some((d, w, h, bpc, a)) => (d, w, h, bpc, a),
                                        None => (
                                            image_data.clone(),
                                            width,
                                            height,
                                            bits_per_component,
                                            has_alpha,
                                        ),
                                    }
                                }
                                #[cfg(not(feature = "advanced-image-formats"))]
                                {
                                    (
                                        image_data.clone(),
                                        width,
                                        height,
                                        bits_per_component,
                                        has_alpha,
                                    )
                                }
                            }
                            "JBIG2Decode" => {
                                #[cfg(feature = "advanced-image-formats")]
                                {
                                    let mut resolver = self.resolver.take();
                                    let globals = jbig2_globals_data(
                                        xobject_dict,
                                        xobject_dict.get("Filter"),
                                        &mut resolver,
                                    );
                                    self.resolver = resolver;
                                    match decode_jbig2(image_data, globals.as_deref()) {
                                        Some(data) => (data, width, height, 1, false),
                                        None => (
                                            image_data.clone(),
                                            width,
                                            height,
                                            bits_per_component,
                                            has_alpha,
                                        ),
                                    }
                                }
                                #[cfg(not(feature = "advanced-image-formats"))]
                                {
                                    (
                                        image_data.clone(),
                                        width,
                                        height,
                                        bits_per_component,
                                        has_alpha,
                                    )
                                }
                            }
                            _ => {
                                // Unsupported filter - try raw data
                                (
                                    image_data.clone(),
                                    width,
                                    height,
                                    bits_per_component,
                                    has_alpha,
                                )
                            }
                        }
                    }
                    PDFObject::Array(_) => {
                        // Apply non-image filters first (FlateDecode, ASCIIHex, etc.)
                        let filter = xobject_dict.get("Filter").unwrap();
                        let decoded = crate::core::decode::apply_filters(image_data, filter)
                            .unwrap_or_else(|_| image_data.clone());

                        // Check if an image-specific filter is in the chain —
                        // apply_filters skips these, so decode the image separately
                        // after the preceding non-image filters have run.
                        let filter_names: Vec<&str> = match filter {
                            PDFObject::Array(arr) => arr
                                .iter()
                                .filter_map(|f| match &**f {
                                    PDFObject::Name(n) => Some(n.as_str()),
                                    _ => None,
                                })
                                .collect(),
                            _ => Vec::new(),
                        };

                        if filter_names.contains(&"DCTDecode") || filter_names.contains(&"DCT") {
                            #[cfg(feature = "jpeg-decoding")]
                            {
                                match crate::core::image::ImageDecoder::decode_pdf_jpeg(
                                    &decoded,
                                    jpeg_decode_options(
                                        xobject_dict,
                                        color_info.components,
                                        color_info.name,
                                    ),
                                ) {
                                    Ok(img) => (
                                        img.data,
                                        img.metadata.width,
                                        img.metadata.height,
                                        8,
                                        img.metadata.has_alpha,
                                    ),
                                    Err(_) => {
                                        (decoded, width, height, bits_per_component, has_alpha)
                                    }
                                }
                            }
                            #[cfg(not(feature = "jpeg-decoding"))]
                            {
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                        } else if filter_names.contains(&"CCITTFaxDecode")
                            || filter_names.contains(&"CCF")
                        {
                            #[cfg(feature = "hayro-ccitt")]
                            {
                                match decode_ccitt(&decoded, xobject_dict, width, height) {
                                    Some(data) => (data, width, height, 1, false),
                                    None => (decoded, width, height, bits_per_component, has_alpha),
                                }
                            }
                            #[cfg(not(feature = "hayro-ccitt"))]
                            {
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                        } else if filter_names.contains(&"JPXDecode") {
                            #[cfg(feature = "advanced-image-formats")]
                            {
                                match decode_jpx(
                                    &decoded,
                                    jpx_decode_options(xobject_dict, color_info.components),
                                ) {
                                    Some((d, w, h, bpc, a)) => (d, w, h, bpc, a),
                                    None => (decoded, width, height, bits_per_component, has_alpha),
                                }
                            }
                            #[cfg(not(feature = "advanced-image-formats"))]
                            {
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                        } else if filter_names.contains(&"JBIG2Decode") {
                            // Filter array e.g. [/ASCIIHexDecode /JBIG2Decode]:
                            // apply_filters already ran non-image filters; decode JBIG2
                            // with optional /JBIG2Globals (ISO 32000-1 §7.4.7).
                            #[cfg(feature = "advanced-image-formats")]
                            {
                                let mut resolver = self.resolver.take();
                                let globals = jbig2_globals_data(
                                    xobject_dict,
                                    xobject_dict.get("Filter"),
                                    &mut resolver,
                                );
                                self.resolver = resolver;
                                match decode_jbig2(&decoded, globals.as_deref()) {
                                    Some(data) => (data, width, height, 1, false),
                                    None => (decoded, width, height, bits_per_component, has_alpha),
                                }
                            }
                            #[cfg(not(feature = "advanced-image-formats"))]
                            {
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                        } else {
                            // Apply PNG predictor from DecodeParms if present.
                            let (decoded, predictor_bpc) =
                                apply_predictor_from_parms(decoded, xobject_dict, width);
                            (
                                decoded,
                                width,
                                height,
                                predictor_bpc.unwrap_or(bits_per_component),
                                has_alpha,
                            )
                        }
                    }
                    _ => (
                        image_data.clone(),
                        width,
                        height,
                        bits_per_component,
                        has_alpha,
                    ),
                }
            } else {
                (
                    image_data.clone(),
                    width,
                    height,
                    bits_per_component,
                    has_alpha,
                )
            };

        // Expand 1-bpc sampled images to 8-bpc before color conversion. PDF image rows
        // are byte-aligned, so the padding bits at the end of each row must be skipped.
        let expand_1bpc_sampled = should_expand_1bpc_sampled_image(xobject_dict.get("Filter"));
        let expand_1bpc_as_indexed = if decoded_bpc == 1 && !is_image_mask && expand_1bpc_sampled {
            let cs_obj = xobject_dict.get("ColorSpace").cloned();
            let mut resolver = self.resolver.take();
            let is_indexed = is_indexed_color_space(cs_obj.as_ref(), &mut resolver);
            self.resolver = resolver;
            is_indexed
        } else {
            false
        };
        let (decoded_data, decoded_bpc) =
            if decoded_bpc == 1 && !is_image_mask && expand_1bpc_sampled {
                (
                    expand_1bpc_image_to_8bpc(
                        &decoded_data,
                        decoded_width,
                        decoded_height,
                        if expand_1bpc_as_indexed { 1 } else { 255 },
                    ),
                    8,
                )
            } else if decoded_bpc > 8 {
                (downscale_16_to_8(&decoded_data), 8)
            } else {
                (decoded_data, decoded_bpc)
            };

        // Handle Indexed color space: [/Indexed baseCS hival lookup_table]
        let (decoded_data, color_expanded) = {
            let cs_obj = xobject_dict.get("ColorSpace").cloned();
            let mut resolver = self.resolver.take();
            let result = expand_indexed_cs(
                &decoded_data,
                cs_obj.as_ref(),
                &mut resolver,
                decoded_bpc,
                decoded_width,
                decoded_height,
            );
            self.resolver = resolver;
            result
        };
        let decoded_bpc = if color_expanded { 8 } else { decoded_bpc };

        // If indexed CS was expanded, update the effective color space.
        let color_space_name = if color_expanded {
            Some("DeviceRGB")
        } else {
            color_space_name
        };

        // Handle Lab color space: convert L*a*b* → sRGB.
        let (decoded_data, color_space_name) =
            if let Some(PDFObject::Array(cs_arr)) = xobject_dict.get("ColorSpace") {
                if cs_arr.len() >= 2 {
                    if let PDFObject::Name(n) = &*cs_arr[0] {
                        if n == "Lab" {
                            // Parse Lab parameters
                            let (xw, yw, zw, amin, amax, bmin, bmax) = parse_lab_params(&cs_arr);
                            let rgb = lab_to_rgb(&decoded_data, xw, yw, zw, amin, amax, bmin, bmax);
                            (rgb, Some("DeviceRGB"))
                        } else {
                            (decoded_data, color_space_name)
                        }
                    } else {
                        (decoded_data, color_space_name)
                    }
                } else {
                    (decoded_data, color_space_name)
                }
            } else {
                (decoded_data, color_space_name)
            };

        let decode_arr = match xobject_dict.get("Decode") {
            Some(PDFObject::Array(decode_arr)) => Some(decode_arr),
            _ => None,
        };

        // /Decode maps raw image samples before color-space conversion. CMYK JPEGs
        // commonly use [1 0 1 0 1 0 1 0], so apply it while the data is still CMYK.
        let was_cmyk = color_space_name == Some("DeviceCMYK");
        let decoded_data = if was_cmyk {
            if let Some(decode_arr) = decode_arr {
                apply_decode_inversion(&decoded_data, decode_arr, decoded_bpc)
            } else {
                decoded_data
            }
        } else {
            decoded_data
        };

        // Handle DeviceCMYK images: convert CMYK → RGB via ICC profile.
        // (only when the `rendering` feature is enabled — which provides moxcms)
        #[cfg(feature = "rendering")]
        let (decoded_data, color_space_name) = if color_space_name == Some("DeviceCMYK") {
            let rgb = crate::rendering::cmyk_icc::cmyk_image_to_rgb(&decoded_data);
            (rgb, Some("DeviceRGB"))
        } else {
            (decoded_data, color_space_name)
        };

        // Apply /Decode for non-CMYK, non-mask images that were not handled
        // before conversion. Image masks resolve /Decode only via
        // `mask_paint_one` (which bit paints); also inverting samples here
        // double-applies Decode (image_mask.pdf Decode [1 0] painted ~6% of
        // the page instead of ~55%). Inline masks already skip this path.
        let decoded_data = if !was_cmyk && !is_image_mask {
            if let Some(decode_arr) = decode_arr {
                apply_decode_inversion(&decoded_data, decode_arr, decoded_bpc)
            } else {
                decoded_data
            }
        } else {
            decoded_data
        };

        let has_image_smask = xobject_dict
            .get("SMask")
            .is_some_and(|smask| !matches!(smask, PDFObject::Name(name) if name == "None"));
        let (decoded_data, decoded_has_alpha) =
            if !is_image_mask && has_image_smask && decoded_bpc == 8 {
                let mut resolver = self.resolver.take();
                let result = image_smask_alpha(
                    &decoded_data,
                    color_space_name,
                    decoded_width,
                    decoded_height,
                    xobject_dict.get("SMask"),
                    &mut resolver,
                );
                self.resolver = resolver;
                match result {
                    Some(rgba) => (rgba, true),
                    None => (decoded_data, decoded_has_alpha),
                }
            } else {
                (decoded_data, decoded_has_alpha)
            };

        // Create image data for the device
        let interpolate = matches!(
            xobject_dict.get("Interpolate"),
            Some(PDFObject::Boolean(true))
        );
        let image = super::ImageData {
            width: decoded_width,
            height: decoded_height,
            data: decoded_data,
            has_alpha: decoded_has_alpha,
            bits_per_component: decoded_bpc,
            is_mask: is_image_mask,
            mask_paint_one: image_mask_paint_one(xobject_dict),
            interpolate,
            mask_color: {
                let c = self.effective_fill_color();
                match c {
                    crate::rendering::Color::Gray(v) => {
                        let g = (v * 255.0).clamp(0.0, 255.0) as u8;
                        [g, g, g]
                    }
                    crate::rendering::Color::RGB(r, g, b) => [
                        (r * 255.0).clamp(0.0, 255.0) as u8,
                        (g * 255.0).clamp(0.0, 255.0) as u8,
                        (b * 255.0).clamp(0.0, 255.0) as u8,
                    ],
                    crate::rendering::Color::CMYK(_, _, _, _) => [0, 0, 0],
                }
            },
        };

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: PaintXObject: Drawing image '{}' ({}x{}, {} bpc, {} bytes)",
            xobject_name,
            width,
            height,
            bits_per_component,
            image_data.len()
        );

        // Use identity transform - the CTM already maps unit square to screen correctly
        // The content stream operators set up the CTM to position and scale the image
        let transform = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

        if has_image_smask {
            self.device.save_state();
            let _ = self.device.set_soft_mask(None);
            let result = self.device.draw_image(image, &transform);
            self.device.restore_state();
            result?;
        } else {
            self.device.draw_image(image, &transform)?;
        }

        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Image drawn successfully");

        Ok(())
    }

    fn paint_inline_image(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        let Some(PDFObject::Stream { dict, data }) = args.first() else {
            return Ok(());
        };

        let is_image_mask = dict
            .get("ImageMask")
            .and_then(PDFObject::as_boolean)
            .unwrap_or(false)
            || (!dict.contains_key("ColorSpace")
                && !filter_contains_any(dict.get("Filter"), &["JPXDecode"]));

        let width = match dict.get("Width").and_then(PDFObject::as_number) {
            Some(w) => w as u32,
            None => return Ok(()),
        };
        let height = match dict.get("Height").and_then(PDFObject::as_number) {
            Some(h) => h as u32,
            None => return Ok(()),
        };
        let bits_per_component = if is_image_mask {
            1
        } else {
            dict.get("BitsPerComponent")
                .and_then(PDFObject::as_number)
                .unwrap_or(8.0) as u8
        };

        let decoded =
            decode_inline_image_data(data, dict, width, height).unwrap_or_else(|| data.clone());

        let (decoded, bpc) = if bits_per_component == 1
            && !is_image_mask
            && should_expand_1bpc_sampled_image(dict.get("Filter"))
        {
            (expand_1bpc_image_to_8bpc(&decoded, width, height, 255), 8)
        } else {
            (decoded, bits_per_component)
        };

        let (decoded, bpc) = if !is_image_mask {
            let (expanded, color_expanded) = expand_indexed_cs(
                &decoded,
                dict.get("ColorSpace"),
                &mut None,
                bpc,
                width,
                height,
            );
            if color_expanded {
                (expanded, 8)
            } else {
                (decoded, bpc)
            }
        } else {
            (decoded, bpc)
        };

        let decoded = if !is_image_mask {
            if let Some(PDFObject::Array(decode_arr)) = dict.get("Decode") {
                apply_decode_inversion(&decoded, decode_arr, bpc)
            } else {
                decoded
            }
        } else {
            decoded
        };

        let decoded = normalize_pdfium_default_ccitt_image_mask(decoded, is_image_mask, dict);

        let image = super::ImageData {
            width,
            height,
            data: decoded,
            has_alpha: false,
            bits_per_component: bpc,
            is_mask: is_image_mask,
            mask_paint_one: image_mask_paint_one(dict),
            interpolate: matches!(dict.get("Interpolate"), Some(PDFObject::Boolean(true))),
            mask_color: {
                let c = self.effective_fill_color();
                match c {
                    crate::rendering::Color::Gray(v) => {
                        let g = (v * 255.0).clamp(0.0, 255.0) as u8;
                        [g, g, g]
                    }
                    crate::rendering::Color::RGB(r, g, b) => [
                        (r * 255.0).clamp(0.0, 255.0) as u8,
                        (g * 255.0).clamp(0.0, 255.0) as u8,
                        (b * 255.0).clamp(0.0, 255.0) as u8,
                    ],
                    crate::rendering::Color::CMYK(_, _, _, _) => [0, 0, 0],
                }
            },
        };

        let transform = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Inline image {}x{} is_mask={} mask_paint_one={} mask_color={:?} fill={:?} type3_shape_color={:?}",
            image.width,
            image.height,
            image.is_mask,
            image.mask_paint_one,
            image.mask_color,
            self.current_state().fill_color,
            self.type3_shape_color
        );
        self.device.draw_image(image, &transform)
    }

    /// Paint Normal appearance streams for page annotations (PDF §12.5.5).
    ///
    /// Skips annotations with the Hidden flag. Maps each Form XObject `/BBox`
    /// through `/Matrix` into the annotation `/Rect`, matching hayro's
    /// `interpret_page` annotation pass.
    /// Source: hayro/hayro-interpret/src/interpret/mod.rs (render_annotations).
    pub fn paint_annotation_appearances(&mut self, annotations: &[PDFObject]) -> PDFResult<()> {
        for annot in annotations {
            let PDFObject::Dictionary(annot_dict) = annot else {
                continue;
            };

            // F bit 1 (value 2) = Hidden — do not paint.
            let flags = annot_dict
                .get("F")
                .and_then(|o| match o {
                    PDFObject::Number(n) => Some(*n as u32),
                    _ => None,
                })
                .unwrap_or(0);
            if flags & 2 != 0 {
                continue;
            }

            let Some(rect) = parse_four_number_array(annot_dict.get("Rect")) else {
                continue;
            };
            let [llx, lly, urx, ury] = rect;
            let annot_w = (urx - llx).abs();
            let annot_h = (ury - lly).abs();
            if annot_w < 1e-9 || annot_h < 1e-9 {
                continue;
            }

            let Some((form_dict, form_data)) = self.resolve_annotation_appearance(annot_dict)
            else {
                continue;
            };

            let matrix = parse_six_number_array(form_dict.get("Matrix"))
                .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
            let bbox =
                parse_four_number_array(form_dict.get("BBox")).unwrap_or([0.0, 0.0, 1.0, 1.0]);

            // Transform BBox corners by Matrix; take axis-aligned bounds.
            let corners = [
                transform_point(matrix, bbox[0], bbox[1]),
                transform_point(matrix, bbox[2], bbox[1]),
                transform_point(matrix, bbox[0], bbox[3]),
                transform_point(matrix, bbox[2], bbox[3]),
            ];
            let mut x_min = corners[0].0;
            let mut x_max = corners[0].0;
            let mut y_min = corners[0].1;
            let mut y_max = corners[0].1;
            for &(x, y) in &corners[1..] {
                x_min = x_min.min(x);
                x_max = x_max.max(x);
                y_min = y_min.min(y);
                y_max = y_max.max(y);
            }
            let box_w = (x_max - x_min).abs().max(1e-9);
            let box_h = (y_max - y_min).abs().max(1e-9);

            // Matrix A: map transformed appearance box → annotation Rect.
            // sx/sy scale; tx/ty place lower-left at Rect lower-left.
            // PDF 32000-1 §12.5.5 steps 2–3.
            let sx = annot_w / box_w;
            let sy = annot_h / box_h;
            let tx = llx.min(urx) - x_min * sx;
            let ty = lly.min(ury) - y_min * sy;
            let a_matrix = [sx, 0.0, 0.0, sy, tx, ty];

            self.save()?;
            self.current_state_mut().concat_matrix(&a_matrix);
            self.device.concat_matrix(&a_matrix);
            self.push_root_ctm();

            let paint_result = self.paint_form_xobject("AnnotAP", &form_dict, &form_data);

            self.pop_root_ctm();
            let restore_result = self.restore();
            paint_result?;
            restore_result?;
        }
        Ok(())
    }

    /// Resolve `/AP /N` to a Form XObject stream (dict + bytes).
    fn resolve_annotation_appearance(
        &mut self,
        annot_dict: &HashMap<String, PDFObject>,
    ) -> Option<(HashMap<String, PDFObject>, Vec<u8>)> {
        let ap = annot_dict.get("AP")?;
        let mut resolver = self.resolver.take();
        let ap_resolved = match resolver.as_mut() {
            Some(r) => r.resolve_if_ref(ap).ok(),
            None => Some(std::sync::Arc::new(ap.clone())),
        };
        let Some(ap_obj) = ap_resolved else {
            self.resolver = resolver;
            return None;
        };

        let n_obj = match ap_obj.as_ref() {
            PDFObject::Dictionary(ap_dict) => {
                let n = ap_dict.get("N")?;
                match resolver.as_mut() {
                    Some(r) => r.resolve_if_ref(n).ok(),
                    None => Some(std::sync::Arc::new(n.clone())),
                }
            }
            // Rare: AP is itself a stream used as the Normal appearance.
            PDFObject::Stream { .. } => Some(ap_obj.clone()),
            _ => None,
        };
        let Some(n_obj) = n_obj else {
            self.resolver = resolver;
            return None;
        };

        // N may be a stream or a dict of named states (use first stream entry).
        let stream_obj = match n_obj.as_ref() {
            PDFObject::Stream { .. } => Some(n_obj.clone()),
            PDFObject::Dictionary(states) => {
                let mut found = None;
                for v in states.values() {
                    let resolved = match resolver.as_mut() {
                        Some(r) => r.resolve_if_ref(v).ok(),
                        None => Some(std::sync::Arc::new(v.clone())),
                    };
                    if let Some(s) = resolved {
                        if matches!(s.as_ref(), PDFObject::Stream { .. }) {
                            found = Some(s);
                            break;
                        }
                    }
                }
                found
            }
            _ => None,
        };
        self.resolver = resolver;

        let stream_obj = stream_obj?;
        match stream_obj.as_ref() {
            PDFObject::Stream { dict, data } => Some((dict.clone(), data.clone())),
            _ => None,
        }
    }

    fn paint_form_xobject(
        &mut self,
        xobject_name: &str,
        form_dict: &HashMap<String, PDFObject>,
        form_data: &[u8],
    ) -> PDFResult<()> {
        // Decode the form stream if it uses filters.
        let decoded_data = if let Some(filter) = form_dict.get("Filter") {
            crate::core::decode::apply_filters(form_data, filter)
                .unwrap_or_else(|_| form_data.to_vec())
        } else {
            form_data.to_vec()
        };

        // Form XObjects can define their own resource dictionary; otherwise they
        // inherit the current one.
        let form_resources = form_dict.get("Resources").cloned();

        let matrix = parse_six_number_array(form_dict.get("Matrix"))
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let bbox = parse_four_number_array(form_dict.get("BBox"));
        let is_transparency_group = self.soft_mask_render_depth == 0
            && form_has_transparency_group(form_dict, &mut self.resolver);

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: PaintXObject: Entering form '{}' ({} bytes), matrix={:?}, bbox={:?}",
            xobject_name,
            decoded_data.len(),
            matrix,
            bbox
        );

        let group_opacity = self.current_state().fill_alpha;
        if is_transparency_group {
            self.device.save_state();
            let _ = self.device.set_soft_mask(None);
            self.device.set_opacity(1.0);
            self.device.begin_offscreen()?;
        }

        self.save()?;
        let previous_resources = self.resources.clone();

        // Keep restore/resource rollback deterministic even on early errors.
        let result = (|| -> PDFResult<()> {
            if is_transparency_group {
                self.current_state_mut().fill_alpha = 1.0;
                self.current_state_mut().stroke_alpha = 1.0;
            }

            // Apply form matrix to map the form space into the current CTM.
            self.current_state_mut().concat_matrix(&matrix);
            self.device.concat_matrix(&matrix);
            self.push_root_ctm();

            // Clip to the form's BBox when provided.
            let form_result = (|| -> PDFResult<()> {
                if let Some([x0, y0, x1, y1]) = bbox {
                    self.device.begin_path();
                    self.device.move_to(x0, y0);
                    self.device.line_to(x1, y0);
                    self.device.line_to(x1, y1);
                    self.device.line_to(x0, y1);
                    self.device.close_path();
                    self.device.clip_path(FillRule::NonZero)?;
                }

                self.resources = merge_pattern_resources(
                    form_resources,
                    previous_resources.clone(),
                    &mut self.resolver,
                );

                self.process_content_stream_bytes(decoded_data)
            })();
            self.pop_root_ctm();
            form_result
        })();

        self.resources = previous_resources;
        let restore_result = self.restore();

        if is_transparency_group {
            self.device.restore_state();
            self.device.set_opacity(group_opacity);
            self.device.end_offscreen_composite(group_opacity)?;
        }

        if let Err(e) = result {
            return Err(e);
        }
        restore_result
    }

    fn process_content_stream_bytes(&mut self, decoded_data: Vec<u8>) -> PDFResult<()> {
        let stream = Box::new(crate::core::Stream::from_bytes(decoded_data))
            as Box<dyn crate::core::BaseStream>;
        let lexer = crate::core::Lexer::new(stream)?;
        let parser = crate::core::Parser::new(lexer)?;
        let mut evaluator = crate::core::ContentStreamEvaluator::new(parser);

        while let Some(op) = evaluator.read_operation()? {
            self.process_operation(&op)?;
        }

        Ok(())
    }
}

/// Helper function to extract a number from a PDFObject.
fn extract_number(args: &[crate::core::parser::PDFObject], index: usize) -> PDFResult<f64> {
    if index >= args.len() {
        return Err(PDFError::content_stream_error(
            "Not enough arguments".to_string(),
        ));
    }

    match &args[index] {
        crate::core::parser::PDFObject::Number(n) => Ok(*n),
        _ => Err(PDFError::content_stream_error(
            "Expected number argument".to_string(),
        )),
    }
}

fn parse_shading(
    dict: &HashMap<String, PDFObject>,
    stream_data: Option<&[u8]>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<super::shading::Shading> {
    use super::shading::ShadingKind;

    let shading_type = dict
        .get("ShadingType")
        .and_then(|v| v.as_number())
        .unwrap_or(0.0) as i32;

    let color_space = dict
        .get("ColorSpace")
        .and_then(|cs| parse_shading_color_space(cs, resolver))
        .unwrap_or(super::shading::ShadingColorSpace::DeviceRGB);
    let n_components = color_space.n_components();

    let function = dict
        .get("Function")
        .map(|f| parse_shading_function(f, resolver));

    let bbox = parse_four_number_array(dict.get("BBox"));
    let matrix =
        parse_six_number_array(dict.get("Matrix")).unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    let kind = match shading_type {
        1 => {
            let function = function?;
            let domain =
                parse_four_number_array(dict.get("Domain")).unwrap_or([0.0, 1.0, 0.0, 1.0]);
            ShadingKind::FunctionBased { domain, function }
        }
        2 | 3 => {
            let function = function?;
            let coords = number_array(dict.get("Coords"))?;
            let domain_arr = number_array(dict.get("Domain")).unwrap_or_else(|| vec![0.0, 1.0]);
            let domain = [
                domain_arr.first().copied().unwrap_or(0.0),
                domain_arr.get(1).copied().unwrap_or(1.0),
            ];
            let extend = bool_pair_array(dict.get("Extend")).unwrap_or([false, false]);

            if shading_type == 2 {
                if coords.len() < 4 {
                    return None;
                }
                ShadingKind::Axial {
                    x0: coords[0],
                    y0: coords[1],
                    x1: coords[2],
                    y1: coords[3],
                    t0: domain[0],
                    t1: domain[1],
                    function,
                    extend_start: extend[0],
                    extend_end: extend[1],
                }
            } else {
                if coords.len() < 6 {
                    return None;
                }
                ShadingKind::Radial {
                    x0: coords[0],
                    y0: coords[1],
                    r0: coords[2],
                    x1: coords[3],
                    y1: coords[4],
                    r1: coords[5],
                    t0: domain[0],
                    t1: domain[1],
                    function,
                    extend_start: extend[0],
                    extend_end: extend[1],
                }
            }
        }
        4 => {
            let data = stream_data?;
            let triangles = parse_type4_triangle_mesh(dict, data, function.as_ref().is_some())?;
            ShadingKind::TriangleMesh {
                triangles,
                function,
            }
        }
        6 | 7 => {
            let data = stream_data?;
            let triangles =
                parse_patch_triangle_mesh(dict, data, shading_type, function.as_ref().is_some())?;
            ShadingKind::TriangleMesh {
                triangles,
                function,
            }
        }
        _ => return None,
    };

    Some(super::shading::Shading {
        kind,
        n_components,
        color_space,
        bbox,
        matrix,
        background: number_array(dict.get("Background")),
        use_root_ctm: false,
    })
}

fn parse_shading_color_space(
    obj: &PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<super::shading::ShadingColorSpace> {
    use super::shading::ShadingColorSpace;

    let resolved = resolve_object_clone(obj.clone(), resolver).unwrap_or_else(|| obj.clone());
    match resolved {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => Some(ShadingColorSpace::DeviceGray),
            "DeviceRGB" | "RGB" => Some(ShadingColorSpace::DeviceRGB),
            "DeviceCMYK" | "CMYK" => Some(ShadingColorSpace::DeviceCMYK),
            _ => None,
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "CalRGB" && arr.len() >= 2 => {
                let dict = match resolve_object_clone(arr[1].as_ref().clone(), resolver)
                    .unwrap_or_else(|| arr[1].as_ref().clone())
                {
                    PDFObject::Dictionary(dict) => dict,
                    _ => return None,
                };
                Some(ShadingColorSpace::CalRGB {
                    white_point: parse_three_number_array(dict.get("WhitePoint"))
                        .unwrap_or([1.0, 1.0, 1.0]),
                    black_point: parse_three_number_array(dict.get("BlackPoint"))
                        .unwrap_or([0.0, 0.0, 0.0]),
                    gamma: parse_three_number_array(dict.get("Gamma")).unwrap_or([1.0, 1.0, 1.0]),
                    matrix: parse_nine_number_array(dict.get("Matrix"))
                        .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
                })
            }
            Some(PDFObject::Name(name)) if name == "Separation" && arr.len() >= 4 => {
                let base = parse_shading_color_space(arr[2].as_ref(), resolver)?;
                let tint_function = parse_shading_function(arr[3].as_ref(), resolver);
                Some(ShadingColorSpace::Alternate {
                    n_components: 1,
                    base: Box::new(base),
                    tint_function,
                })
            }
            Some(PDFObject::Name(name)) if name == "DeviceN" && arr.len() >= 4 => {
                let n_components = match resolve_object_clone(arr[1].as_ref().clone(), resolver)
                    .unwrap_or_else(|| arr[1].as_ref().clone())
                {
                    PDFObject::Array(names) => names.len(),
                    _ => return None,
                };
                if n_components == 0 {
                    return None;
                }
                let base = parse_shading_color_space(arr[2].as_ref(), resolver)?;
                let tint_function = parse_shading_function(arr[3].as_ref(), resolver);
                Some(ShadingColorSpace::Alternate {
                    n_components,
                    base: Box::new(base),
                    tint_function,
                })
            }
            // ICCBased shadings: map by component count. Full ICC transform is
            // not required for the common Gray/RGB/CMYK profile cases used by
            // Type3 soft-mask + pattern fixtures (hayro/mutool agree on that path).
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                let profile = resolve_object_clone(arr[1].as_ref().clone(), resolver)
                    .unwrap_or_else(|| arr[1].as_ref().clone());
                let n = match profile {
                    PDFObject::Stream { dict, .. } => dict
                        .get("N")
                        .and_then(|v| v.as_number())
                        .map(|n| n as i32)
                        .unwrap_or(3),
                    PDFObject::Dictionary(dict) => dict
                        .get("N")
                        .and_then(|v| v.as_number())
                        .map(|n| n as i32)
                        .unwrap_or(3),
                    _ => 3,
                };
                match n {
                    1 => Some(ShadingColorSpace::DeviceGray),
                    4 => Some(ShadingColorSpace::DeviceCMYK),
                    _ => Some(ShadingColorSpace::DeviceRGB),
                }
            }
            _ => None,
        },
        _ => None,
    }
}

fn parse_type4_triangle_mesh(
    dict: &HashMap<String, PDFObject>,
    data: &[u8],
    has_function: bool,
) -> Option<Vec<super::shading::ShadingTriangle>> {
    use super::shading::{ShadingTriangle, ShadingVertex};

    let bits_per_coordinate = dict.get("BitsPerCoordinate").and_then(|v| v.as_number())? as usize;
    let bits_per_component = dict.get("BitsPerComponent").and_then(|v| v.as_number())? as usize;
    let bits_per_flag = dict.get("BitsPerFlag").and_then(|v| v.as_number())? as usize;
    if bits_per_coordinate == 0
        || bits_per_coordinate > 32
        || bits_per_component == 0
        || bits_per_component > 32
        || bits_per_flag == 0
        || bits_per_flag > 8
    {
        return None;
    }

    let decode = number_array(dict.get("Decode"))?;
    if decode.len() < 6 {
        return None;
    }
    let x_min = decode[0];
    let x_max = decode[1];
    let y_min = decode[2];
    let y_max = decode[3];
    let component_decode = &decode[4..];
    let component_count = if has_function {
        1
    } else {
        component_decode.len() / 2
    };
    if component_count == 0 || component_decode.len() < component_count * 2 {
        return None;
    }

    let decoded = dict
        .get("Filter")
        .and_then(|filter| crate::core::decode::apply_filters(data, filter).ok())
        .unwrap_or_else(|| data.to_vec());
    let mut reader = MeshBitReader::new(&decoded);
    let coord_max = sample_max(bits_per_coordinate)?;
    let component_max = sample_max(bits_per_component)?;

    let mut a: Option<ShadingVertex> = None;
    let mut b: Option<ShadingVertex> = None;
    let mut c: Option<ShadingVertex> = None;
    let mut triangles = Vec::new();

    while reader.remaining_bits() >= bits_per_flag + bits_per_coordinate * 2 {
        let flag = reader.read(bits_per_flag)?;
        if flag > 2 {
            return None;
        }
        let x = interpolate_number(
            reader.read(bits_per_coordinate)? as f64,
            0.0,
            coord_max,
            x_min,
            x_max,
        );
        let y = interpolate_number(
            reader.read(bits_per_coordinate)? as f64,
            0.0,
            coord_max,
            y_min,
            y_max,
        );
        let mut components = Vec::with_capacity(component_count);
        for i in 0..component_count {
            let d0 = component_decode[i * 2];
            let d1 = component_decode[i * 2 + 1];
            components.push(interpolate_number(
                reader.read(bits_per_component)? as f64,
                0.0,
                component_max,
                d0,
                d1,
            ));
        }
        reader.align_byte();

        let vertex = ShadingVertex { x, y, components };
        match flag {
            0 => {
                let second = read_type4_vertex(
                    &mut reader,
                    bits_per_flag,
                    bits_per_coordinate,
                    bits_per_component,
                    component_count,
                    coord_max,
                    component_max,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                    component_decode,
                )?;
                let third = read_type4_vertex(
                    &mut reader,
                    bits_per_flag,
                    bits_per_coordinate,
                    bits_per_component,
                    component_count,
                    coord_max,
                    component_max,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                    component_decode,
                )?;
                a = Some(vertex);
                b = Some(second);
                c = Some(third);
            }
            1 => {
                a = b.clone();
                b = c.clone();
                c = Some(vertex);
            }
            2 => {
                b = c.clone();
                c = Some(vertex);
            }
            _ => unreachable!(),
        }

        let (p0, p1, p2) = (a.clone()?, b.clone()?, c.clone()?);
        if points_nearly_same(&p0, &p1) || points_nearly_same(&p1, &p2) {
            continue;
        }
        triangles.push(ShadingTriangle { p0, p1, p2 });
    }

    Some(triangles)
}

#[allow(clippy::too_many_arguments)]
fn read_type4_vertex(
    reader: &mut MeshBitReader<'_>,
    bits_per_flag: usize,
    bits_per_coordinate: usize,
    bits_per_component: usize,
    component_count: usize,
    coord_max: f64,
    component_max: f64,
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
    component_decode: &[f64],
) -> Option<super::shading::ShadingVertex> {
    let _flag = reader.read(bits_per_flag)?;
    let x = interpolate_number(
        reader.read(bits_per_coordinate)? as f64,
        0.0,
        coord_max,
        x_min,
        x_max,
    );
    let y = interpolate_number(
        reader.read(bits_per_coordinate)? as f64,
        0.0,
        coord_max,
        y_min,
        y_max,
    );
    let mut components = Vec::with_capacity(component_count);
    for i in 0..component_count {
        components.push(interpolate_number(
            reader.read(bits_per_component)? as f64,
            0.0,
            component_max,
            component_decode[i * 2],
            component_decode[i * 2 + 1],
        ));
    }
    reader.align_byte();
    Some(super::shading::ShadingVertex { x, y, components })
}

fn points_nearly_same(
    a: &super::shading::ShadingVertex,
    b: &super::shading::ShadingVertex,
) -> bool {
    (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
}

fn parse_patch_triangle_mesh(
    dict: &HashMap<String, PDFObject>,
    data: &[u8],
    shading_type: i32,
    has_function: bool,
) -> Option<Vec<super::shading::ShadingTriangle>> {
    let bits_per_coordinate = dict.get("BitsPerCoordinate").and_then(|v| v.as_number())? as usize;
    let bits_per_component = dict.get("BitsPerComponent").and_then(|v| v.as_number())? as usize;
    let bits_per_flag = dict.get("BitsPerFlag").and_then(|v| v.as_number())? as usize;
    if bits_per_coordinate == 0
        || bits_per_coordinate > 32
        || bits_per_component == 0
        || bits_per_component > 32
        || bits_per_flag == 0
        || bits_per_flag > 8
    {
        return None;
    }

    let decode = number_array(dict.get("Decode"))?;
    if decode.len() < 6 {
        return None;
    }
    let x_min = decode[0];
    let x_max = decode[1];
    let y_min = decode[2];
    let y_max = decode[3];
    let component_decode = &decode[4..];
    let component_count = if has_function {
        1
    } else {
        component_decode.len() / 2
    };
    if component_count == 0 || component_decode.len() < component_count * 2 {
        return None;
    }

    let decoded = dict
        .get("Filter")
        .and_then(|filter| crate::core::decode::apply_filters(data, filter).ok())
        .unwrap_or_else(|| data.to_vec());
    let mut reader = MeshBitReader::new(&decoded);
    let coord_max = sample_max(bits_per_coordinate)?;
    let component_max = sample_max(bits_per_component)?;
    let control_points_count = if shading_type == 7 { 16 } else { 12 };
    let mut prev_points: Option<Vec<PatchPoint>> = None;
    let mut prev_colors: Option<[Vec<f64>; 4]> = None;
    let mut triangles = Vec::new();

    while reader.remaining_bits() >= bits_per_flag {
        let flag = reader.read(bits_per_flag)?;
        if flag > 3 {
            break;
        }

        let mut control_points = vec![PatchPoint::default(); 16];
        let mut colors: [Vec<f64>; 4] = Default::default();

        if flag == 0 {
            for point in control_points.iter_mut().take(control_points_count) {
                *point = read_patch_point(
                    &mut reader,
                    bits_per_coordinate,
                    coord_max,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                )?;
            }
            for color in &mut colors {
                *color = read_patch_color(
                    &mut reader,
                    bits_per_component,
                    component_count,
                    component_max,
                    component_decode,
                )?;
            }
        } else {
            let prev_points_ref = prev_points.as_ref()?;
            let prev_colors_ref = prev_colors.as_ref()?;
            copy_patch_control_points(flag, prev_points_ref, &mut control_points);
            match flag {
                1 => {
                    colors[0] = prev_colors_ref[1].clone();
                    colors[1] = prev_colors_ref[2].clone();
                }
                2 => {
                    colors[0] = prev_colors_ref[2].clone();
                    colors[1] = prev_colors_ref[3].clone();
                }
                3 => {
                    colors[0] = prev_colors_ref[3].clone();
                    colors[1] = prev_colors_ref[0].clone();
                }
                _ => unreachable!(),
            }
            for point in control_points.iter_mut().take(control_points_count).skip(4) {
                *point = read_patch_point(
                    &mut reader,
                    bits_per_coordinate,
                    coord_max,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                )?;
            }
            colors[2] = read_patch_color(
                &mut reader,
                bits_per_component,
                component_count,
                component_max,
                component_decode,
            )?;
            colors[3] = read_patch_color(
                &mut reader,
                bits_per_component,
                component_count,
                component_max,
                component_decode,
            )?;
        }

        prev_points = Some(control_points.clone());
        prev_colors = Some(colors.clone());
        if shading_type == 7 {
            append_tensor_patch_triangles(&control_points, &colors, &mut triangles);
        } else {
            append_coons_patch_triangles(&control_points, &colors, &mut triangles);
        }
    }

    Some(triangles)
}

#[allow(clippy::too_many_arguments)]
fn read_patch_point(
    reader: &mut MeshBitReader<'_>,
    bits_per_coordinate: usize,
    coord_max: f64,
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
) -> Option<PatchPoint> {
    Some(PatchPoint {
        x: interpolate_number(
            reader.read(bits_per_coordinate)? as f64,
            0.0,
            coord_max,
            x_min,
            x_max,
        ),
        y: interpolate_number(
            reader.read(bits_per_coordinate)? as f64,
            0.0,
            coord_max,
            y_min,
            y_max,
        ),
    })
}

fn read_patch_color(
    reader: &mut MeshBitReader<'_>,
    bits_per_component: usize,
    component_count: usize,
    component_max: f64,
    component_decode: &[f64],
) -> Option<Vec<f64>> {
    let mut color = Vec::with_capacity(component_count);
    for i in 0..component_count {
        color.push(interpolate_number(
            reader.read(bits_per_component)? as f64,
            0.0,
            component_max,
            component_decode[i * 2],
            component_decode[i * 2 + 1],
        ));
    }
    Some(color)
}

fn copy_patch_control_points(flag: u32, prev: &[PatchPoint], current: &mut [PatchPoint]) {
    match flag {
        1 => {
            current[0] = prev[3];
            current[1] = prev[4];
            current[2] = prev[5];
            current[3] = prev[6];
        }
        2 => {
            current[0] = prev[6];
            current[1] = prev[7];
            current[2] = prev[8];
            current[3] = prev[9];
        }
        3 => {
            current[0] = prev[9];
            current[1] = prev[10];
            current[2] = prev[11];
            current[3] = prev[0];
        }
        _ => {}
    }
}

fn append_coons_patch_triangles(
    control_points: &[PatchPoint],
    colors: &[Vec<f64>; 4],
    triangles: &mut Vec<super::shading::ShadingTriangle>,
) {
    generate_patch_triangles(
        |u, v| map_coons_patch(control_points, u, v),
        |u, v| interpolate_patch_color(colors, u, v),
        triangles,
    );
}

fn append_tensor_patch_triangles(
    control_points: &[PatchPoint],
    colors: &[Vec<f64>; 4],
    triangles: &mut Vec<super::shading::ShadingTriangle>,
) {
    generate_patch_triangles(
        |u, v| map_tensor_patch(control_points, u, v),
        |u, v| interpolate_patch_color(colors, u, v),
        triangles,
    );
}

fn generate_patch_triangles<M, C>(
    map_coordinate: M,
    interpolate_color: C,
    triangles: &mut Vec<super::shading::ShadingTriangle>,
) where
    M: Fn(f64, f64) -> PatchPoint,
    C: Fn(f64, f64) -> Vec<f64>,
{
    use super::shading::{ShadingTriangle, ShadingVertex};
    const GRID_SIZE: usize = 20;

    for i in 0..(GRID_SIZE - 1) {
        for j in 0..(GRID_SIZE - 1) {
            let u0 = i as f64 / (GRID_SIZE - 1) as f64;
            let u1 = (i + 1) as f64 / (GRID_SIZE - 1) as f64;
            let v0 = j as f64 / (GRID_SIZE - 1) as f64;
            let v1 = (j + 1) as f64 / (GRID_SIZE - 1) as f64;

            let p00 = map_coordinate(u0, v0);
            let p10 = map_coordinate(u1, v0);
            let p01 = map_coordinate(u0, v1);
            let p11 = map_coordinate(u1, v1);

            let v00 = ShadingVertex {
                x: p00.x,
                y: p00.y,
                components: interpolate_color(u0, v0),
            };
            let v10 = ShadingVertex {
                x: p10.x,
                y: p10.y,
                components: interpolate_color(u1, v0),
            };
            let v01 = ShadingVertex {
                x: p01.x,
                y: p01.y,
                components: interpolate_color(u0, v1),
            };
            let v11 = ShadingVertex {
                x: p11.x,
                y: p11.y,
                components: interpolate_color(u1, v1),
            };

            triangles.push(ShadingTriangle {
                p0: v00.clone(),
                p1: v10.clone(),
                p2: v01.clone(),
            });
            triangles.push(ShadingTriangle {
                p0: v10,
                p1: v11,
                p2: v01,
            });
        }
    }
}

fn map_coons_patch(cp: &[PatchPoint], u: f64, v: f64) -> PatchPoint {
    let c1 = cubic_eval(cp[0], cp[11], cp[10], cp[9], u);
    let c2 = cubic_eval(cp[3], cp[4], cp[5], cp[6], u);
    let d1 = cubic_eval(cp[0], cp[1], cp[2], cp[3], v);
    let d2 = cubic_eval(cp[9], cp[8], cp[7], cp[6], v);
    let c10 = cubic_eval(cp[0], cp[11], cp[10], cp[9], 0.0);
    let c11 = cubic_eval(cp[0], cp[11], cp[10], cp[9], 1.0);
    let c20 = cubic_eval(cp[3], cp[4], cp[5], cp[6], 0.0);
    let c21 = cubic_eval(cp[3], cp[4], cp[5], cp[6], 1.0);

    let sc = PatchPoint {
        x: (1.0 - v) * c1.x + v * c2.x,
        y: (1.0 - v) * c1.y + v * c2.y,
    };
    let sd = PatchPoint {
        x: (1.0 - u) * d1.x + u * d2.x,
        y: (1.0 - u) * d1.y + u * d2.y,
    };
    let sb = PatchPoint {
        x: (1.0 - v) * ((1.0 - u) * c10.x + u * c11.x) + v * ((1.0 - u) * c20.x + u * c21.x),
        y: (1.0 - v) * ((1.0 - u) * c10.y + u * c11.y) + v * ((1.0 - u) * c20.y + u * c21.y),
    };

    PatchPoint {
        x: sc.x + sd.x - sb.x,
        y: sc.y + sd.y - sb.y,
    }
}

fn map_tensor_patch(cp: &[PatchPoint], u: f64, v: f64) -> PatchPoint {
    fn idx(i: usize, j: usize) -> usize {
        match (i, j) {
            (0, 0) => 0,
            (0, 1) => 1,
            (0, 2) => 2,
            (0, 3) => 3,
            (1, 0) => 11,
            (1, 1) => 12,
            (1, 2) => 13,
            (1, 3) => 4,
            (2, 0) => 10,
            (2, 1) => 15,
            (2, 2) => 14,
            (2, 3) => 5,
            (3, 0) => 9,
            (3, 1) => 8,
            (3, 2) => 7,
            (3, 3) => 6,
            _ => 0,
        }
    }

    let mut x = 0.0;
    let mut y = 0.0;
    for i in 0..4 {
        for j in 0..4 {
            let basis = bernstein(i, u) * bernstein(j, v);
            let p = cp[idx(i, j)];
            x += p.x * basis;
            y += p.y * basis;
        }
    }
    PatchPoint { x, y }
}

fn interpolate_patch_color(colors: &[Vec<f64>; 4], u: f64, v: f64) -> Vec<f64> {
    let n = colors.iter().map(Vec::len).min().unwrap_or(0);
    (0..n)
        .map(|i| {
            (1.0 - u) * (1.0 - v) * colors[0][i]
                + u * (1.0 - v) * colors[3][i]
                + u * v * colors[2][i]
                + (1.0 - u) * v * colors[1][i]
        })
        .collect()
}

fn cubic_eval(
    p0: PatchPoint,
    p1: PatchPoint,
    p2: PatchPoint,
    p3: PatchPoint,
    t: f64,
) -> PatchPoint {
    let mt = 1.0 - t;
    let b0 = mt.powi(3);
    let b1 = 3.0 * t * mt.powi(2);
    let b2 = 3.0 * t.powi(2) * mt;
    let b3 = t.powi(3);
    PatchPoint {
        x: b0 * p0.x + b1 * p1.x + b2 * p2.x + b3 * p3.x,
        y: b0 * p0.y + b1 * p1.y + b2 * p2.y + b3 * p3.y,
    }
}

fn bernstein(i: usize, t: f64) -> f64 {
    match i {
        0 => (1.0 - t).powi(3),
        1 => 3.0 * t * (1.0 - t).powi(2),
        2 => 3.0 * t.powi(2) * (1.0 - t),
        3 => t.powi(3),
        _ => 0.0,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PatchPoint {
    x: f64,
    y: f64,
}

fn sample_max(bits: usize) -> Option<f64> {
    if bits == 0 || bits > 32 {
        None
    } else {
        Some((1u64.checked_shl(bits as u32)? - 1) as f64)
    }
}

fn interpolate_number(x: f64, xmin: f64, xmax: f64, ymin: f64, ymax: f64) -> f64 {
    if (xmax - xmin).abs() < f64::EPSILON {
        ymin
    } else {
        ymin + (x - xmin) * (ymax - ymin) / (xmax - xmin)
    }
}

struct MeshBitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> MeshBitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn remaining_bits(&self) -> usize {
        self.data
            .len()
            .saturating_mul(8)
            .saturating_sub(self.bit_pos)
    }

    fn read(&mut self, bits: usize) -> Option<u32> {
        if bits > 32 || self.remaining_bits() < bits {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..bits {
            let byte = *self.data.get(self.bit_pos / 8)?;
            let bit = 7 - (self.bit_pos % 8);
            value = (value << 1) | (((byte >> bit) & 1) as u32);
            self.bit_pos += 1;
        }
        Some(value)
    }

    fn align_byte(&mut self) {
        let rem = self.bit_pos % 8;
        if rem != 0 {
            self.bit_pos += 8 - rem;
        }
    }
}

fn bool_pair_array(object: Option<&PDFObject>) -> Option<[bool; 2]> {
    let PDFObject::Array(arr) = object? else {
        return None;
    };

    let mut out = [true; 2];
    for (index, slot) in out.iter_mut().enumerate() {
        let Some(item) = arr.get(index) else {
            break;
        };
        let PDFObject::Boolean(value) = item.as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

/// Parse a shading function from a PDFObject (Type 2 exponential or Type 4 PostScript).
fn parse_shading_function(
    obj: &PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> super::shading::ShadingFunction {
    let resolved = resolve_object_clone(obj.clone(), resolver).unwrap_or(PDFObject::Null);
    match &resolved {
        PDFObject::Dictionary(dict) => {
            let ft = dict
                .get("FunctionType")
                .and_then(|v| v.as_number())
                .unwrap_or(2.0) as i32;
            if ft == 2 {
                let c0 = number_array(dict.get("C0")).unwrap_or_default();
                let c1 = number_array(dict.get("C1")).unwrap_or_default();
                let n = dict.get("N").and_then(|v| v.as_number()).unwrap_or(1.0);
                super::shading::ShadingFunction::Exponential { c0, c1, n }
            } else if ft == 3 {
                let domain_arr = number_array(dict.get("Domain")).unwrap_or_else(|| vec![0.0, 1.0]);
                let domain = [
                    domain_arr.first().copied().unwrap_or(0.0),
                    domain_arr.get(1).copied().unwrap_or(1.0),
                ];
                let bounds = number_array(dict.get("Bounds")).unwrap_or_default();
                let encode = number_array(dict.get("Encode")).unwrap_or_default();
                let functions: Vec<super::shading::ShadingFunction> = match dict.get("Functions") {
                    Some(PDFObject::Array(arr)) => arr
                        .iter()
                        .map(|f| parse_shading_function(f, resolver))
                        .collect(),
                    _ => Vec::new(),
                };
                super::shading::ShadingFunction::Stitching {
                    functions,
                    bounds,
                    encode,
                    domain,
                }
            } else {
                super::shading::ShadingFunction::Exponential {
                    c0: vec![0.0],
                    c1: vec![1.0],
                    n: 1.0,
                }
            }
        }
        PDFObject::Stream { dict, data } => {
            let ft = dict
                .get("FunctionType")
                .and_then(|v| v.as_number())
                .unwrap_or(4.0) as i32;
            if ft == 0 {
                parse_sampled_function(dict, data)
            } else {
                super::shading::ShadingFunction::PostScript {
                    program: data.clone(),
                    domain: number_array(dict.get("Domain")).unwrap_or_default(),
                    range: number_array(dict.get("Range")).unwrap_or_default(),
                }
            }
        }
        _ => super::shading::ShadingFunction::Exponential {
            c0: vec![0.0],
            c1: vec![1.0],
            n: 1.0,
        },
    }
}

fn parse_sampled_function(
    dict: &HashMap<String, PDFObject>,
    data: &[u8],
) -> super::shading::ShadingFunction {
    let domain = number_array(dict.get("Domain")).unwrap_or_default();
    let range = number_array(dict.get("Range")).unwrap_or_default();
    let size: Vec<usize> = number_array(dict.get("Size"))
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.max(1.0) as usize)
        .collect();
    let bits_per_sample = dict
        .get("BitsPerSample")
        .and_then(|v| v.as_number())
        .unwrap_or(8.0) as usize;

    let input_size = domain.len() / 2;
    let output_size = range.len() / 2;
    if input_size == 0 || output_size == 0 || size.len() < input_size {
        return super::shading::ShadingFunction::Exponential {
            c0: vec![0.0],
            c1: vec![1.0],
            n: 1.0,
        };
    }

    let encode = number_array(dict.get("Encode")).unwrap_or_else(|| {
        size.iter()
            .take(input_size)
            .flat_map(|s| [0.0, s.saturating_sub(1) as f64])
            .collect()
    });
    let decode = number_array(dict.get("Decode")).unwrap_or_else(|| range.clone());
    let decoded = dict
        .get("Filter")
        .and_then(|filter| crate::core::decode::apply_filters(data, filter).ok())
        .unwrap_or_else(|| data.to_vec());
    let sample_count = size
        .iter()
        .take(input_size)
        .fold(output_size, |acc, s| acc.saturating_mul(*s));
    let samples = read_sample_array(&decoded, sample_count, bits_per_sample);

    super::shading::ShadingFunction::Sampled {
        domain,
        range,
        size,
        bits_per_sample,
        encode,
        decode,
        samples,
    }
}

fn read_sample_array(data: &[u8], sample_count: usize, bits_per_sample: usize) -> Vec<f64> {
    if sample_count == 0 || bits_per_sample == 0 || bits_per_sample > 32 {
        return Vec::new();
    }

    let sample_max = 2f64.powi(bits_per_sample as i32) - 1.0;
    let mut samples = Vec::with_capacity(sample_count);
    let mut code_buf = 0u64;
    let mut code_size = 0usize;
    let mut byte_index = 0usize;

    for _ in 0..sample_count {
        while code_size < bits_per_sample {
            code_buf = (code_buf << 8) | data.get(byte_index).copied().unwrap_or(0) as u64;
            byte_index += 1;
            code_size += 8;
        }

        code_size -= bits_per_sample;
        let value = (code_buf >> code_size) & ((1u64 << bits_per_sample) - 1);
        samples.push(value as f64 / sample_max);
        code_buf &= if code_size == 0 {
            0
        } else {
            (1u64 << code_size) - 1
        };
    }

    samples
}

/// Map an arbitrary BaseFont name to a Base-14 style key for bundled substitutes.
///
/// Mirrors hayro's `FallbackFontQuery::pick_standard_font` (serif/sans/mono ×
/// bold × italic). Critical for unembedded Type1s like Garamond-Bold → Times-Bold
/// rather than always falling back to regular serif.
fn pick_fallback_standard_font_key(base_font: &str) -> &'static str {
    let lower = base_font.to_ascii_lowercase();
    let bold = lower.contains("bold")
        || lower.contains("black")
        || lower.contains("heavy")
        || lower.contains("medi")
        || lower.contains("demibold")
        || lower.contains("semibold");
    let italic = lower.contains("italic")
        || lower.contains("oblique")
        || lower.contains("slant")
        || lower.contains("kursiv");
    let mono = lower.contains("mono")
        || lower.contains("courier")
        || lower.contains("typewriter")
        || lower.contains("console");
    let sans = lower.contains("sans")
        || lower.contains("helvetica")
        || lower.contains("arial")
        || lower.contains("gothic")
        || lower.contains("ui");

    if mono {
        match (bold, italic) {
            (true, true) => "Courier-BoldOblique",
            (true, false) => "Courier-Bold",
            (false, true) => "Courier-Oblique",
            (false, false) => "Courier",
        }
    } else if sans {
        match (bold, italic) {
            (true, true) => "Helvetica-BoldOblique",
            (true, false) => "Helvetica-Bold",
            (false, true) => "Helvetica-Oblique",
            (false, false) => "Helvetica",
        }
    } else {
        // Default serif family (Times / Garamond / Book / Roman / …).
        match (bold, italic) {
            (true, true) => "Times-BoldItalic",
            (true, false) => "Times-Bold",
            (false, true) => "Times-Italic",
            (false, false) => "Times-Roman",
        }
    }
}

fn bundled_standard_font_data(base_font: &str) -> Option<Vec<u8>> {
    // Unknown names go through style-aware Base-14 mapping first so
    // "Garamond-Bold" resolves to Times-Bold, not Times-Roman.
    let key = match base_font {
        "Helvetica"
        | "ArialMT"
        | "Arial"
        | "LiberationSans"
        | "NimbusSanL-Regu"
        | "Helvetica-Bold"
        | "Arial-BoldMT"
        | "Arial-Bold"
        | "Arial,Bold"
        | "LiberationSans-Bold"
        | "NimbusSanL-Bold"
        | "Helvetica-Oblique"
        | "Arial-ItalicMT"
        | "Arial-Italic"
        | "Helvetica-Italic"
        | "LiberationSans-Italic"
        | "NimbusSanL-ReguItal"
        | "Helvetica-BoldOblique"
        | "Arial-BoldItalicMT"
        | "Helvetica-BoldItalic"
        | "LiberationSans-BoldItalic"
        | "NimbusSanL-BoldItal"
        | "Times-Roman"
        | "Times"
        | "Times New Roman"
        | "TimesNewRomanPSMT"
        | "TimesNewRoman"
        | "TimesNewRomanPS"
        | "LiberationSerif"
        | "NimbusRomNo9L-Regu"
        | "Times-Bold"
        | "TimesNewRomanPS-BoldMT"
        | "TimesNewRomanPS-Bold"
        | "TimesNewRoman-Bold"
        | "LiberationSerif-Bold"
        | "NimbusRomNo9L-Medi"
        | "Times-Italic"
        | "TimesNewRomanPS-ItalicMT"
        | "TimesNewRomanPS-Italic"
        | "TimesNewRoman-Italic"
        | "LiberationSerif-Italic"
        | "NimbusRomNo9L-ReguItal"
        | "Times-BoldItalic"
        | "TimesNewRomanPS-BoldItalicMT"
        | "TimesNewRomanPS-BoldItalic"
        | "TimesNewRoman-BoldItalic"
        | "LiberationSerif-BoldItalic"
        | "NimbusRomNo9L-MediItal"
        | "Courier"
        | "CourierNew"
        | "CourierNewPSMT"
        | "LiberationMono"
        | "NimbusMonL-Regu"
        | "Courier-Bold"
        | "CourierNewPS-BoldMT"
        | "CourierNew-Bold"
        | "LiberationMono-Bold"
        | "NimbusMonL-Bold"
        | "Courier-Oblique"
        | "CourierNewPS-ItalicMT"
        | "CourierNew-Italic"
        | "LiberationMono-Italic"
        | "NimbusMonL-ReguObli"
        | "Courier-BoldOblique"
        | "CourierNewPS-BoldItalicMT"
        | "CourierNew-BoldItalic"
        | "LiberationMono-BoldItalic"
        | "NimbusMonL-BoldObli" => base_font,
        _ => pick_fallback_standard_font_key(base_font),
    };

    // Prefer Foxit Base14 CFF (same programs as pdf.js / hayro) so unembedded
    // standard fonts share outlines and 1000-unit glyph space with those engines.
    // Liberation TTF remains on disk for optional system-font experiments only.
    // Source: hayro/hayro-interpret/src/font/standard_font.rs (`StandardFont::get_font_data`)
    let font = match key {
        "Helvetica" | "ArialMT" | "Arial" | "LiberationSans" | "NimbusSanL-Regu" => {
            include_bytes!("../../assets/fonts/FoxitSans.pfb").as_slice()
        }
        "Helvetica-Bold"
        | "Arial-BoldMT"
        | "Arial-Bold"
        | "Arial,Bold"
        | "LiberationSans-Bold"
        | "NimbusSanL-Bold" => include_bytes!("../../assets/fonts/FoxitSansBold.pfb").as_slice(),
        "Helvetica-Oblique"
        | "Arial-ItalicMT"
        | "Arial-Italic"
        | "Helvetica-Italic"
        | "LiberationSans-Italic"
        | "NimbusSanL-ReguItal" => {
            include_bytes!("../../assets/fonts/FoxitSansItalic.pfb").as_slice()
        }
        "Helvetica-BoldOblique"
        | "Arial-BoldItalicMT"
        | "Helvetica-BoldItalic"
        | "LiberationSans-BoldItalic"
        | "NimbusSanL-BoldItal" => {
            include_bytes!("../../assets/fonts/FoxitSansBoldItalic.pfb").as_slice()
        }
        "Times-Roman" | "Times" | "Times New Roman" | "TimesNewRomanPSMT" | "TimesNewRoman"
        | "TimesNewRomanPS" | "LiberationSerif" | "NimbusRomNo9L-Regu" => {
            include_bytes!("../../assets/fonts/FoxitSerif.pfb").as_slice()
        }
        "Times-Bold"
        | "TimesNewRomanPS-BoldMT"
        | "TimesNewRomanPS-Bold"
        | "TimesNewRoman-Bold"
        | "LiberationSerif-Bold"
        | "NimbusRomNo9L-Medi" => {
            include_bytes!("../../assets/fonts/FoxitSerifBold.pfb").as_slice()
        }
        "Times-Italic"
        | "TimesNewRomanPS-ItalicMT"
        | "TimesNewRomanPS-Italic"
        | "TimesNewRoman-Italic"
        | "LiberationSerif-Italic"
        | "NimbusRomNo9L-ReguItal" => {
            include_bytes!("../../assets/fonts/FoxitSerifItalic.pfb").as_slice()
        }
        "Times-BoldItalic"
        | "TimesNewRomanPS-BoldItalicMT"
        | "TimesNewRomanPS-BoldItalic"
        | "TimesNewRoman-BoldItalic"
        | "LiberationSerif-BoldItalic"
        | "NimbusRomNo9L-MediItal" => {
            include_bytes!("../../assets/fonts/FoxitSerifBoldItalic.pfb").as_slice()
        }
        "Courier" | "CourierNew" | "CourierNewPSMT" | "LiberationMono" | "NimbusMonL-Regu" => {
            include_bytes!("../../assets/fonts/FoxitFixed.pfb").as_slice()
        }
        "Courier-Bold"
        | "CourierNewPS-BoldMT"
        | "CourierNew-Bold"
        | "LiberationMono-Bold"
        | "NimbusMonL-Bold" => include_bytes!("../../assets/fonts/FoxitFixedBold.pfb").as_slice(),
        "Courier-Oblique"
        | "CourierNewPS-ItalicMT"
        | "CourierNew-Italic"
        | "LiberationMono-Italic"
        | "NimbusMonL-ReguObli" => {
            include_bytes!("../../assets/fonts/FoxitFixedItalic.pfb").as_slice()
        }
        "Courier-BoldOblique"
        | "CourierNewPS-BoldItalicMT"
        | "CourierNew-BoldItalic"
        | "LiberationMono-BoldItalic"
        | "NimbusMonL-BoldObli" => {
            include_bytes!("../../assets/fonts/FoxitFixedBoldItalic.pfb").as_slice()
        }
        "Symbol" => include_bytes!("../../assets/fonts/FoxitSymbol.pfb").as_slice(),
        "ZapfDingbats" => include_bytes!("../../assets/fonts/FoxitDingbats.pfb").as_slice(),
        // pick_fallback always returns a Base-14 family key above.
        _ => include_bytes!("../../assets/fonts/FoxitSerif.pfb").as_slice(),
    };

    Some(font.to_vec())
}

fn parse_four_number_array(obj: Option<&PDFObject>) -> Option<[f64; 4]> {
    let PDFObject::Array(values) = obj? else {
        return None;
    };
    if values.len() < 4 {
        return None;
    }

    let mut out = [0.0f64; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let PDFObject::Number(value) = values[i].as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

fn parse_three_number_array(obj: Option<&PDFObject>) -> Option<[f64; 3]> {
    let PDFObject::Array(values) = obj? else {
        return None;
    };
    if values.len() < 3 {
        return None;
    }

    let mut out = [0.0f64; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        let PDFObject::Number(value) = values[i].as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

fn parse_nine_number_array(obj: Option<&PDFObject>) -> Option<[f64; 9]> {
    let PDFObject::Array(values) = obj? else {
        return None;
    };
    if values.len() < 9 {
        return None;
    }

    let mut out = [0.0f64; 9];
    for (i, slot) in out.iter_mut().enumerate() {
        let PDFObject::Number(value) = values[i].as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

fn parse_six_number_array(obj: Option<&PDFObject>) -> Option<[f64; 6]> {
    let PDFObject::Array(values) = obj? else {
        return None;
    };
    if values.len() < 6 {
        return None;
    }

    let mut out = [0.0f64; 6];
    for (i, slot) in out.iter_mut().enumerate() {
        let PDFObject::Number(value) = values[i].as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

fn soft_mask_backdrop_rgb(obj: Option<&PDFObject>) -> Option<[u8; 3]> {
    let components = number_array(obj)?;
    let color = match components.as_slice() {
        [g] => Color::Gray(*g),
        [r, g, b] => Color::RGB(*r, *g, *b),
        [c, m, y, k] => Color::CMYK(*c, *m, *y, *k),
        _ => return None,
    };
    let (r, g, b, _) = color.rgba();
    Some([r, g, b])
}

fn form_has_transparency_group(
    form_dict: &HashMap<String, PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> bool {
    let Some(group_obj) = form_dict.get("Group").cloned() else {
        return false;
    };
    let Some(PDFObject::Dictionary(group_dict)) = resolve_object_clone(group_obj, resolver) else {
        return false;
    };
    matches!(group_dict.get("S"), Some(PDFObject::Name(name)) if name == "Transparency")
}

fn is_pattern_color_space(color_space: &PDFObject) -> bool {
    match color_space {
        PDFObject::Name(name) => name == "Pattern",
        PDFObject::Array(arr) => {
            matches!(arr.first().map(|v| v.as_ref()), Some(PDFObject::Name(name)) if name == "Pattern")
        }
        _ => false,
    }
}

fn merge_pattern_resources(
    pattern_resources: Option<PDFObject>,
    parent_resources: Option<PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<PDFObject> {
    let pattern_resources = pattern_resources.and_then(|r| resolve_object_clone(r, resolver));
    let parent_resources = parent_resources.and_then(|r| resolve_object_clone(r, resolver));

    match (pattern_resources, parent_resources) {
        (Some(PDFObject::Dictionary(mut pattern)), Some(PDFObject::Dictionary(parent))) => {
            for (key, value) in parent {
                pattern.entry(key).or_insert(value);
            }
            Some(PDFObject::Dictionary(pattern))
        }
        (Some(resources), _) => Some(resources),
        (None, Some(resources)) => Some(resources),
        (None, None) => None,
    }
}

fn pattern_tile_bounds(
    pattern: &TilingPatternPaint,
    fill_bbox: Option<(f64, f64, f64, f64)>,
    page_bounds: (f64, f64),
    ctm: [f64; 6],
    base: [f64; 6],
) -> (i32, i32, i32, i32) {
    let device_bbox = fill_bbox.map(|bbox| transform_bbox(bbox, ctm)).unwrap_or((
        0.0,
        0.0,
        page_bounds.0,
        page_bounds.1,
    ));

    let expanded = (
        device_bbox.0.floor().max(0.0),
        device_bbox.1.floor().max(0.0),
        device_bbox.2.ceil().min(page_bounds.0),
        device_bbox.3.ceil().min(page_bounds.1),
    );

    if let Some(inv) = invert_matrix(base) {
        let pattern_bbox = transform_bbox(expanded, inv);
        tile_range_for_axis(
            pattern_bbox.0,
            pattern_bbox.2,
            pattern.bbox[0],
            pattern.bbox[2],
            pattern.x_step,
        )
        .zip(tile_range_for_axis(
            pattern_bbox.1,
            pattern_bbox.3,
            pattern.bbox[1],
            pattern.bbox[3],
            pattern.y_step,
        ))
        .map(|((ix0, ix1), (iy0, iy1))| (ix0, ix1, iy0, iy1))
        .unwrap_or((0, 0, 0, 0))
    } else {
        (0, 0, 0, 0)
    }
}

#[allow(unused_variables)]
fn trace_tiling_pattern(
    stage: &str,
    pattern: &TilingPatternPaint,
    fill_bbox: Option<(f64, f64, f64, f64)>,
    page_bounds: (f64, f64),
    ctm: [f64; 6],
    base: [f64; 6],
    bounds: (i32, i32, i32, i32),
) {
    #[cfg(feature = "debug-logging")]
    {
        let device_bbox = fill_bbox.map(|bbox| transform_bbox(bbox, ctm)).unwrap_or((
            0.0,
            0.0,
            page_bounds.0,
            page_bounds.1,
        ));
        let clip = (
            device_bbox.0.floor().max(0.0),
            device_bbox.1.floor().max(0.0),
            device_bbox.2.ceil().min(page_bounds.0),
            device_bbox.3.ceil().min(page_bounds.1),
        );
        let cell_bbox = transform_bbox(
            (
                pattern.bbox[0],
                pattern.bbox[1],
                pattern.bbox[2],
                pattern.bbox[3],
            ),
            base,
        );
        let bitmap_w = cell_bbox_width(cell_bbox).ceil().max(1.0) as i32;
        let bitmap_h = cell_bbox_height(cell_bbox).ceil().max(1.0) as i32;
        let pdfium_bounds = pdfium_like_tile_bounds(pattern, clip, base);

        eprintln!(
            "[tiling-trace:{stage}] bbox={:?} xstep={:.6} ystep={:.6} matrix={:?}",
            pattern.bbox, pattern.x_step, pattern.y_step, pattern.matrix
        );
        eprintln!(
            "[tiling-trace:{stage}] ctm={:?} root_pattern_base={:?} fill_bbox={:?} clip={:?}",
            ctm, base, fill_bbox, clip
        );
        eprintln!(
            "[tiling-trace:{stage}] cell_bbox_device={:?} bitmap={}x{} ours_bounds={:?} pdfium_bounds={:?}",
            cell_bbox, bitmap_w, bitmap_h, bounds, pdfium_bounds
        );
        if let Some((min_col, max_col, min_row, max_row)) = pdfium_bounds {
            let left_offset = cell_bbox.0 - base[4];
            let top_offset = cell_bbox.1 - base[5];
            eprintln!(
                "[tiling-trace:{stage}] offsets left={:.6} top={:.6} first_start=({:.6},{:.6}) last_start=({:.6},{:.6})",
                left_offset,
                top_offset,
                base[0] * (min_col as f64 * pattern.x_step)
                    + base[2] * (min_row as f64 * pattern.y_step)
                    + base[4]
                    + left_offset,
                base[1] * (min_col as f64 * pattern.x_step)
                    + base[3] * (min_row as f64 * pattern.y_step)
                    + base[5]
                    + top_offset,
                base[0] * (max_col as f64 * pattern.x_step)
                    + base[2] * (max_row as f64 * pattern.y_step)
                    + base[4]
                    + left_offset,
                base[1] * (max_col as f64 * pattern.x_step)
                    + base[3] * (max_row as f64 * pattern.y_step)
                    + base[5]
                    + top_offset,
            );
        }
    }
}

#[allow(unused_variables)]
fn trace_tiling_cell(
    pattern: &TilingPatternPaint,
    base: [f64; 6],
    cell_transform: [f64; 6],
    image_to_device: [f64; 6],
    pix_w: u32,
    pix_h: u32,
    render_w: u32,
    render_h: u32,
) {
    #[cfg(feature = "debug-logging")]
    {
        let bbox_w = (pattern.bbox[2] - pattern.bbox[0]).abs();
        let bbox_h = (pattern.bbox[3] - pattern.bbox[1]).abs();
        eprintln!(
            "[tiling-cell-core] bbox_size=({:.6},{:.6}) step=({:.6},{:.6}) pix={}x{} render={}x{} overlap=({}, {})",
            bbox_w,
            bbox_h,
            pattern.x_step,
            pattern.y_step,
            pix_w,
            pix_h,
            render_w,
            render_h,
            bbox_w > pattern.x_step.abs(),
            bbox_h > pattern.y_step.abs(),
        );
        eprintln!(
            "[tiling-cell-core] base={:?} cell_transform={:?} image_to_device={:?}",
            base, cell_transform, image_to_device
        );
    }
}

#[cfg(feature = "debug-logging")]
fn cell_bbox_width(bbox: (f64, f64, f64, f64)) -> f64 {
    (bbox.2 - bbox.0).abs()
}

#[cfg(feature = "debug-logging")]
fn cell_bbox_height(bbox: (f64, f64, f64, f64)) -> f64 {
    (bbox.3 - bbox.1).abs()
}

#[cfg(feature = "debug-logging")]
fn pdfium_like_tile_bounds(
    pattern: &TilingPatternPaint,
    clip: (f64, f64, f64, f64),
    base: [f64; 6],
) -> Option<(i32, i32, i32, i32)> {
    let inv = invert_matrix(base)?;
    let clip_p = transform_bbox(clip, inv);
    if pattern.x_step.abs() <= f64::EPSILON || pattern.y_step.abs() <= f64::EPSILON {
        return None;
    }

    let min_col = ((clip_p.0 - pattern.bbox[2]) / pattern.x_step).ceil() as i32;
    let max_col = ((clip_p.2 - pattern.bbox[0]) / pattern.x_step).floor() as i32;
    let min_row = ((clip_p.1 - pattern.bbox[3]) / pattern.y_step).ceil() as i32;
    let max_row = ((clip_p.3 - pattern.bbox[1]) / pattern.y_step).floor() as i32;
    Some((min_col, max_col, min_row, max_row))
}

fn pattern_has_overlapping_tiles(pattern: &TilingPatternPaint) -> bool {
    let width = (pattern.bbox[2] - pattern.bbox[0]).abs();
    let height = (pattern.bbox[3] - pattern.bbox[1]).abs();
    pattern.x_step.abs() < width || pattern.y_step.abs() < height
}

fn tile_range_for_axis(
    area_min: f64,
    area_max: f64,
    tile_min: f64,
    tile_max: f64,
    step: f64,
) -> Option<(i32, i32)> {
    if step.abs() <= f64::EPSILON {
        return None;
    }
    let s = step.abs();
    let lo = ((area_min - tile_max) / s).ceil() as i32;
    let hi = ((area_max - tile_min) / s).floor() as i32;
    Some((lo, hi))
}

fn transform_bbox(bbox: (f64, f64, f64, f64), matrix: [f64; 6]) -> (f64, f64, f64, f64) {
    let corners = [
        transform_point(matrix, bbox.0, bbox.1),
        transform_point(matrix, bbox.2, bbox.1),
        transform_point(matrix, bbox.2, bbox.3),
        transform_point(matrix, bbox.0, bbox.3),
    ];

    corners.iter().fold(
        (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
        |(min_x, min_y, max_x, max_y), (x, y)| {
            (min_x.min(*x), min_y.min(*y), max_x.max(*x), max_y.max(*y))
        },
    )
}

fn transform_point(matrix: [f64; 6], x: f64, y: f64) -> (f64, f64) {
    (
        matrix[0] * x + matrix[2] * y + matrix[4],
        matrix[1] * x + matrix[3] * y + matrix[5],
    )
}

fn invert_matrix(matrix: [f64; 6]) -> Option<[f64; 6]> {
    let [a, b, c, d, e, f] = matrix;
    let det = a * d - b * c;
    if det.abs() <= 1e-12 {
        return None;
    }

    let inv_det = 1.0 / det;
    Some([
        d * inv_det,
        -b * inv_det,
        -c * inv_det,
        a * inv_det,
        (c * f - d * e) * inv_det,
        (b * e - a * f) * inv_det,
    ])
}

fn parse_blend_mode(obj: &PDFObject) -> Option<super::graphics_state::BlendMode> {
    use super::graphics_state::BlendMode;

    let name = match obj {
        PDFObject::Name(name) => Some(name.as_str()),
        PDFObject::Array(items) => items.iter().find_map(|item| match item.as_ref() {
            PDFObject::Name(name) => Some(name.as_str()),
            _ => None,
        }),
        _ => None,
    }?;

    match name {
        "Normal" | "Compatible" => Some(BlendMode::Normal),
        "Multiply" => Some(BlendMode::Multiply),
        "Screen" => Some(BlendMode::Screen),
        "Overlay" => Some(BlendMode::Overlay),
        "Darken" => Some(BlendMode::Darken),
        "Lighten" => Some(BlendMode::Lighten),
        "ColorDodge" => Some(BlendMode::ColorDodge),
        "ColorBurn" => Some(BlendMode::ColorBurn),
        "HardLight" => Some(BlendMode::HardLight),
        "SoftLight" => Some(BlendMode::SoftLight),
        "Difference" => Some(BlendMode::Difference),
        "Exclusion" => Some(BlendMode::Exclusion),
        "Hue" => Some(BlendMode::Hue),
        "Saturation" => Some(BlendMode::Saturation),
        "Color" => Some(BlendMode::Color),
        "Luminosity" => Some(BlendMode::Luminosity),
        _ => None,
    }
}

fn font_needs_to_unicode_remap(font_dict_info: &crate::core::font::FontDict) -> bool {
    use crate::core::font::FontType;

    if matches!(
        font_dict_info.font_type,
        FontType::CIDFontType0 | FontType::CIDFontType2
    ) {
        return true;
    }

    if font_dict_info.descendant_fonts.is_some() {
        return true;
    }

    matches!(
        font_dict_info.encoding.as_ref(),
        Some(PDFObject::Name(name)) if matches!(name.as_str(), "Identity-H" | "Identity-V")
    )
}

fn font_encoding_is_identity_v(font_dict_info: &crate::core::font::FontDict) -> bool {
    matches!(
        font_dict_info.encoding.as_ref(),
        Some(PDFObject::Name(name)) if name == "Identity-V"
    )
}

fn font_uses_identity_cid_to_gid(
    font_obj: &PDFObject,
    font_dict_info: &crate::core::font::FontDict,
    resolver: &mut dyn PageResolver,
) -> bool {
    use crate::core::font::FontType;

    let has_identity_encoding = matches!(
        font_dict_info.encoding.as_ref(),
        Some(PDFObject::Name(name)) if matches!(name.as_str(), "Identity-H" | "Identity-V")
    );
    if !has_identity_encoding {
        return false;
    }

    let Some(font_dict) = font_obj.as_dictionary() else {
        return false;
    };

    if matches!(font_dict_info.font_type, FontType::CIDFontType2) {
        // PDF 32000-1:2008 §9.7.4.2 Table 117: CIDToGIDMap default is Identity
        // when the entry is absent. Only a stream map is non-identity.
        return match font_dict.get("CIDToGIDMap") {
            None => true,
            Some(PDFObject::Name(name)) if name == "Identity" => true,
            _ => false,
        };
    }

    if !matches!(font_dict_info.font_type, FontType::Type0) {
        return false;
    }

    // `/DescendantFonts` is frequently an indirect array ref (`6 0 R` →
    // `[7 0 R]`), as on tile_clamp_bug.pdf Type0 fonts. Resolve before Array.
    let Some(descendant_fonts) = font_dict.get("DescendantFonts") else {
        return false;
    };
    let mut resolver_opt = Some(resolver);
    let Some(PDFObject::Array(descendants)) =
        resolve_object_clone(descendant_fonts.clone(), &mut resolver_opt)
    else {
        return false;
    };
    let Some(first_descendant) = descendants.first() else {
        return false;
    };
    let Some(PDFObject::Dictionary(descendant)) =
        resolve_object_clone(first_descendant.as_ref().clone(), &mut resolver_opt)
    else {
        return false;
    };

    let subtype = match descendant.get("Subtype") {
        Some(PDFObject::Name(s)) => s.as_str(),
        _ => return false,
    };

    // Identity-H/V Type0 always carries multi-byte CIDs in the content stream
    // (PDF §9.7.6.2). Enable raw-CID decoding for both CFF (CIDFontType0) and
    // TrueType (CIDFontType2) descendants — otherwise UTF-16BE CIDs are misread
    // as single bytes (nulls + ASCII), mangling headlines like BentonSansComp
    // Black on tile_clamp_bug.pdf.
    //
    // CIDToGIDMap applies only to CIDFontType2 (PDF §9.7.4.2 Table 117); when
    // omitted the default is Identity. Type0 CFF maps CIDs via the font program
    // charset (`glyph_index_by_cid` / Identity gid).
    match subtype {
        "CIDFontType0" => true,
        "CIDFontType2" => match descendant.get("CIDToGIDMap") {
            None => true,
            Some(PDFObject::Name(name)) if name == "Identity" => true,
            // Stream map is non-identity but text is still multi-byte CIDs.
            Some(PDFObject::Stream { .. }) | Some(PDFObject::Ref(_)) => true,
            _ => false,
        },
        _ => false,
    }
}

/// Returns raw text bytes for PDF text string objects.
///
/// PDF content streams represent text with both literal strings `( ... )`
/// and hexadecimal strings `< ... >`. Both should be rendered as byte sequences.
fn text_bytes(obj: &PDFObject) -> Option<&[u8]> {
    match obj {
        PDFObject::String(bytes) | PDFObject::HexString(bytes) => Some(bytes.as_slice()),
        _ => None,
    }
}

impl Type3Font {
    fn width_for_code(&self, code: u8) -> f64 {
        let code = code as u32;
        let width = if code < self.first_char {
            self.default_width
        } else {
            let idx = (code - self.first_char) as usize;
            self.widths.get(idx).copied().unwrap_or(self.default_width)
        };

        width * self.font_matrix[0]
    }
}

fn build_type3_font(
    font_obj: &Arc<PDFObject>,
    font_dict_info: &crate::core::font::FontDict,
    resolver: &mut dyn PageResolver,
) -> Option<Type3Font> {
    let dict = match &**font_obj {
        PDFObject::Dictionary(d) => d,
        _ => return None,
    };

    let char_procs_obj = resolver.resolve_if_ref(dict.get("CharProcs")?).ok()?;
    let char_procs_dict = match &*char_procs_obj {
        PDFObject::Dictionary(d) => d,
        _ => return None,
    };

    let mut char_procs = HashMap::new();
    for (name, proc_ref) in char_procs_dict {
        let proc_obj = resolver.resolve_if_ref(proc_ref).ok()?;
        if let PDFObject::Stream { dict, data } = &*proc_obj {
            let decoded = if let Some(filter) = dict.get("Filter") {
                crate::core::decode::apply_filters(data, filter).unwrap_or_else(|_| data.clone())
            } else {
                data.clone()
            };
            let is_shape_glyph = type3_is_shape_glyph(&decoded);
            char_procs.insert(
                name.clone(),
                Type3Glyph {
                    data: decoded,
                    is_shape_glyph,
                    resources: dict.get("Resources").cloned(),
                },
            );
        }
    }

    if char_procs.is_empty() {
        return None;
    }

    Some(Type3Font {
        encoding: build_type3_encoding(font_dict_info.encoding.as_ref(), resolver),
        char_procs,
        resources: dict.get("Resources").cloned(),
        font_matrix: parse_six_number_array(dict.get("FontMatrix"))
            .unwrap_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]),
        first_char: font_dict_info.first_char.unwrap_or(0),
        widths: font_dict_info.widths.clone().unwrap_or_default(),
        default_width: font_dict_info.default_width,
    })
}

/// Classify a Type3 CharProc as an uncolored *shape* glyph (`d0`) vs a colored
/// glyph (`d1`). Shape glyphs paint with the current text colour; coloured
/// glyphs supply their own paints (often ImageMasks with `/Decode`).
///
/// PDF 1.7 §9.6.5: `d0` = wx wy d0 (shape), `d1` = wx wy llx lly urx ury d1
/// (coloured). Matches hayro `Type3::render_glyph` ShapeGlyph vs ColorGlyph.
///
/// Source: hayro/hayro-interpret/src/font/type3.rs
fn type3_is_shape_glyph(data: &[u8]) -> bool {
    let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
    let Ok(lexer) = Lexer::new(stream) else {
        return false;
    };
    let Ok(parser) = Parser::new(lexer) else {
        return false;
    };
    let mut evaluator = ContentStreamEvaluator::new(parser);

    while let Ok(Some(op)) = evaluator.read_operation() {
        match op.op {
            // d0 — uncolored shape glyph
            OpCode::SetCharWidth => return true,
            // d1 — coloured glyph (own colour / image mask)
            OpCode::SetCharWidthAndBounds => return false,
            _ => {}
        }
    }

    false
}

fn build_type3_encoding(
    encoding_obj: Option<&PDFObject>,
    resolver: &mut dyn PageResolver,
) -> Type3Encoding {
    let Some(encoding_obj) = encoding_obj else {
        return Type3Encoding::Names(type3_base_encoding_names("StandardEncoding"));
    };

    let resolved = resolver
        .resolve_if_ref(encoding_obj)
        .unwrap_or_else(|_| Arc::new(encoding_obj.clone()));

    if let Some(names) = type3_difference_names(&resolved) {
        Type3Encoding::Names(names)
    } else {
        Type3Encoding::Text(Encoding::from_pdf_object(&resolved).unwrap_or(Encoding::Standard))
    }
}

fn type3_difference_names(encoding_obj: &PDFObject) -> Option<Vec<Option<String>>> {
    let PDFObject::Dictionary(dict) = encoding_obj else {
        return None;
    };

    let base_name = match dict.get("BaseEncoding") {
        Some(PDFObject::Name(name)) => name.as_str(),
        _ => "StandardEncoding",
    };
    let mut names = type3_base_encoding_names(base_name);

    let PDFObject::Array(differences) = dict.get("Differences")? else {
        return None;
    };

    let mut current = 0usize;
    for item in differences {
        match item.as_ref() {
            PDFObject::Number(n) => current = (*n as usize).min(255),
            PDFObject::Name(name) => {
                if current < names.len() {
                    names[current] = Some(name.clone());
                }
                current = (current + 1).min(256);
            }
            _ => {}
        }
    }

    Some(names)
}

fn type3_base_encoding_names(base_name: &str) -> Vec<Option<String>> {
    let mut names = vec![None; 256];
    for code in 0x20u8..=0x7e {
        names[code as usize] = type3_ascii_glyph_name(code).map(str::to_string);
    }

    if matches!(base_name, "WinAnsiEncoding" | "MacRomanEncoding") {
        const WIN_ANSI_NAMES: &[(u8, &str)] = &[
            (0x80, "Euro"),
            (0x82, "quotesinglbase"),
            (0x83, "florin"),
            (0x84, "quotedblbase"),
            (0x85, "ellipsis"),
            (0x86, "dagger"),
            (0x87, "daggerdbl"),
            (0x88, "circumflex"),
            (0x89, "perthousand"),
            (0x8a, "Scaron"),
            (0x8b, "guilsinglleft"),
            (0x8c, "OE"),
            (0x8e, "Zcaron"),
            (0x91, "quoteleft"),
            (0x92, "quoteright"),
            (0x93, "quotedblleft"),
            (0x94, "quotedblright"),
            (0x95, "bullet"),
            (0x96, "endash"),
            (0x97, "emdash"),
            (0x98, "tilde"),
            (0x99, "trademark"),
            (0x9a, "scaron"),
            (0x9b, "guilsinglright"),
            (0x9c, "oe"),
            (0x9e, "zcaron"),
            (0x9f, "Ydieresis"),
        ];
        for &(code, name) in WIN_ANSI_NAMES {
            names[code as usize] = Some(name.to_string());
        }
    }

    names
}

fn type3_ascii_glyph_name(code: u8) -> Option<&'static str> {
    Some(match code {
        b' ' => "space",
        b'!' => "exclam",
        b'"' => "quotedbl",
        b'#' => "numbersign",
        b'$' => "dollar",
        b'%' => "percent",
        b'&' => "ampersand",
        b'\'' => "quotesingle",
        b'(' => "parenleft",
        b')' => "parenright",
        b'*' => "asterisk",
        b'+' => "plus",
        b',' => "comma",
        b'-' => "hyphen",
        b'.' => "period",
        b'/' => "slash",
        b'0' => "zero",
        b'1' => "one",
        b'2' => "two",
        b'3' => "three",
        b'4' => "four",
        b'5' => "five",
        b'6' => "six",
        b'7' => "seven",
        b'8' => "eight",
        b'9' => "nine",
        b':' => "colon",
        b';' => "semicolon",
        b'<' => "less",
        b'=' => "equal",
        b'>' => "greater",
        b'?' => "question",
        b'@' => "at",
        b'A' => "A",
        b'B' => "B",
        b'C' => "C",
        b'D' => "D",
        b'E' => "E",
        b'F' => "F",
        b'G' => "G",
        b'H' => "H",
        b'I' => "I",
        b'J' => "J",
        b'K' => "K",
        b'L' => "L",
        b'M' => "M",
        b'N' => "N",
        b'O' => "O",
        b'P' => "P",
        b'Q' => "Q",
        b'R' => "R",
        b'S' => "S",
        b'T' => "T",
        b'U' => "U",
        b'V' => "V",
        b'W' => "W",
        b'X' => "X",
        b'Y' => "Y",
        b'Z' => "Z",
        b'a' => "a",
        b'b' => "b",
        b'c' => "c",
        b'd' => "d",
        b'e' => "e",
        b'f' => "f",
        b'g' => "g",
        b'h' => "h",
        b'i' => "i",
        b'j' => "j",
        b'k' => "k",
        b'l' => "l",
        b'm' => "m",
        b'n' => "n",
        b'o' => "o",
        b'p' => "p",
        b'q' => "q",
        b'r' => "r",
        b's' => "s",
        b't' => "t",
        b'u' => "u",
        b'v' => "v",
        b'w' => "w",
        b'x' => "x",
        b'y' => "y",
        b'z' => "z",
        b'[' => "bracketleft",
        b'\\' => "backslash",
        b']' => "bracketright",
        b'^' => "asciicircum",
        b'_' => "underscore",
        b'`' => "grave",
        b'{' => "braceleft",
        b'|' => "bar",
        b'}' => "braceright",
        b'~' => "asciitilde",
        _ => return None,
    })
}

fn compose_type3_text_transform(
    text_matrix: [f64; 6],
    font_size: f64,
    horizontal_scaling: f64,
    text_rise: f64,
) -> [f64; 6] {
    let [a, b, c, d, e, f] = text_matrix;
    let h_scale = horizontal_scaling / 100.0;

    [
        a * font_size * h_scale,
        b * font_size * h_scale,
        c * font_size,
        d * font_size,
        c * text_rise + e,
        d * text_rise + f,
    ]
}

fn text_paint_for_rendering_mode(
    mode: super::graphics_state::TextRenderingMode,
    fill_color: Color,
) -> Paint {
    use super::graphics_state::TextRenderingMode;

    match mode {
        // Invisible/clip-only text should not be painted but still advance the text matrix.
        // Stroke-only modes use the stroke paint instead of fill.
        TextRenderingMode::Invisible
        | TextRenderingMode::Clip
        | TextRenderingMode::Stroke
        | TextRenderingMode::StrokeClip => Paint::Transparent,
        _ => Paint::from_color(fill_color),
    }
}

/// Stroke paint for text rendering modes that stroke glyph outlines (1, 2, 5, 6).
fn text_stroke_paint_for_rendering_mode(
    mode: super::graphics_state::TextRenderingMode,
    stroke_color: Color,
) -> Paint {
    use super::graphics_state::TextRenderingMode;

    match mode {
        TextRenderingMode::Stroke
        | TextRenderingMode::FillStroke
        | TextRenderingMode::StrokeClip
        | TextRenderingMode::FillStrokeClip => Paint::from_color(stroke_color),
        // Invisible/clip-only/fill-only: no stroke contribution.
        _ => Paint::Transparent,
    }
}

/// Whether the rendering mode requires adding glyph outlines to the clip path.
fn mode_clips(mode: super::graphics_state::TextRenderingMode) -> bool {
    matches!(
        mode,
        super::graphics_state::TextRenderingMode::FillClip
            | super::graphics_state::TextRenderingMode::StrokeClip
            | super::graphics_state::TextRenderingMode::FillStrokeClip
            | super::graphics_state::TextRenderingMode::Clip
    )
}

fn mode_fills(mode: super::graphics_state::TextRenderingMode) -> bool {
    use super::graphics_state::TextRenderingMode;
    matches!(
        mode,
        TextRenderingMode::Fill
            | TextRenderingMode::FillStroke
            | TextRenderingMode::FillClip
            | TextRenderingMode::FillStrokeClip
    )
}

fn mode_strokes(mode: super::graphics_state::TextRenderingMode) -> bool {
    use super::graphics_state::TextRenderingMode;
    matches!(
        mode,
        TextRenderingMode::Stroke
            | TextRenderingMode::FillStroke
            | TextRenderingMode::StrokeClip
            | TextRenderingMode::FillStrokeClip
    )
}

fn resolve_object_clone(
    object: PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<PDFObject> {
    match object {
        PDFObject::Ref(r) => resolver
            .as_mut()?
            .resolve_ref(r.num, r.generation)
            .ok()
            .map(|o| (*o).clone()),
        other => Some(other),
    }
}

fn color_from_color_space(
    color_space: &PDFObject,
    args: &[PDFObject],
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Color> {
    match color_space {
        PDFObject::Name(name) => color_from_named_space(name, args),
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" && arr.len() >= 2 => {
                let (xw, yw, zw, amin, amax, bmin, bmax) = parse_lab_params(arr);
                let l = extract_number(args, 0).ok()?;
                let a = extract_number(args, 1).ok()?;
                let b = extract_number(args, 2).ok()?;
                Some(lab_components_to_color(
                    l, a, b, xw, yw, zw, amin, amax, bmin, bmax,
                ))
            }
            Some(PDFObject::Name(name)) if name == "Separation" && arr.len() >= 4 => {
                let tint = extract_number(args, 0).ok()?;
                let alternate = resolve_object_clone(arr[2].as_ref().clone(), resolver)?;
                let tint_fn = resolve_object_clone(arr[3].as_ref().clone(), resolver)?;
                let alternate_components = components_for_color_space(&alternate, resolver).max(1);
                let components = eval_tint_function(&tint_fn, &[tint], alternate_components)?;
                color_from_components(&alternate, &components, resolver)
            }
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                color_from_icc_based(arr[1].as_ref(), args, resolver)
            }
            _ => None,
        },
        _ => None,
    }
}

fn color_from_named_space(name: &str, args: &[PDFObject]) -> Option<Color> {
    match name {
        "DeviceGray" | "G" => Some(Color::Gray(extract_number(args, 0).ok()?)),
        "DeviceRGB" | "RGB" => Some(Color::RGB(
            extract_number(args, 0).ok()?,
            extract_number(args, 1).ok()?,
            extract_number(args, 2).ok()?,
        )),
        "DeviceCMYK" | "CMYK" => Some(Color::CMYK(
            extract_number(args, 0).ok()?,
            extract_number(args, 1).ok()?,
            extract_number(args, 2).ok()?,
            extract_number(args, 3).ok()?,
        )),
        _ => None,
    }
}

fn initial_color_for_color_space(
    color_space: Option<PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Color> {
    let color_space = resolve_object_clone(color_space?, resolver)?;
    match color_space {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => Some(Color::Gray(0.0)),
            "DeviceRGB" | "RGB" => Some(Color::RGB(0.0, 0.0, 0.0)),
            "DeviceCMYK" | "CMYK" => Some(Color::CMYK(0.0, 0.0, 0.0, 1.0)),
            _ => None,
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" => Some(Color::RGB(0.0, 0.0, 0.0)),
            Some(PDFObject::Name(name)) if name == "Separation" => color_from_color_space(
                &PDFObject::Array(arr.clone()),
                &[PDFObject::Number(1.0)],
                resolver,
            ),
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                let profile = resolve_object_clone(arr[1].as_ref().clone(), resolver)
                    .unwrap_or_else(|| arr[1].as_ref().clone());
                match icc_based_component_count(&profile) {
                    1 => Some(Color::Gray(0.0)),
                    3 => Some(Color::RGB(0.0, 0.0, 0.0)),
                    4 => Some(Color::CMYK(0.0, 0.0, 0.0, 1.0)),
                    _ => None,
                }
            }
            _ => None,
        },
        PDFObject::Ref(_) => initial_color_for_color_space(Some(color_space), resolver),
        _ => None,
    }
}

fn color_from_components(
    color_space: &PDFObject,
    components: &[f64],
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Color> {
    match color_space {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => Some(Color::Gray(*components.first()?)),
            "DeviceRGB" | "RGB" => Some(Color::RGB(
                *components.first()?,
                *components.get(1)?,
                *components.get(2)?,
            )),
            "DeviceCMYK" | "CMYK" => Some(Color::CMYK(
                *components.first()?,
                *components.get(1)?,
                *components.get(2)?,
                *components.get(3)?,
            )),
            _ => None,
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" && arr.len() >= 2 => {
                let (xw, yw, zw, amin, amax, bmin, bmax) = parse_lab_params(arr);
                Some(lab_components_to_color(
                    *components.first()?,
                    *components.get(1)?,
                    *components.get(2)?,
                    xw,
                    yw,
                    zw,
                    amin,
                    amax,
                    bmin,
                    bmax,
                ))
            }
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                let profile = resolve_object_clone(arr[1].as_ref().clone(), resolver)?;
                color_from_icc_components(&profile, components)
            }
            _ => None,
        },
        PDFObject::Ref(_) => {
            let resolved = resolve_object_clone(color_space.clone(), resolver)?;
            color_from_components(&resolved, components, resolver)
        }
        _ => None,
    }
}

fn components_for_color_space(
    color_space: &PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> usize {
    match color_space {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => 1,
            "DeviceRGB" | "RGB" => 3,
            "DeviceCMYK" | "CMYK" => 4,
            _ => 0,
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" => 3,
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                let profile = resolve_object_clone(arr[1].as_ref().clone(), resolver);
                profile
                    .as_ref()
                    .map(icc_based_component_count)
                    .unwrap_or_else(|| icc_based_component_count(arr[1].as_ref()))
            }
            _ => 0,
        },
        PDFObject::Ref(_) => {
            let resolved = resolve_object_clone(color_space.clone(), resolver);
            resolved
                .as_ref()
                .map(|resolved| components_for_color_space(resolved, resolver))
                .unwrap_or(0)
        }
        _ => 0,
    }
}

fn color_from_icc_based(
    profile_obj: &PDFObject,
    args: &[PDFObject],
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Color> {
    let profile = resolve_object_clone(profile_obj.clone(), resolver)?;
    let components: Vec<f64> = args
        .iter()
        .filter_map(|arg| match arg {
            PDFObject::Number(value) => Some(*value),
            _ => None,
        })
        .collect();

    #[cfg(feature = "rendering")]
    if let Some(color) = color_from_icc_components_with_profile(&profile, &components) {
        return Some(color);
    }

    if let PDFObject::Stream { dict, .. } = &profile {
        if let Some(alternate) = dict
            .get("Alternate")
            .and_then(|alt| resolve_object_clone(alt.clone(), resolver))
        {
            if let Some(color) = color_from_color_space(&alternate, args, resolver) {
                return Some(color);
            }
        }
    }

    color_from_icc_components(&profile, &components)
}

fn color_from_icc_components(profile_obj: &PDFObject, components: &[f64]) -> Option<Color> {
    #[cfg(feature = "rendering")]
    if let Some(color) = color_from_icc_components_with_profile(profile_obj, components) {
        return Some(color);
    }

    match icc_based_component_count(profile_obj) {
        1 => Some(Color::Gray(*components.first()?)),
        3 => Some(Color::RGB(
            *components.first()?,
            *components.get(1)?,
            *components.get(2)?,
        )),
        4 => Some(Color::CMYK(
            *components.first()?,
            *components.get(1)?,
            *components.get(2)?,
            *components.get(3)?,
        )),
        _ => None,
    }
}

#[cfg(feature = "rendering")]
fn color_from_icc_components_with_profile(
    profile_obj: &PDFObject,
    components: &[f64],
) -> Option<Color> {
    use moxcms::{ColorProfile, Layout, TransformOptions};

    let PDFObject::Stream { dict, data } = profile_obj else {
        return None;
    };
    let n = dict
        .get("N")
        .and_then(|value| value.as_number())
        .map(|value| value as usize)?;
    if components.len() < n {
        return None;
    }

    let profile_data = dict
        .get("Filter")
        .and_then(|filter| crate::core::decode::apply_filters(data, filter).ok())
        .unwrap_or_else(|| data.clone());

    if n == 3 && is_srgb_icc_profile(&profile_data) {
        return Some(Color::RGB(
            components[0].clamp(0.0, 1.0),
            components[1].clamp(0.0, 1.0),
            components[2].clamp(0.0, 1.0),
        ));
    }

    let src = ColorProfile::new_from_slice(&profile_data).ok()?;
    let src_layout = match n {
        1 => Layout::Gray,
        3 => Layout::Rgb,
        4 => Layout::Rgba,
        _ => return None,
    };
    let dst = ColorProfile::new_srgb();
    let transform = src
        .create_transform_f32(src_layout, &dst, Layout::Rgb, TransformOptions::default())
        .ok()?;
    let input: Vec<f32> = components.iter().take(n).map(|v| *v as f32).collect();
    let mut output = [0.0f32; 3];
    transform.transform(&input, &mut output).ok()?;
    Some(Color::RGB(
        output[0].clamp(0.0, 1.0) as f64,
        output[1].clamp(0.0, 1.0) as f64,
        output[2].clamp(0.0, 1.0) as f64,
    ))
}

#[cfg(feature = "rendering")]
fn is_srgb_icc_profile(profile: &[u8]) -> bool {
    profile.len() >= 128
        && profile.get(36..40) == Some(b"acsp")
        && profile.get(16..20) == Some(b"RGB ")
        && (profile
            .windows(b"sRGB IEC61966-2.1".len())
            .any(|window| window == b"sRGB IEC61966-2.1")
            || profile
                .windows(b"IEC 61966-2.1".len())
                .any(|window| window == b"IEC 61966-2.1"))
}

fn icc_based_component_count(profile_obj: &PDFObject) -> usize {
    match profile_obj {
        PDFObject::Stream { dict, .. } => dict
            .get("N")
            .and_then(|value| value.as_number())
            .map(|value| value as usize)
            .unwrap_or(0),
        _ => 0,
    }
}

fn eval_tint_function(
    function: &PDFObject,
    input: &[f64],
    output_count: usize,
) -> Option<Vec<f64>> {
    let dict = match function {
        PDFObject::Dictionary(dict) => dict,
        PDFObject::Stream { dict, .. } => dict,
        _ => return None,
    };
    match dict.get("FunctionType") {
        Some(PDFObject::Number(n)) if (*n as i32) == 0 => {
            let PDFObject::Stream { data, .. } = function else {
                return None;
            };
            let sampled = parse_sampled_function(dict, data);
            let mut output = sampled.eval_inputs(input);
            output.truncate(output_count);
            Some(output)
        }
        Some(PDFObject::Number(n)) if (*n as i32) == 2 => eval_type2_function(dict, input),
        Some(PDFObject::Number(n)) if (*n as i32) == 4 => {
            let PDFObject::Stream { data, .. } = function else {
                return None;
            };
            eval_type4_function(dict, data, input, output_count)
        }
        _ => None,
    }
}

fn eval_type2_function(dict: &HashMap<String, PDFObject>, input: &[f64]) -> Option<Vec<f64>> {
    // Ported from pdf.js `PDFFunctionFactory.constructInterpolated`
    // (`src/core/function.js`): y_j = C0_j + x^N * (C1_j - C0_j).
    let x = input.first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
    let c0 = number_array(dict.get("C0"))?;
    let c1 = number_array(dict.get("C1"))?;
    let n = match dict.get("N") {
        Some(PDFObject::Number(n)) => *n,
        _ => 1.0,
    };
    let scale = x.powf(n);
    Some(
        c0.iter()
            .zip(c1.iter())
            .map(|(start, end)| start + scale * (end - start))
            .collect(),
    )
}

fn eval_type4_function(
    _dict: &HashMap<String, PDFObject>,
    data: &[u8],
    input: &[f64],
    output_count: usize,
) -> Option<Vec<f64>> {
    // Minimal PostScript calculator evaluator for tint transforms, following
    // pdf.js `PostScriptEvaluator` behavior for stack arithmetic operators.
    let program = std::str::from_utf8(data).ok()?;
    let mut stack = input.to_vec();
    for token in program
        .replace(['{', '}'], " ")
        .split_whitespace()
        .filter(|token| !token.is_empty())
    {
        if let Ok(value) = token.parse::<f64>() {
            stack.push(value);
            continue;
        }
        match token {
            "dup" => stack.push(*stack.last()?),
            "exch" => {
                let len = stack.len();
                if len < 2 {
                    return None;
                }
                stack.swap(len - 1, len - 2);
            }
            "pop" => {
                stack.pop()?;
            }
            "add" => binary_stack_op(&mut stack, |a, b| a + b)?,
            "sub" => binary_stack_op(&mut stack, |a, b| a - b)?,
            "mul" => binary_stack_op(&mut stack, |a, b| a * b)?,
            "div" => binary_stack_op(&mut stack, |a, b| a / b)?,
            _ => return None,
        }
    }
    if stack.len() < output_count {
        return None;
    }
    Some(stack[stack.len() - output_count..].to_vec())
}

fn binary_stack_op(stack: &mut Vec<f64>, op: impl FnOnce(f64, f64) -> f64) -> Option<()> {
    let b = stack.pop()?;
    let a = stack.pop()?;
    stack.push(op(a, b));
    Some(())
}

fn number_array(object: Option<&PDFObject>) -> Option<Vec<f64>> {
    let PDFObject::Array(arr) = object? else {
        return None;
    };
    arr.iter()
        .map(|item| match &**item {
            PDFObject::Number(n) => Some(*n),
            _ => None,
        })
        .collect()
}

/// Resolve and expand an Indexed color space lookup table.
fn expand_indexed_cs(
    data: &[u8],
    cs_obj: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
    bits_per_component: u8,
    width: u32,
    height: u32,
) -> (Vec<u8>, bool) {
    // Try to resolve the ColorSpace and lookup table.
    let cs_obj = match cs_obj {
        Some(o) => o,
        None => return (data.to_vec(), false),
    };

    // Resolve indirect ref for the ColorSpace array
    let resolved_cs;
    let cs = match cs_obj {
        PDFObject::Ref(r) => {
            let res = match resolver.as_mut() {
                Some(r) => r,
                None => return (data.to_vec(), false),
            };
            match res.resolve_ref(r.num, r.generation) {
                Ok(o) => {
                    resolved_cs = o;
                    &*resolved_cs
                }
                Err(_) => return (data.to_vec(), false),
            }
        }
        other => other,
    };

    let cs_arr = match cs {
        PDFObject::Array(a) if a.len() >= 4 => a,
        _ => return (data.to_vec(), false),
    };

    if !matches!(&*cs_arr[0], PDFObject::Name(n) if n == "Indexed") {
        return (data.to_vec(), false);
    }

    let base_channels = match &*cs_arr[1] {
        PDFObject::Name(n) => match n.as_str() {
            "DeviceRGB" | "CalRGB" => 3,
            "DeviceGray" | "CalGray" => 1,
            "DeviceCMYK" => 4,
            _ => 3,
        },
        _ => 3,
    };

    // Get lookup table (inline String/HexString/Stream or Ref).
    let lut = match &*cs_arr[3] {
        PDFObject::String(s) | PDFObject::HexString(s) => s.clone(),
        PDFObject::Stream {
            dict,
            data: stream_data,
        } => {
            if let Some(filter) = dict.get("Filter") {
                crate::core::decode::apply_filters(stream_data, filter)
                    .unwrap_or_else(|_| stream_data.clone())
            } else {
                stream_data.clone()
            }
        }
        PDFObject::Ref(r) => {
            // Resolve the lookup table reference.
            let res = match resolver.as_mut() {
                Some(r) => r,
                None => return (data.to_vec(), false),
            };
            match res.resolve_ref(r.num, r.generation) {
                Ok(fetched) => match &*fetched {
                    PDFObject::String(s) | PDFObject::HexString(s) => s.clone(),
                    PDFObject::Stream {
                        dict,
                        data: stream_data,
                    } => {
                        if let Some(filter) = dict.get("Filter") {
                            crate::core::decode::apply_filters(stream_data, filter)
                                .unwrap_or_else(|_| stream_data.clone())
                        } else {
                            stream_data.clone()
                        }
                    }
                    _ => return (data.to_vec(), false),
                },
                Err(_) => return (data.to_vec(), false),
            }
        }
        _ => return (data.to_vec(), false),
    };

    let hival = cs_arr
        .get(2)
        .and_then(|item| item.as_number())
        .map(|value| value as u32)
        .unwrap_or(255);

    (
        expand_indexed(
            data,
            &lut,
            base_channels,
            bits_per_component,
            width,
            height,
            hival,
        ),
        true,
    )
}

fn is_indexed_color_space(
    cs_obj: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> bool {
    let Some(cs_obj) = cs_obj else {
        return false;
    };

    let resolved_cs;
    let cs = match cs_obj {
        PDFObject::Ref(r) => {
            let Some(resolver) = resolver.as_mut() else {
                return false;
            };
            match resolver.resolve_ref(r.num, r.generation) {
                Ok(object) => {
                    resolved_cs = object;
                    &*resolved_cs
                }
                Err(_) => return false,
            }
        }
        other => other,
    };

    matches!(
        cs,
        PDFObject::Array(arr)
            if matches!(arr.first().map(|item| item.as_ref()), Some(PDFObject::Name(name)) if name == "Indexed")
    )
}

#[derive(Debug, Clone, Copy)]
struct ImageColorSpaceInfo {
    name: Option<&'static str>,
    components: Option<u8>,
}

fn image_color_space_info(
    dict: &HashMap<String, PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> ImageColorSpaceInfo {
    let color_space = dict.get("ColorSpace").or_else(|| dict.get("CS"));
    image_color_space_obj_info(color_space, resolver)
}

fn image_color_space_obj_info(
    obj: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> ImageColorSpaceInfo {
    let Some(obj) = obj else {
        return ImageColorSpaceInfo {
            name: Some("DeviceRGB"),
            components: Some(3),
        };
    };

    match obj {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => ImageColorSpaceInfo {
                name: Some("DeviceGray"),
                components: Some(1),
            },
            "DeviceRGB" | "RGB" => ImageColorSpaceInfo {
                name: Some("DeviceRGB"),
                components: Some(3),
            },
            "DeviceCMYK" | "CMYK" => ImageColorSpaceInfo {
                name: Some("DeviceCMYK"),
                components: Some(4),
            },
            _ => ImageColorSpaceInfo {
                name: Some("DeviceRGB"),
                components: Some(3),
            },
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" => ImageColorSpaceInfo {
                name: Some("Lab"),
                components: Some(3),
            },
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                icc_based_image_color_space_info(arr[1].as_ref(), resolver)
            }
            Some(PDFObject::Name(name)) if name == "Indexed" => ImageColorSpaceInfo {
                name: Some("Indexed"),
                components: Some(1),
            },
            Some(PDFObject::Name(name)) if name == "DeviceN" && arr.len() >= 2 => {
                let components = match arr[1].as_ref() {
                    PDFObject::Array(names) => u8::try_from(names.len()).ok(),
                    _ => None,
                };
                ImageColorSpaceInfo {
                    name: Some("DeviceN"),
                    components,
                }
            }
            _ => ImageColorSpaceInfo {
                name: Some("DeviceRGB"),
                components: Some(3),
            },
        },
        PDFObject::Ref(_) => {
            let resolved = resolver
                .as_mut()
                .and_then(|resolver| resolver.resolve_if_ref(obj).ok());
            image_color_space_obj_info(resolved.as_deref(), resolver)
        }
        _ => ImageColorSpaceInfo {
            name: Some("DeviceRGB"),
            components: Some(3),
        },
    }
}

fn icc_based_image_color_space_info(
    profile_obj: &PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> ImageColorSpaceInfo {
    let resolved = match profile_obj {
        PDFObject::Ref(_) => resolver
            .as_mut()
            .and_then(|resolver| resolver.resolve_if_ref(profile_obj).ok()),
        _ => None,
    };
    let profile = resolved.as_deref().unwrap_or(profile_obj);
    let components = match profile {
        PDFObject::Stream { dict, .. } | PDFObject::Dictionary(dict) => dict
            .get("N")
            .and_then(PDFObject::as_number)
            .and_then(|n| u8::try_from(n as usize).ok()),
        _ => None,
    };
    let name = match components {
        Some(1) => Some("DeviceGray"),
        Some(3) => Some("DeviceRGB"),
        Some(4) => Some("DeviceCMYK"),
        _ => Some("ICCBased"),
    };
    ImageColorSpaceInfo { name, components }
}

fn jpeg_decode_options(
    dict: &HashMap<String, PDFObject>,
    components: Option<u8>,
    color_space_name: Option<&'static str>,
) -> crate::core::image::JpegDecodeOptions {
    let _ = color_space_name;
    let color_transform = dct_color_transform(dict);
    crate::core::image::JpegDecodeOptions {
        components,
        color_transform,
    }
}

fn dct_color_transform(dict: &HashMap<String, PDFObject>) -> Option<i32> {
    decode_parms_for_filter(dict, "DCTDecode")
        .or_else(|| decode_parms_for_filter(dict, "DCT"))
        .and_then(|parms| match parms {
            PDFObject::Dictionary(d) => d.get("ColorTransform"),
            _ => None,
        })
        .and_then(PDFObject::as_number)
        .map(|n| n as i32)
}

fn decode_parms_for_filter<'a>(
    dict: &'a HashMap<String, PDFObject>,
    filter_name: &str,
) -> Option<&'a PDFObject> {
    let decode_parms = dict.get("DecodeParms").or_else(|| dict.get("DP"))?;
    match decode_parms {
        PDFObject::Dictionary(_) => Some(decode_parms),
        PDFObject::Array(parms) => {
            let index = filter_index(dict.get("Filter").or_else(|| dict.get("F")), filter_name)?;
            parms.get(index).map(|item| item.as_ref())
        }
        _ => None,
    }
}

fn filter_index(filter: Option<&PDFObject>, filter_name: &str) -> Option<usize> {
    match filter? {
        PDFObject::Name(name) => {
            if name == filter_name {
                Some(0)
            } else {
                None
            }
        }
        PDFObject::Array(filters) => filters.iter().position(
            |filter| matches!(filter.as_ref(), PDFObject::Name(name) if name == filter_name),
        ),
        _ => None,
    }
}

fn filter_contains_any(filter: Option<&PDFObject>, names: &[&str]) -> bool {
    match filter {
        Some(PDFObject::Name(name)) => names.contains(&name.as_str()),
        Some(PDFObject::Array(filters)) => filters.iter().any(|filter| {
            matches!(filter.as_ref(), PDFObject::Name(name) if names.contains(&name.as_str()))
        }),
        _ => false,
    }
}

/// Apply PNG predictor from DecodeParms (handles both Dict and Array forms).
fn apply_predictor_from_parms(
    data: Vec<u8>,
    dict: &HashMap<String, PDFObject>,
    default_columns: u32,
) -> (Vec<u8>, Option<u8>) {
    let parms = match dict.get("DecodeParms").or_else(|| dict.get("DP")) {
        Some(p) => p,
        None => return (data, None),
    };
    let parms_dict = match parms {
        PDFObject::Dictionary(d) => Some(d),
        PDFObject::Array(arr) => arr.iter().find_map(|item| match &**item {
            PDFObject::Dictionary(d) => Some(d),
            _ => None,
        }),
        _ => None,
    };
    let parms_dict = match parms_dict {
        Some(d) => d,
        None => return (data, None),
    };
    let pred = match parms_dict.get("Predictor") {
        Some(PDFObject::Number(n)) => *n as i32,
        _ => return (data, None),
    };
    if pred <= 1 {
        return (data, None);
    }
    let colors = match parms_dict.get("Colors") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => 1,
    };
    let bpc = match parms_dict.get("BitsPerComponent") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => match dict.get("BitsPerComponent") {
            Some(PDFObject::Number(n)) => *n as usize,
            _ => 8,
        },
    };
    let image_bpc = match dict.get("BitsPerComponent") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => bpc,
    };
    let predictor_bpc = if bpc == image_bpc {
        u8::try_from(bpc).ok()
    } else {
        None
    };
    let cols = match parms_dict.get("Columns") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => default_columns as usize,
    };
    // Compute row sizes
    let row_bytes = (cols * colors * bpc + 7) / 8;
    let image_row_bytes = (default_columns as usize * colors * image_bpc + 7) / 8;
    let expected_image_bytes = match dict.get("Height") {
        Some(PDFObject::Number(h)) if bpc != image_bpc => {
            Some((*h as usize).saturating_mul(image_row_bytes))
        }
        _ => None,
    };
    if pred == 2 {
        let height = match dict.get("Height") {
            Some(PDFObject::Number(h)) => *h as usize,
            _ => 0,
        };
        let target = if let Some(expected) = expected_image_bytes {
            expected.div_ceil(row_bytes).saturating_mul(row_bytes)
        } else if height > 0 {
            height * row_bytes
        } else {
            (data.len() / row_bytes) * row_bytes
        };
        let mut aligned = data;
        if aligned.len() < target {
            aligned.resize(target, 0);
        } else {
            aligned.truncate(target);
        }
        let decoded = crate::core::decode::decode_tiff_predictor(&aligned, colors, bpc, cols)
            .unwrap_or(aligned);
        if let Some(expected) = expected_image_bytes {
            return (decoded.into_iter().take(expected).collect(), predictor_bpc);
        }
        return (decoded, predictor_bpc);
    }
    if pred < 10 {
        return (data, None);
    }

    let pred_stride = 1 + row_bytes; // stride with predictor byte per row
    let height = match dict.get("Height") {
        Some(PDFObject::Number(h)) => *h as usize,
        _ => 0,
    };

    let data_len = data.len();
    let pred_target = if let Some(expected) = expected_image_bytes {
        expected.div_ceil(row_bytes).saturating_mul(pred_stride)
    } else if height > 0 {
        (height * pred_stride).min(data_len)
    } else {
        (data_len / pred_stride) * pred_stride
    };
    let mut aligned = data;
    if aligned.len() < pred_target {
        aligned.resize(pred_target, 0);
    } else {
        aligned.truncate(pred_target);
    }

    // Try PNG predictor
    let decoded = crate::core::decode::decode_png_predictor(&aligned, colors, bpc, cols);
    match decoded {
        Ok(out) => {
            if let Some(expected) = expected_image_bytes {
                (out.into_iter().take(expected).collect(), predictor_bpc)
            } else {
                (out, predictor_bpc)
            }
        }
        Err(_) => {
            // Predictor failed — data likely has no PNG prediction.
            // Fall back to the exact raw pixel count; malformed streams can
            // declare Predictor=15 even when the decoded bytes are plain pixels.
            let raw_target = if height > 0 {
                height * row_bytes
            } else {
                (data_len / row_bytes) * row_bytes
            };
            aligned.truncate(raw_target.min(aligned.len()));
            (aligned, predictor_bpc)
        }
    }
}

/// Parse Lab color space parameters from the CS array.
fn parse_lab_params(cs_arr: &[Box<PDFObject>]) -> (f64, f64, f64, f64, f64, f64, f64) {
    let mut xw = 1.0;
    let mut yw = 1.0;
    let mut zw = 1.0;
    let mut amin = -100.0;
    let mut amax = 100.0;
    let mut bmin = -100.0;
    let mut bmax = 100.0;

    if cs_arr.len() >= 2 {
        if let PDFObject::Dictionary(d) = &*cs_arr[1] {
            if let Some(PDFObject::Array(wp)) = d.get("WhitePoint") {
                if wp.len() >= 3 {
                    if let (PDFObject::Number(x), PDFObject::Number(y), PDFObject::Number(z)) =
                        (&*wp[0], &*wp[1], &*wp[2])
                    {
                        xw = *x;
                        yw = *y;
                        zw = *z;
                    }
                }
            }
            if let Some(PDFObject::Array(range)) = d.get("Range") {
                if range.len() >= 4 {
                    if let (
                        PDFObject::Number(a0),
                        PDFObject::Number(a1),
                        PDFObject::Number(b0),
                        PDFObject::Number(b1),
                    ) = (&*range[0], &*range[1], &*range[2], &*range[3])
                    {
                        amin = *a0;
                        amax = *a1;
                        bmin = *b0;
                        bmax = *b1;
                    }
                }
            }
        }
    }
    (xw, yw, zw, amin, amax, bmin, bmax)
}

/// Convert Lab pixel data to RGB. Reference: pdf.js/src/core/colorspace.js — LabCS.#toRgb
fn lab_to_rgb(
    data: &[u8],
    xw: f64,
    yw: f64,
    zw: f64,
    amin: f64,
    amax: f64,
    bmin: f64,
    bmax: f64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(3) {
        if chunk.len() < 3 {
            out.extend_from_slice(chunk);
            continue;
        }
        // Decode from 0-255 to Lab ranges
        let l_star = chunk[0] as f64 * 100.0 / 255.0;
        let a_star = amin + chunk[1] as f64 * (amax - amin) / 255.0;
        let b_star = bmin + chunk[2] as f64 * (bmax - bmin) / 255.0;

        let (r, g, b) =
            lab_components_to_rgb_u8(l_star, a_star, b_star, xw, yw, zw, amin, amax, bmin, bmax);
        out.push(r);
        out.push(g);
        out.push(b);
    }
    out
}

fn lab_components_to_color(
    l_star: f64,
    a_star: f64,
    b_star: f64,
    xw: f64,
    yw: f64,
    zw: f64,
    amin: f64,
    amax: f64,
    bmin: f64,
    bmax: f64,
) -> Color {
    let (r, g, b) =
        lab_components_to_rgb_u8(l_star, a_star, b_star, xw, yw, zw, amin, amax, bmin, bmax);
    Color::RGB(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

fn lab_components_to_rgb_u8(
    l_star: f64,
    a_star: f64,
    b_star: f64,
    xw: f64,
    yw: f64,
    zw: f64,
    amin: f64,
    amax: f64,
    bmin: f64,
    bmax: f64,
) -> (u8, u8, u8) {
    let fn_g = |x: f64| -> f64 {
        if x >= 6.0 / 29.0 {
            x * x * x
        } else {
            (108.0 / 841.0) * (x - 4.0 / 29.0)
        }
    };

    let a_star = a_star.clamp(amin, amax);
    let b_star = b_star.clamp(bmin, bmax);

    #[cfg(feature = "rendering")]
    if let Some(rgb) =
        crate::rendering::lab_icc::lab_pixel_to_rgb(l_star, a_star, b_star, xw, yw, zw)
    {
        return rgb;
    }

    let m = (l_star + 16.0) / 116.0;
    let l = m + a_star / 500.0;
    let n = m - b_star / 200.0;

    let x = xw * fn_g(l);
    let y = yw * fn_g(m);
    let z = zw * fn_g(n);

    // Ported from pdf.js LabCS.#toRgb.
    let (r, g, b) = if zw < 1.0 {
        (
            x * 3.1339 + y * -1.617 + z * -0.4906,
            x * -0.9785 + y * 1.916 + z * 0.0333,
            x * 0.072 + y * -0.229 + z * 1.4057,
        )
    } else {
        (
            x * 3.2406 + y * -1.5372 + z * -0.4986,
            x * -0.9689 + y * 1.8758 + z * 0.0415,
            x * 0.0557 + y * -0.204 + z * 1.057,
        )
    };

    (
        (r.max(0.0).sqrt() * 255.0).clamp(0.0, 255.0) as u8,
        (g.max(0.0).sqrt() * 255.0).clamp(0.0, 255.0) as u8,
        (b.max(0.0).sqrt() * 255.0).clamp(0.0, 255.0) as u8,
    )
}

/// Each sample in `data` is an index into `lut`, which contains
/// `base_channels` bytes per entry. Indexed images may pack indices at
/// 1/2/4 bpc; PDF.js/hayro unpack those samples before lookup.
fn expand_indexed(
    data: &[u8],
    lut: &[u8],
    base_channels: usize,
    bits_per_component: u8,
    width: u32,
    height: u32,
    hival: u32,
) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let row_bytes = (width.saturating_mul(bits_per_component as usize) + 7) / 8;
    let sample_count = width.saturating_mul(height);
    let mut out = Vec::with_capacity(sample_count.saturating_mul(base_channels));
    for y in 0..height {
        let row_start = y.saturating_mul(row_bytes);
        let row = data
            .get(row_start..row_start.saturating_add(row_bytes))
            .unwrap_or(&[]);
        for x in 0..width {
            let idx = read_packed_index_sample(row, x, bits_per_component).min(hival);
            let offset = idx as usize * base_channels;
            for ch in 0..base_channels {
                out.push(lut.get(offset + ch).copied().unwrap_or(0));
            }
        }
    }
    out
}

fn read_packed_index_sample(data: &[u8], sample_index: usize, bits_per_component: u8) -> u32 {
    match bits_per_component {
        1 | 2 | 4 => {
            let bit_offset = sample_index.saturating_mul(bits_per_component as usize);
            let byte = data.get(bit_offset / 8).copied().unwrap_or(0) as u32;
            let bit_in_byte = bit_offset % 8;
            let shift = 8usize.saturating_sub(bit_in_byte + bits_per_component as usize);
            (byte >> shift) & ((1u32 << bits_per_component) - 1)
        }
        _ => data.get(sample_index).copied().unwrap_or(0) as u32,
    }
}

fn image_smask_alpha(
    color_data: &[u8],
    color_space_name: Option<&str>,
    width: u32,
    height: u32,
    smask_obj: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Vec<u8>> {
    let channels = match color_space_name {
        Some("DeviceGray") | Some("CalGray") => 1,
        Some("DeviceCMYK") => 4,
        _ => 3,
    };
    let pixel_count = width as usize * height as usize;
    if color_data.len() < pixel_count.saturating_mul(channels) {
        return None;
    }

    let alpha = decode_smask_image(smask_obj?, width, height, resolver)?;
    if alpha.len() < pixel_count {
        return None;
    }

    let mut rgba = Vec::with_capacity(pixel_count * 4);
    match channels {
        1 => {
            for (gray, alpha) in color_data.iter().take(pixel_count).zip(alpha.iter()) {
                rgba.extend_from_slice(&[*gray, *gray, *gray, *alpha]);
            }
        }
        4 => {
            for (pixel, alpha) in color_data
                .chunks_exact(4)
                .take(pixel_count)
                .zip(alpha.iter())
            {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], *alpha]);
            }
        }
        _ => {
            for (pixel, alpha) in color_data
                .chunks_exact(3)
                .take(pixel_count)
                .zip(alpha.iter())
            {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], *alpha]);
            }
        }
    }
    Some(rgba)
}

fn decode_smask_image(
    smask_obj: &PDFObject,
    fallback_width: u32,
    fallback_height: u32,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Vec<u8>> {
    let resolved;
    let smask = match smask_obj {
        PDFObject::Ref(r) => {
            let res = resolver.as_mut()?;
            resolved = res.resolve_ref(r.num, r.generation).ok()?;
            &*resolved
        }
        other => other,
    };

    let PDFObject::Stream { dict, data } = smask else {
        return None;
    };

    if !matches!(dict.get("Subtype"), Some(PDFObject::Name(name)) if name == "Image") {
        return None;
    }

    let width = match dict.get("Width") {
        Some(PDFObject::Number(w)) => *w as u32,
        _ => fallback_width,
    };
    let height = match dict.get("Height") {
        Some(PDFObject::Number(h)) => *h as u32,
        _ => fallback_height,
    };
    if width == 0 || height == 0 || fallback_width == 0 || fallback_height == 0 {
        return None;
    }

    let bpc = match dict.get("BitsPerComponent") {
        Some(PDFObject::Number(b)) => *b as u8,
        _ => 8,
    };

    let filter = dict.get("Filter").or_else(|| dict.get("F"));
    let mut decoded = if let Some(filter) = filter {
        crate::core::decode::apply_filters(data, filter).unwrap_or_else(|_| data.clone())
    } else {
        data.clone()
    };
    let mut bpc = bpc;
    let mut decoded_width = width;
    let mut decoded_height = height;

    if filter_contains_any(filter, &["DCTDecode", "DCT"]) {
        #[cfg(feature = "jpeg-decoding")]
        {
            let color_info = image_color_space_info(dict, &mut None);
            if let Ok(image) = crate::core::image::ImageDecoder::decode_pdf_jpeg(
                &decoded,
                jpeg_decode_options(dict, color_info.components, color_info.name),
            ) {
                decoded = image.data;
                decoded_width = image.metadata.width;
                decoded_height = image.metadata.height;
                bpc = 8;
            }
        }
    } else if filter_contains_any(filter, &["CCITTFaxDecode", "CCF"]) {
        #[cfg(feature = "hayro-ccitt")]
        {
            decoded = decode_ccitt(&decoded, dict, width, height)?;
            bpc = 1;
        }
    } else if filter_contains_any(filter, &["JPXDecode"]) {
        #[cfg(feature = "advanced-image-formats")]
        {
            let color_info = {
                let mut resolver = None;
                image_color_space_info(dict, &mut resolver)
            };
            let (data, jpx_width, jpx_height, jpx_bpc, _) =
                decode_jpx(&decoded, jpx_decode_options(dict, color_info.components))?;
            decoded = data;
            decoded_width = jpx_width;
            decoded_height = jpx_height;
            bpc = jpx_bpc;
        }
    } else if filter_contains_any(filter, &["JBIG2Decode"]) {
        #[cfg(feature = "advanced-image-formats")]
        {
            // Inline images rarely carry JBIG2Globals; pass None.
            decoded = decode_jbig2(&decoded, None)?;
            bpc = 1;
        }
    }

    let (decoded, predictor_bpc) = apply_predictor_from_parms(decoded, dict, decoded_width);
    let bpc = predictor_bpc.unwrap_or(bpc);

    // Soft masks are treated as single-channel coverage. Multi-channel sources
    // (e.g. accidental RGB after JPEG) keep the first channel as gray.
    let gray = match bpc {
        1 => expand_1bpc_image_to_8bpc(&decoded, decoded_width, decoded_height, 255),
        8 => {
            let need = decoded_width as usize * decoded_height as usize;
            if decoded.len() >= need * 3 && decoded.len() != need {
                // RGB/gray-padded: take R (or gray) channel only.
                let stride = if decoded.len() >= need * 4 { 4 } else { 3 };
                decoded
                    .chunks_exact(stride)
                    .take(need)
                    .map(|c| c[0])
                    .collect()
            } else {
                decoded
            }
        }
        16 => downscale_16_to_8(&decoded),
        _ => return None,
    };

    let mut alpha = if let Some(PDFObject::Array(decode_arr)) = dict.get("Decode") {
        apply_decode_inversion(&gray, decode_arr, 8)
    } else {
        gray
    };

    // PDF 1.7 §11.6.5.2 / §8.9.6.3: when soft-mask dimensions differ from the
    // parent image, scale mask samples to the parent size. InDesign/Acrobat
    // often ship 2× SMask bitmaps (tile_clamp_bug photos).
    // Source: PDF 32000-1:2008 §11.6.5.2; hayro soft-mask image path.
    if decoded_width != fallback_width || decoded_height != fallback_height {
        alpha = scale_gray_nearest(
            &alpha,
            decoded_width,
            decoded_height,
            fallback_width,
            fallback_height,
        )?;
    }

    let need = fallback_width as usize * fallback_height as usize;
    if alpha.len() < need {
        return None;
    }
    alpha.truncate(need);
    Some(alpha)
}

/// Nearest-neighbor scale of an 8-bit grayscale buffer (soft-mask coverage).
fn scale_gray_nearest(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Option<Vec<u8>> {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return None;
    }
    let sw = src_w as usize;
    let sh = src_h as usize;
    if src.len() < sw.saturating_mul(sh) {
        return None;
    }
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    let mut out = vec![0u8; dw * dh];
    for y in 0..dh {
        let sy = (y as u64 * sh as u64 / dh as u64) as usize;
        let src_row = sy * sw;
        let dst_row = y * dw;
        for x in 0..dw {
            let sx = (x as u64 * sw as u64 / dw as u64) as usize;
            out[dst_row + x] = src[src_row + sx];
        }
    }
    Some(out)
}

fn decode_inline_image_data(
    data: &[u8],
    dict: &HashMap<String, PDFObject>,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let filter = dict.get("Filter")?;
    let bits_per_component = dict
        .get("BitsPerComponent")
        .and_then(PDFObject::as_number)
        .unwrap_or(8.0) as u8;

    match filter {
        PDFObject::Name(name) => match name.as_str() {
            "FlateDecode" | "Fl" => {
                let decoded = crate::core::decode::decode_flate(data).ok()?;
                Some(apply_predictor_from_parms(decoded, dict, width).0)
            }
            "LZWDecode" | "LZW" => {
                let decoded = crate::core::decode::decode_lzw(data).ok()?;
                Some(apply_predictor_from_parms(decoded, dict, width).0)
            }
            // Type3 glyph ImageMasks from Ghostscript/dvips often use bare
            // ASCIIHex (`/F/AHx`) rather than a filter array. Without decoding,
            // hex digits are painted as raw 1-bpp samples and scramble the
            // glyph (font_type3_widths_with_matrix).
            // Source: PDF 1.7 §7.4.2; hayro stream decoded_image path.
            "ASCIIHexDecode" | "AHx" => crate::core::decode::decode_ascii_hex(data).ok(),
            "ASCII85Decode" | "A85" => crate::core::decode::decode_ascii85(data).ok(),
            "RunLengthDecode" | "RL" => crate::core::decode::decode_run_length(data).ok(),
            "CCITTFaxDecode" | "CCF" => {
                #[cfg(feature = "hayro-ccitt")]
                {
                    decode_ccitt(data, dict, width, height)
                }
                #[cfg(not(feature = "hayro-ccitt"))]
                {
                    None
                }
            }
            "DCTDecode" | "DCT" => {
                #[cfg(feature = "jpeg-decoding")]
                {
                    let color_info = image_color_space_info(dict, &mut None);
                    crate::core::image::ImageDecoder::decode_pdf_jpeg(
                        data,
                        jpeg_decode_options(dict, color_info.components, color_info.name),
                    )
                    .ok()
                    .map(|image| image.data)
                }
                #[cfg(not(feature = "jpeg-decoding"))]
                {
                    None
                }
            }
            _ => None,
        },
        PDFObject::Array(filters) => {
            let decoded = crate::core::decode::apply_filters(data, filter).ok()?;
            let filter_names: Vec<&str> = filters
                .iter()
                .filter_map(|filter| match filter.as_ref() {
                    PDFObject::Name(name) => Some(name.as_str()),
                    _ => None,
                })
                .collect();

            if filter_names.contains(&"CCITTFaxDecode") || filter_names.contains(&"CCF") {
                #[cfg(feature = "hayro-ccitt")]
                {
                    decode_ccitt(&decoded, dict, width, height)
                }
                #[cfg(not(feature = "hayro-ccitt"))]
                {
                    None
                }
            } else if filter_names.contains(&"DCTDecode") || filter_names.contains(&"DCT") {
                #[cfg(feature = "jpeg-decoding")]
                {
                    let color_info = image_color_space_info(dict, &mut None);
                    crate::core::image::ImageDecoder::decode_pdf_jpeg(
                        &decoded,
                        jpeg_decode_options(dict, color_info.components, color_info.name),
                    )
                    .ok()
                    .map(|image| image.data)
                }
                #[cfg(not(feature = "jpeg-decoding"))]
                {
                    None
                }
            } else if bits_per_component > 0 {
                Some(apply_predictor_from_parms(decoded, dict, width).0)
            } else {
                Some(decoded)
            }
        }
        _ => None,
    }
}

/// Downscale 16-bit pixel data to 8-bit (take high byte of each 16-bit sample).
fn downscale_16_to_8(data: &[u8]) -> Vec<u8> {
    data.chunks_exact(2).map(|pair| pair[0]).collect()
}

fn should_expand_1bpc_sampled_image(filter: Option<&PDFObject>) -> bool {
    match filter {
        None => true,
        Some(PDFObject::Name(name)) => {
            matches!(
                name.as_str(),
                "FlateDecode" | "Fl" | "LZWDecode" | "LZW" | "CCITTFaxDecode" | "CCF"
            )
        }
        Some(PDFObject::Array(filters)) => filters.iter().all(|filter| {
            matches!(
                filter.as_ref(),
                PDFObject::Name(name)
                    if !matches!(
                        name.as_str(),
                        "DCTDecode"
                            | "DCT"
                            | "JPXDecode"
                            | "JBIG2Decode"
                    )
            )
        }),
        _ => false,
    }
}

/// Expand a 1-bit sampled image to one byte per pixel.
///
/// DeviceGray-style images use `one_value = 255`; Indexed images use `one_value = 1`
/// so palette expansion still receives valid lookup indices.
fn expand_1bpc_image_to_8bpc(data: &[u8], width: u32, height: u32, one_value: u8) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let row_bytes = (width + 7) / 8;
    let mut out = Vec::with_capacity(width.saturating_mul(height));

    for y in 0..height {
        let row_start = y.saturating_mul(row_bytes);
        let row = data
            .get(row_start..row_start.saturating_add(row_bytes))
            .unwrap_or(&[]);
        for x in 0..width {
            let byte = row.get(x / 8).copied().unwrap_or(0);
            let bit = 7 - (x % 8);
            let sample = (byte >> bit) & 1;
            out.push(if sample == 0 { 0 } else { one_value });
        }
    }

    out
}

fn image_mask_paint_one(dict: &HashMap<String, PDFObject>) -> bool {
    if let Some(PDFObject::Array(decode)) = dict.get("Decode") {
        return match (decode.first(), decode.get(1)) {
            (Some(a), Some(b)) => match (a.as_ref(), b.as_ref()) {
                (PDFObject::Number(min), PDFObject::Number(max)) => *min > *max,
                _ => false,
            },
            _ => false,
        };
    }

    false
}

fn image_mask_default_decode(dict: &HashMap<String, PDFObject>) -> bool {
    match dict.get("Decode") {
        Some(PDFObject::Array(decode)) => match decode.first() {
            Some(first) => match first.as_ref() {
                PDFObject::Number(value) => *value == 0.0,
                _ => false,
            },
            None => false,
        },
        None => true,
        _ => false,
    }
}

fn normalize_pdfium_default_ccitt_image_mask(
    mut data: Vec<u8>,
    is_image_mask: bool,
    dict: &HashMap<String, PDFObject>,
) -> Vec<u8> {
    if is_image_mask
        && image_mask_default_decode(dict)
        && !ccitt_black_is_1(dict)
        && filter_contains_any(dict.get("Filter"), &["CCITTFaxDecode", "CCF"])
    {
        for byte in &mut data {
            *byte = !*byte;
        }
    }
    data
}

fn ccitt_black_is_1(dict: &HashMap<String, PDFObject>) -> bool {
    let Some(parms) = dict.get("DecodeParms").or_else(|| dict.get("DP")) else {
        return false;
    };

    match parms {
        PDFObject::Dictionary(parms) => {
            matches!(parms.get("BlackIs1"), Some(PDFObject::Boolean(true)))
        }
        PDFObject::Array(items) => {
            let Some(index) =
                ccitt_filter_index_for_object(dict.get("Filter").or_else(|| dict.get("F")))
            else {
                return false;
            };
            matches!(
                items.get(index).map(|item| item.as_ref()),
                Some(PDFObject::Dictionary(parms))
                    if matches!(parms.get("BlackIs1"), Some(PDFObject::Boolean(true)))
            )
        }
        _ => false,
    }
}

fn ccitt_filter_index_for_object(filter: Option<&PDFObject>) -> Option<usize> {
    match filter? {
        PDFObject::Name(name) => matches!(name.as_str(), "CCITTFaxDecode" | "CCF").then_some(0),
        PDFObject::Array(filters) => filters.iter().position(|filter| {
            matches!(
                filter.as_ref(),
                PDFObject::Name(name) if matches!(name.as_str(), "CCITTFaxDecode" | "CCF")
            )
        }),
        _ => None,
    }
}

/// Apply /Decode array inversion. Each pair [min max] in the array
/// controls one component. When [1 0], the component is inverted (255 - v).
/// When [0 1] (default), no change. Only 8-bpc is handled.
fn apply_decode_inversion(data: &[u8], decode: &[Box<PDFObject>], bpc: u8) -> Vec<u8> {
    if bpc != 8 {
        return data.to_vec();
    }

    // Determine which channels are inverted ([1 0] = inverted).
    let channels = decode.len() / 2;
    let mut inverted = Vec::with_capacity(channels);
    for i in 0..channels {
        let is_inverted = match (decode.get(i * 2), decode.get(i * 2 + 1)) {
            (Some(b), Some(c)) => match (&**b, &**c) {
                (PDFObject::Number(min), PDFObject::Number(max)) => *min == 1.0 && *max == 0.0,
                _ => false,
            },
            _ => false,
        };
        inverted.push(is_inverted);
    }

    if !inverted.iter().any(|&b| b) {
        return data.to_vec(); // No inversions needed.
    }

    let mut result = data.to_vec();
    for (i, byte) in result.iter_mut().enumerate() {
        let ch = i % channels;
        if ch < inverted.len() && inverted[ch] {
            *byte = 255 - *byte;
        }
    }
    result
}

/// Post-concatenate a translation matrix.
///
/// Equivalent to `matrix * translate(tx, ty)` in PDF 2D affine form.
fn post_concat_translate(matrix: [f64; 6], tx: f64, ty: f64) -> [f64; 6] {
    post_concat_matrix(matrix, [1.0, 0.0, 0.0, 1.0, tx, ty])
}

/// Multiply two PDF affine transforms in 6-element form.
///
/// Returns `left * right`.
fn post_concat_matrix(left: [f64; 6], right: [f64; 6]) -> [f64; 6] {
    let [a1, b1, c1, d1, e1, f1] = left;
    let [a2, b2, c2, d2, e2, f2] = right;

    [
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * e2 + c1 * f2 + e1,
        b1 * e2 + d1 * f2 + f1,
    ]
}

fn remap_text_bytes_with_cmap(bytes: &[u8], cmap: &CMap) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        return None;
    }

    let one_byte_codes: Vec<u16> = bytes.iter().map(|&b| b as u16).collect();
    let one_byte_hits = one_byte_codes
        .iter()
        .filter(|&&code| cmap.to_unicode(code).is_some())
        .count();

    let mut best_codes = one_byte_codes;
    let mut best_hits = one_byte_hits;
    let mut best_is_two_byte = false;

    if bytes.len() % 2 == 0 {
        let two_byte_codes: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        let two_byte_hits = two_byte_codes
            .iter()
            .filter(|&&code| cmap.to_unicode(code).is_some())
            .count();

        if two_byte_hits > best_hits {
            best_codes = two_byte_codes;
            best_hits = two_byte_hits;
            best_is_two_byte = true;
        }
    }

    if best_hits == 0 {
        return None;
    }

    let mapped_chars: Vec<char> = best_codes
        .iter()
        .map(|&code| {
            cmap.to_unicode(code)
                .or_else(|| char::from_u32(code as u32))
                .unwrap_or('\u{FFFD}')
        })
        .collect();

    if mapped_chars.iter().all(|&ch| (ch as u32) <= 0xFF) {
        let mapped_single_byte: Vec<u8> = mapped_chars.iter().map(|&ch| ch as u8).collect();
        if best_is_two_byte || mapped_single_byte != bytes {
            return Some(mapped_single_byte);
        }
        return None;
    }

    let mut mapped_utf16be = Vec::new();
    for unicode_char in mapped_chars {
        let mut utf16_buf = [0u16; 2];
        let units = unicode_char.encode_utf16(&mut utf16_buf);
        for unit in units.iter() {
            mapped_utf16be.extend_from_slice(&unit.to_be_bytes());
        }
    }

    if best_is_two_byte || mapped_utf16be != bytes {
        Some(mapped_utf16be)
    } else {
        None
    }
}

// Image decoder helpers

#[cfg(feature = "hayro-ccitt")]
fn decode_ccitt(
    data: &[u8],
    dict: &HashMap<String, PDFObject>,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    use hayro_ccitt::{self, DecodeSettings, EncodingMode};
    let parms = ccitt_decode_parms(dict);
    let k = num_in(parms, "K").unwrap_or(0);
    let columns = num_in(parms, "Columns").unwrap_or(width as i64) as u32;
    let end_of_block = bool_in(parms, "EndOfBlock").unwrap_or(true);
    let mut rows = num_in(parms, "Rows").unwrap_or(height as i64) as u32;
    if end_of_block {
        rows = u32::MAX;
    }
    let settings = DecodeSettings {
        columns,
        rows,
        end_of_block,
        end_of_line: bool_in(parms, "EndOfLine").unwrap_or(false),
        rows_are_byte_aligned: bool_in(parms, "EncodedByteAlign").unwrap_or(false),
        encoding: if k < 0 {
            EncodingMode::Group4
        } else if k > 0 {
            EncodingMode::Group3_2D { k: k as u32 }
        } else {
            EncodingMode::Group3_1D
        },
        invert_black: bool_in(parms, "BlackIs1").unwrap_or(false),
    };
    let mut sink = BitmapSink::new();
    let _ = hayro_ccitt::decode(data, &mut sink, &settings);
    Some(sink.finish())
}

#[cfg(feature = "advanced-image-formats")]
#[derive(Debug, Clone, Copy)]
struct JpxDecodeOptions {
    pdf_components: Option<u8>,
    smask_in_data: bool,
}

#[cfg(feature = "advanced-image-formats")]
fn jpx_decode_options(
    dict: &HashMap<String, PDFObject>,
    pdf_components: Option<u8>,
) -> JpxDecodeOptions {
    let pdf_components = if dict.contains_key("ColorSpace") || dict.contains_key("CS") {
        pdf_components
    } else {
        None
    };
    let smask_in_data = dict
        .get("SMaskInData")
        .and_then(PDFObject::as_number)
        .is_some_and(|n| n as i64 == 1);
    JpxDecodeOptions {
        pdf_components,
        smask_in_data,
    }
}

#[cfg(feature = "advanced-image-formats")]
fn decode_jpx(data: &[u8], options: JpxDecodeOptions) -> Option<(Vec<u8>, u32, u32, u8, bool)> {
    use hayro_jpeg2000::ColorSpace;
    use image::ImageDecoder;

    let settings = hayro_jpeg2000::DecodeSettings {
        resolve_palette_indices: false,
        strict: false,
        target_resolution: None,
    };
    let image = hayro_jpeg2000::Image::new(data, &settings).ok()?;
    let width = image.width();
    let height = image.height();
    let color_components = match image.color_space() {
        ColorSpace::Gray => 1,
        ColorSpace::RGB => 3,
        ColorSpace::CMYK => 4,
        ColorSpace::Unknown { num_channels } => *num_channels,
        ColorSpace::Icc { num_channels, .. } => *num_channels,
    };

    if let Some(pdf_components) = options.pdf_components {
        let bpc = image.original_bit_depth();
        let has_alpha = image.has_alpha();
        let decoded = image.decode().ok()?;

        if pdf_components != color_components
            && !(pdf_components == 3 && color_components == 3 && has_alpha)
        {
            return None;
        }

        let decoded = if has_alpha && pdf_components == 3 && !options.smask_in_data {
            let channels = color_components as usize + 1;
            let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
            for sample in decoded.chunks_exact(channels) {
                rgb.extend_from_slice(&sample[..3]);
            }
            rgb
        } else if bpc > 8 {
            decoded.chunks_exact(2).map(|pair| pair[0]).collect()
        } else {
            decoded
        };

        return Some((
            decoded,
            width,
            height,
            8,
            has_alpha && options.smask_in_data,
        ));
    }

    let has_alpha = matches!(
        image.color_type(),
        image::ColorType::La8 | image::ColorType::Rgba8
    );
    let total_bytes = image.total_bytes().try_into().ok()?;
    let mut decoded = vec![0; total_bytes];
    image.read_image(&mut decoded).ok()?;

    Some((decoded, width, height, 8, has_alpha))
}

/// Decode a PDF-embedded JBIG2 image stream.
///
/// Uses hayro-jbig2's PDF.js-style decoder (bare segments + optional globals,
/// including SymbolDictionary / TextRegion). Falls back to the standalone
/// file decoder when the input carries a JBIG2 file header.
///
/// Source: hayro-jbig2 `pdf_stream.rs` ← PDF.js `core/jbig2.js`.
#[cfg(feature = "advanced-image-formats")]
fn decode_jbig2(data: &[u8], globals: Option<&[u8]>) -> Option<Vec<u8>> {
    // Prefer the PDF-aware decoder (handles bare streams + JBIG2Globals).
    if let Some(bitmap) = crate::core::jbig2_pdf::decode_with_globals(data, globals) {
        return Some(bitmap);
    }
    // Fallback: standalone JBIG2 file (with file header).
    // Polarity: hayro-jbig2 true=black; DeviceGray wants bit0=black.
    let image = hayro_jbig2::decode(data).ok()?;
    let width = image.width;
    let height = image.height;
    let row_bytes = ((width + 7) / 8) as usize;
    let mut bitmap = vec![0u8; row_bytes * height as usize];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) as usize;
            // true = black → leave bit 0; false = white → set bit 1
            if i < image.data.len() && !image.data[i] {
                let byte_i = y as usize * row_bytes + (x as usize / 8);
                bitmap[byte_i] |= 1 << (7 - (x % 8));
            }
        }
    }
    Some(bitmap)
}

/// Resolve `/DecodeParms` → `/JBIG2Globals` stream data (already filter-decoded).
///
/// DecodeParms may be a dict or an array parallel to the Filter array
/// (ISO 32000-1 §7.4.7, Table 12).
#[cfg(feature = "advanced-image-formats")]
fn jbig2_globals_data(
    dict: &HashMap<String, PDFObject>,
    filter: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Vec<u8>> {
    let parms = dict.get("DecodeParms").or_else(|| dict.get("DP"))?;
    let parms_dict = match parms {
        PDFObject::Dictionary(d) => d,
        PDFObject::Array(arr) => {
            let idx = jbig2_filter_index(filter)?;
            match arr.get(idx).map(|o| o.as_ref()) {
                Some(PDFObject::Dictionary(d)) => d,
                Some(PDFObject::Null) | None => return None,
                Some(PDFObject::Ref(r)) => {
                    let res = resolver.as_mut()?;
                    let fetched = res.resolve_ref(r.num, r.generation).ok()?;
                    match &*fetched {
                        PDFObject::Dictionary(d) => {
                            // Need owned data from the stream below; resolve globals from this dict.
                            return resolve_jbig2_globals_from_dict(d, resolver);
                        }
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        PDFObject::Ref(r) => {
            let res = resolver.as_mut()?;
            let fetched = res.resolve_ref(r.num, r.generation).ok()?;
            match &*fetched {
                PDFObject::Dictionary(d) => {
                    return resolve_jbig2_globals_from_dict(d, resolver);
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    resolve_jbig2_globals_from_dict(parms_dict, resolver)
}

#[cfg(feature = "advanced-image-formats")]
fn jbig2_filter_index(filter: Option<&PDFObject>) -> Option<usize> {
    match filter? {
        PDFObject::Name(name) if name == "JBIG2Decode" => Some(0),
        PDFObject::Array(filters) => filters
            .iter()
            .position(|f| matches!(f.as_ref(), PDFObject::Name(n) if n == "JBIG2Decode")),
        _ => None,
    }
}

#[cfg(feature = "advanced-image-formats")]
fn resolve_jbig2_globals_from_dict(
    parms: &HashMap<String, PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Vec<u8>> {
    let globals_obj = parms.get("JBIG2Globals")?;
    match globals_obj {
        PDFObject::Stream { dict, data } => {
            if let Some(filter) = dict.get("Filter") {
                crate::core::decode::apply_filters(data, filter).ok()
            } else {
                Some(data.clone())
            }
        }
        PDFObject::Ref(r) => {
            let res = resolver.as_mut()?;
            let resolved = res.resolve_ref(r.num, r.generation).ok()?;
            match &*resolved {
                PDFObject::Stream { dict, data } => {
                    if let Some(filter) = dict.get("Filter") {
                        crate::core::decode::apply_filters(data, filter).ok()
                    } else {
                        Some(data.clone())
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(feature = "hayro-ccitt")]
fn ccitt_decode_parms(dict: &HashMap<String, PDFObject>) -> Option<&PDFObject> {
    let decode_parms = dict.get("DecodeParms").or_else(|| dict.get("DP"))?;
    match decode_parms {
        PDFObject::Dictionary(_) => Some(decode_parms),
        PDFObject::Array(parms) => {
            let index = ccitt_filter_index(dict.get("Filter").or_else(|| dict.get("F")))?;
            parms.get(index).and_then(|item| match item.as_ref() {
                PDFObject::Dictionary(_) => Some(item.as_ref()),
                _ => None,
            })
        }
        _ => None,
    }
}

#[cfg(feature = "hayro-ccitt")]
fn ccitt_filter_index(filter: Option<&PDFObject>) -> Option<usize> {
    match filter? {
        PDFObject::Name(name) => {
            if matches!(name.as_str(), "CCITTFaxDecode" | "CCF") {
                Some(0)
            } else {
                None
            }
        }
        PDFObject::Array(filters) => filters.iter().position(|filter| {
            matches!(
                filter.as_ref(),
                PDFObject::Name(name) if matches!(name.as_str(), "CCITTFaxDecode" | "CCF")
            )
        }),
        _ => None,
    }
}

#[cfg(feature = "hayro-ccitt")]
fn num_in(p: Option<&PDFObject>, key: &str) -> Option<i64> {
    p.and_then(|p| match p {
        PDFObject::Dictionary(d) => Some(d),
        _ => None,
    })?
    .get(key)
    .and_then(|v| match v {
        PDFObject::Number(n) => Some(*n as i64),
        _ => None,
    })
}

#[cfg(feature = "hayro-ccitt")]
fn bool_in(p: Option<&PDFObject>, key: &str) -> Option<bool> {
    p.and_then(|p| match p {
        PDFObject::Dictionary(d) => Some(d),
        _ => None,
    })?
    .get(key)
    .and_then(|v| match v {
        PDFObject::Boolean(b) => Some(*b),
        _ => None,
    })
}

#[cfg(feature = "hayro-ccitt")]
struct BitmapSink {
    bytes: Vec<u8>,
}

#[cfg(feature = "hayro-ccitt")]
impl BitmapSink {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(feature = "hayro-ccitt")]
impl hayro_ccitt::Decoder for BitmapSink {
    fn push_byte(&mut self, byte: u8) {
        self.bytes.push(byte);
    }
    fn push_bytes(&mut self, byte: u8, count: usize) {
        self.bytes.extend(std::iter::repeat_n(byte, count));
    }
    fn next_line(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::parser::PDFObject;
    use crate::core::parser::Ref;
    use crate::rendering::device::TestDevice;
    use smallvec::smallvec;
    use std::sync::Arc;

    struct TestResolver {
        objects: HashMap<(u32, u32), Arc<PDFObject>>,
    }

    impl TestResolver {
        fn new(objects: HashMap<(u32, u32), PDFObject>) -> Self {
            Self {
                objects: objects
                    .into_iter()
                    .map(|(key, value)| (key, Arc::new(value)))
                    .collect(),
            }
        }
    }

    impl PageResolver for TestResolver {
        fn resolve_if_ref(&mut self, obj: &PDFObject) -> PDFResult<Arc<PDFObject>> {
            match obj {
                PDFObject::Ref(r) => self.resolve_ref(r.num, r.generation),
                _ => Ok(Arc::new(obj.clone())),
            }
        }

        fn resolve_ref(&mut self, num: u32, generation: u32) -> PDFResult<Arc<PDFObject>> {
            self.objects
                .get(&(num, generation))
                .cloned()
                .ok_or_else(|| PDFError::Generic(format!("missing test object {num} {generation}")))
        }
    }

    #[test]
    fn test_context_creation() {
        let mut device = TestDevice::new(612.0, 792.0);
        let ctx = RenderingContext::new(&mut device);
        assert_eq!(ctx.state_stack.len(), 1);
        assert!(ctx.current_path.is_empty());
    }

    #[test]
    fn test_save_restore() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.current_state_mut().stroke_color = Color::red();
        ctx.save().unwrap();

        ctx.current_state_mut().stroke_color = Color::blue();
        assert_eq!(ctx.current_state().stroke_color, Color::RGB(0.0, 0.0, 1.0));

        ctx.restore().unwrap();
        assert_eq!(ctx.current_state().stroke_color, Color::RGB(1.0, 0.0, 0.0));
    }

    #[test]
    fn separation_tint_resolves_indirect_icc_alternate_profile() {
        let mut objects = HashMap::new();
        objects.insert(
            (15, 0),
            PDFObject::Stream {
                dict: HashMap::from([("N".to_string(), PDFObject::Number(3.0))]),
                data: Vec::new(),
            },
        );
        let mut resolver_impl = TestResolver::new(objects);
        let mut resolver: Option<&mut dyn PageResolver> = Some(&mut resolver_impl);

        let color_space = PDFObject::Array(smallvec![
            Box::new(PDFObject::Name("Separation".to_string())),
            Box::new(PDFObject::Name("Spot".to_string())),
            Box::new(PDFObject::Array(smallvec![
                Box::new(PDFObject::Name("ICCBased".to_string())),
                Box::new(PDFObject::Ref(Ref {
                    num: 15,
                    generation: 0,
                })),
            ])),
            Box::new(PDFObject::Dictionary(HashMap::from([
                ("FunctionType".to_string(), PDFObject::Number(2.0)),
                (
                    "C0".to_string(),
                    PDFObject::Array(smallvec![
                        Box::new(PDFObject::Number(0.0)),
                        Box::new(PDFObject::Number(0.2)),
                        Box::new(PDFObject::Number(0.4)),
                    ]),
                ),
                (
                    "C1".to_string(),
                    PDFObject::Array(smallvec![
                        Box::new(PDFObject::Number(0.5)),
                        Box::new(PDFObject::Number(0.6)),
                        Box::new(PDFObject::Number(0.7)),
                    ]),
                ),
                ("N".to_string(), PDFObject::Number(1.0)),
            ]))),
        ]);

        let color = color_from_color_space(&color_space, &[PDFObject::Number(1.0)], &mut resolver);

        assert_eq!(color, Some(Color::RGB(0.5, 0.6, 0.7)));
    }

    #[test]
    fn color_space_initial_color_matches_pdf_defaults() {
        let mut resolver = None;

        assert_eq!(
            initial_color_for_color_space(
                Some(PDFObject::Name("DeviceGray".to_string())),
                &mut resolver
            ),
            Some(Color::Gray(0.0))
        );
        assert_eq!(
            initial_color_for_color_space(
                Some(PDFObject::Name("DeviceRGB".to_string())),
                &mut resolver
            ),
            Some(Color::RGB(0.0, 0.0, 0.0))
        );
        assert_eq!(
            initial_color_for_color_space(
                Some(PDFObject::Name("DeviceCMYK".to_string())),
                &mut resolver
            ),
            Some(Color::CMYK(0.0, 0.0, 0.0, 1.0))
        );
    }

    #[test]
    fn type3_differences_preserve_base_encoding_names() {
        let encoding = PDFObject::Dictionary(HashMap::from([
            (
                "BaseEncoding".to_string(),
                PDFObject::Name("WinAnsiEncoding".to_string()),
            ),
            (
                "Differences".to_string(),
                PDFObject::Array(smallvec![
                    Box::new(PDFObject::Number(65.0)),
                    Box::new(PDFObject::Name("CustomA".to_string())),
                ]),
            ),
        ]));

        let names = type3_difference_names(&encoding).expect("differences parsed");

        assert_eq!(names[65], Some("CustomA".to_string()));
        assert_eq!(names[66], Some("B".to_string()));
        assert_eq!(names[b' ' as usize], Some("space".to_string()));
    }

    #[test]
    fn indexed_4bpc_expansion_resets_at_row_boundaries() {
        let lut = [
            0, 0, 0, // 0
            10, 0, 0, // 1
            20, 0, 0, // 2
            30, 0, 0, // 3
            40, 0, 0, // 4
            50, 0, 0, // 5
            60, 0, 0, // 6
        ];
        let data = [0x12, 0x30, 0x45, 0x60];

        let expanded = expand_indexed(&data, &lut, 3, 4, 3, 2, 6);

        assert_eq!(
            expanded,
            vec![10, 0, 0, 20, 0, 0, 30, 0, 0, 40, 0, 0, 50, 0, 0, 60, 0, 0,]
        );
    }

    #[test]
    fn test_move_to_operator() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        let op = Operation::new(
            OpCode::MoveTo,
            vec![PDFObject::Number(10.0), PDFObject::Number(20.0)],
        );

        ctx.process_operation(&op).unwrap();
        assert_eq!(ctx.current_path.current_point(), Some((10.0, 20.0)));
    }

    #[test]
    fn test_show_text_advances_text_matrix_only() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().text_matrix = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        ctx.current_state_mut().text_line_matrix = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];

        let op = Operation::new(OpCode::ShowText, vec![PDFObject::String(vec![b'A', b'B'])]);
        ctx.process_operation(&op).unwrap();

        assert_eq!(
            ctx.current_state().text_matrix,
            [1.0, 0.0, 0.0, 1.0, 22.0, 20.0]
        );
        assert_eq!(
            ctx.current_state().text_line_matrix,
            [1.0, 0.0, 0.0, 1.0, 10.0, 20.0]
        );
    }

    #[test]
    fn test_show_text_hex_string_advances_text_matrix_only() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().text_matrix = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        ctx.current_state_mut().text_line_matrix = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];

        let op = Operation::new(
            OpCode::ShowText,
            vec![PDFObject::HexString(vec![b'A', b'B'])],
        );
        ctx.process_operation(&op).unwrap();

        assert_eq!(
            ctx.current_state().text_matrix,
            [1.0, 0.0, 0.0, 1.0, 22.0, 20.0]
        );
        assert_eq!(
            ctx.current_state().text_line_matrix,
            [1.0, 0.0, 0.0, 1.0, 10.0, 20.0]
        );
    }

    #[test]
    fn test_show_text_respects_rotated_text_matrix() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().text_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];
        ctx.current_state_mut().text_line_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];

        let op = Operation::new(OpCode::ShowText, vec![PDFObject::String(vec![b'A'])]);
        ctx.process_operation(&op).unwrap();

        assert_eq!(
            ctx.current_state().text_matrix,
            [0.0, 1.0, -1.0, 0.0, 10.0, 26.0]
        );
    }

    #[test]
    fn test_show_text_applies_char_and_word_spacing() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().font_size = Some(10.0);
        ctx.current_state_mut().character_spacing = 0.5;
        ctx.current_state_mut().word_spacing = 1.0;

        let op = Operation::new(
            OpCode::ShowText,
            vec![PDFObject::String(vec![b'A', b' ', b'B'])],
        );
        ctx.process_operation(&op).unwrap();

        // Width from TestDevice:
        // glyphs: (3 * 500 * 10) / 1000 = 15.0
        // spacing: (Tc * 3) + (Tw * 1) = 2.5
        // total: 17.5
        assert!((ctx.current_state().text_matrix[4] - 17.5).abs() < 1e-6);
        assert_eq!(ctx.current_state().text_line_matrix[4], 0.0);
    }

    #[test]
    fn test_show_text_applies_hscale_to_char_and_word_spacing() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().font_size = Some(10.0);
        ctx.current_state_mut().character_spacing = 0.5;
        ctx.current_state_mut().word_spacing = 1.0;
        ctx.current_state_mut().text_horizontal_scaling = 50.0;

        let op = Operation::new(
            OpCode::ShowText,
            vec![PDFObject::String(vec![b'A', b' ', b'B'])],
        );
        ctx.process_operation(&op).unwrap();

        // Unscaled width: 17.5 (see test above), with 50% horizontal scale => 8.75.
        assert!((ctx.current_state().text_matrix[4] - 8.75).abs() < 1e-6);
    }

    #[test]
    fn test_show_spaced_text_offset_sign_and_scaling() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().font_size = Some(10.0);
        ctx.current_state_mut().text_horizontal_scaling = 50.0;

        let op = Operation::new(
            OpCode::ShowSpacedText,
            vec![PDFObject::Array(smallvec![
                Box::new(PDFObject::String(vec![b'A'])),
                Box::new(PDFObject::Number(120.0))
            ])],
        );

        ctx.process_operation(&op).unwrap();

        // Width from TestDevice draw_text: ((1 * 500 * 10) / 1000) * 0.5 = 2.5
        // TJ adjustment: -(120 * 10 * 0.5) / 1000 = -0.6
        assert!((ctx.current_state().text_matrix[4] - 1.9).abs() < 1e-6);
        assert_eq!(ctx.current_state().text_line_matrix[4], 0.0);
    }

    #[test]
    fn test_show_spaced_text_vertical_tj_adjusts_ty_without_hscale() {
        // Identity-V: TJ numbers advance along ty and ignore Tz.
        // Source: hayro TextState::apply_adjustment
        let mut device = TestDevice::new(612.0, 792.0);
        device.set_vertical_writing(true);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().font_name = Some("F1".to_string());
        ctx.current_state_mut().font_size = Some(10.0);
        // Tz must not affect vertical TJ adjustments.
        ctx.current_state_mut().text_horizontal_scaling = 50.0;

        let op = Operation::new(
            OpCode::ShowSpacedText,
            vec![PDFObject::Array(smallvec![Box::new(PDFObject::Number(
                120.0
            ))])],
        );
        ctx.process_operation(&op).unwrap();

        // adjust = -(120 * 10 * 1.0) / 1000 = -1.2 on ty, tx unchanged.
        assert!((ctx.current_state().text_matrix[4]).abs() < 1e-6);
        assert!((ctx.current_state().text_matrix[5] - (-1.2)).abs() < 1e-6);
        assert_eq!(ctx.current_state().text_line_matrix[5], 0.0);
    }

    #[test]
    fn test_show_spaced_text_applies_char_word_spacing_and_tj_adjustment() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().font_size = Some(10.0);
        ctx.current_state_mut().character_spacing = 0.5;
        ctx.current_state_mut().word_spacing = 1.0;

        let op = Operation::new(
            OpCode::ShowSpacedText,
            vec![PDFObject::Array(smallvec![
                Box::new(PDFObject::String(vec![b'A', b' ', b'B'])),
                Box::new(PDFObject::Number(100.0))
            ])],
        );
        ctx.process_operation(&op).unwrap();

        // String width: 17.5 (with Tc/Tw), TJ adjustment: -(100 * 10 * 1.0) / 1000 = -1.0
        assert!((ctx.current_state().text_matrix[4] - 16.5).abs() < 1e-6);
    }

    #[test]
    fn test_show_spaced_text_hex_string_offset_sign_and_scaling() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().font_size = Some(10.0);
        ctx.current_state_mut().text_horizontal_scaling = 50.0;

        let op = Operation::new(
            OpCode::ShowSpacedText,
            vec![PDFObject::Array(smallvec![
                Box::new(PDFObject::HexString(vec![b'A'])),
                Box::new(PDFObject::Number(120.0))
            ])],
        );

        ctx.process_operation(&op).unwrap();

        // Width from TestDevice draw_text: ((1 * 500 * 10) / 1000) * 0.5 = 2.5
        // TJ adjustment: -(120 * 10 * 0.5) / 1000 = -0.6
        assert!((ctx.current_state().text_matrix[4] - 1.9).abs() < 1e-6);
        assert_eq!(ctx.current_state().text_line_matrix[4], 0.0);
    }

    #[test]
    fn test_move_text_post_concats_translation() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().text_line_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];
        ctx.current_state_mut().text_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];

        let op = Operation::new(
            OpCode::MoveText,
            vec![PDFObject::Number(5.0), PDFObject::Number(3.0)],
        );
        ctx.process_operation(&op).unwrap();

        assert_eq!(
            ctx.current_state().text_matrix,
            [0.0, 1.0, -1.0, 0.0, 7.0, 25.0]
        );
        assert_eq!(
            ctx.current_state().text_line_matrix,
            [0.0, 1.0, -1.0, 0.0, 7.0, 25.0]
        );
    }

    #[test]
    fn test_next_line_post_concats_leading() {
        let mut device = TestDevice::new(612.0, 792.0);
        let mut ctx = RenderingContext::new(&mut device);

        ctx.begin_text().unwrap();
        ctx.current_state_mut().text_line_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];
        ctx.current_state_mut().text_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];
        ctx.current_state_mut().text_leading = 4.0;

        ctx.next_line().unwrap();

        assert_eq!(
            ctx.current_state().text_matrix,
            [0.0, 1.0, -1.0, 0.0, 14.0, 20.0]
        );
        assert_eq!(
            ctx.current_state().text_line_matrix,
            [0.0, 1.0, -1.0, 0.0, 14.0, 20.0]
        );
    }

    #[test]
    fn test_remap_text_bytes_with_cmap_shifted_single_byte_codes() {
        let cmap = CMap::parse(
            br#"
4 beginbfchar
<2A> <0047>
<44> <0061>
<50> <006D>
<48> <0065>
endbfchar
"#,
        )
        .unwrap();

        let remapped = remap_text_bytes_with_cmap(b"*DPH", &cmap).unwrap();
        assert_eq!(remapped, b"Game");
    }

    #[test]
    fn test_remap_text_bytes_with_cmap_prefers_two_byte_codes() {
        let cmap = CMap::parse(
            br#"
2 beginbfchar
<0102> <0041>
<0304> <0042>
endbfchar
"#,
        )
        .unwrap();

        let remapped = remap_text_bytes_with_cmap(&[0x01, 0x02, 0x03, 0x04], &cmap).unwrap();
        assert_eq!(remapped, b"AB");
    }

    #[test]
    fn test_remap_text_bytes_with_cmap_unchanged_returns_none() {
        let cmap = CMap::parse(
            br#"
1 beginbfchar
<41> <0041>
endbfchar
"#,
        )
        .unwrap();

        assert!(remap_text_bytes_with_cmap(b"A", &cmap).is_none());
    }

    #[test]
    fn test_font_needs_to_unicode_remap_for_simple_type1_is_false() {
        let info = crate::core::font::FontDict {
            font_type: crate::core::font::FontType::Type1,
            base_font: "Test".to_string(),
            encoding: Some(PDFObject::Name("WinAnsiEncoding".to_string())),
            to_unicode: None,
            font_descriptor: None,
            widths: None,
            first_char: None,
            last_char: None,
            default_width: 250.0,
            descendant_fonts: None,
        };

        assert!(!font_needs_to_unicode_remap(&info));
    }

    #[test]
    fn test_font_needs_to_unicode_remap_for_descendant_font_is_true() {
        let info = crate::core::font::FontDict {
            font_type: crate::core::font::FontType::Unknown,
            base_font: "Test".to_string(),
            encoding: None,
            to_unicode: None,
            font_descriptor: None,
            widths: None,
            first_char: None,
            last_char: None,
            default_width: 250.0,
            descendant_fonts: Some(PDFObject::Array(smallvec![])),
        };

        assert!(font_needs_to_unicode_remap(&info));
    }

    #[test]
    fn test_parse_blend_mode_from_name() {
        let mode = parse_blend_mode(&PDFObject::Name("SoftLight".to_string()));
        assert_eq!(mode, Some(crate::rendering::BlendMode::SoftLight));
    }

    #[test]
    fn test_parse_blend_mode_from_array() {
        let mode = parse_blend_mode(&PDFObject::Array(smallvec![
            Box::new(PDFObject::Name("Multiply".to_string())),
            Box::new(PDFObject::Name("Normal".to_string())),
        ]));
        assert_eq!(mode, Some(crate::rendering::BlendMode::Multiply));
    }

    #[test]
    fn test_text_paint_for_invisible_mode_is_transparent() {
        let paint = text_paint_for_rendering_mode(
            crate::rendering::TextRenderingMode::Invisible,
            Color::RGB(0.0, 0.0, 0.0),
        );
        assert!(matches!(paint, Paint::Transparent));
    }

    #[test]
    fn test_text_paint_for_fill_mode_uses_fill_color() {
        let paint = text_paint_for_rendering_mode(
            crate::rendering::TextRenderingMode::Fill,
            Color::RGB(1.0, 0.0, 0.0),
        );
        assert!(matches!(paint, Paint::Solid(Color::RGB(1.0, 0.0, 0.0))));
    }

    #[test]
    fn test_pick_fallback_standard_font_respects_style() {
        // Unembedded Garamond faces must not all collapse to Times-Roman.
        assert_eq!(
            pick_fallback_standard_font_key("Garamond-Bold"),
            "Times-Bold"
        );
        assert_eq!(
            pick_fallback_standard_font_key("Garamond-LightItalic"),
            "Times-Italic"
        );
        assert_eq!(
            pick_fallback_standard_font_key("Garamond-Book"),
            "Times-Roman"
        );
        assert_eq!(
            pick_fallback_standard_font_key("MySans-BoldOblique"),
            "Helvetica-BoldOblique"
        );
        assert_eq!(
            pick_fallback_standard_font_key("CourierNewPS-BoldMT"),
            "Courier-Bold"
        );
    }
}
