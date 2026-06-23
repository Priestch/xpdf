//! Rendering context for processing PDF content streams.
//!
//! This module provides the RenderingContext, which coordinates:
//! - Graphics state stack (save/restore)
//! - Current path being constructed
//! - Device for rendering operations
//! - Processing of content stream operators

use super::device::Device;
use super::graphics_state::{Color, FillRule, GraphicsState};
use super::path::Path;
use super::{Paint, PathDrawMode};
use crate::core::cmap::CMap;
use crate::core::content_stream::{OpCode, Operation};
use crate::core::error::{PDFError, PDFResult};
use crate::core::page::PageResolver;
use crate::core::parser::PDFObject;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

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

    /// Current path being constructed
    current_path: Path,

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
            current_path: Path::new(),
            clip_stack: Vec::new(),
            in_text_object: false,
            resolver: None,
            resources: None,
            operation_count: 0,
            font_to_unicode: HashMap::new(),
            font_requires_cmap_remap: HashMap::new(),
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

            // Line property operators
            OpCode::SetLineWidth => self.set_line_width(&op.args)?,
            OpCode::SetLineCap => self.set_line_cap(&op.args)?,
            OpCode::SetLineJoin => self.set_line_join(&op.args)?,
            OpCode::SetMiterLimit => self.set_miter_limit(&op.args)?,
            OpCode::SetDash => self.set_dash(&op.args)?,
            OpCode::SetGState => self.set_graphics_state(&op.args)?,

            // XObject operator
            OpCode::PaintXObject => self.paint_xobject(&op.args)?,

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
        let state = self.current_state();
        let paint = Paint::from_color(state.stroke_color.clone());
        let stroke_props = state.stroke_props.clone();
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Stroke path with color {:?}", state.stroke_color);
        self.device
            .draw_path(PathDrawMode::Stroke, &paint, &stroke_props)?;
        self.current_path.begin();
        Ok(())
    }

    fn close_and_stroke(&mut self) -> PDFResult<()> {
        self.close_path()?;
        self.stroke()
    }

    fn fill(&mut self, rule: FillRule) -> PDFResult<()> {
        let state = self.current_state();
        let paint = Paint::from_color(state.fill_color.clone());
        let stroke_props = state.stroke_props.clone();
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Fill path with color {:?}", state.fill_color);
        self.device
            .draw_path(PathDrawMode::Fill(rule), &paint, &stroke_props)?;
        self.current_path.begin();
        Ok(())
    }

    fn fill_and_stroke(&mut self, rule: FillRule) -> PDFResult<()> {
        let state = self.current_state();
        // For fill and stroke, we use fill color for fill, stroke color for stroke
        // But our Device trait only takes one paint, so we use fill color
        let paint = Paint::from_color(state.fill_color.clone());
        let stroke_props = state.stroke_props.clone();
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
        self.current_path.begin();
        Ok(())
    }

    // === Clipping Operators ===

    fn clip(&mut self, rule: FillRule) -> PDFResult<()> {
        self.device.clip_path(rule)?;
        self.clip_stack.push(rule);
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
        let paint = text_paint_for_rendering_mode(state.text_rendering_mode, state.fill_color);
        let text_matrix = state.text_matrix;
        let horizontal_scaling = state.text_horizontal_scaling;
        let text_rise = state.text_rise;
        let character_spacing = state.character_spacing;
        let word_spacing = state.word_spacing;

        // Extract text bytes (using font's encoding, NOT UTF-8).
        // PDF text strings can be literal strings (...) or hex strings <...>.
        if let Some(bytes) = text_bytes(&args[0]) {
            let mapped_bytes = self.map_text_bytes_for_device(&font_name, bytes);
            // Draw text and get the actual rendered width
            let rendered_width = self.device.draw_text(
                mapped_bytes.as_ref(),
                &font_name,
                font_size,
                &paint,
                &text_matrix,
                horizontal_scaling,
                text_rise,
                character_spacing,
                word_spacing,
            )?;

            if is_text_trace_enabled() {
                eprintln!(
                    "[text-trace] Tj bytes={:?} width={} Tc={} Tw={} hscale={}",
                    bytes, rendered_width, character_spacing, word_spacing, horizontal_scaling
                );
            }

            // Advance only the current text matrix (Tm = Tm * T(tx, 0)).
            // The text line matrix (Tlm) must stay unchanged for T*/' operators.
            let state = self.current_state_mut();
            state.text_matrix = post_concat_translate(state.text_matrix, rendered_width, 0.0);
        }

        Ok(())
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
        let paint = text_paint_for_rendering_mode(state.text_rendering_mode, state.fill_color);
        let horizontal_scaling = state.text_horizontal_scaling;
        let text_rise = state.text_rise;
        let character_spacing = state.character_spacing;
        let word_spacing = state.word_spacing;

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: show_spaced_text: fill_color={:?}, paint={:?}",
            state.fill_color, paint
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
                        let mapped_bytes = self.map_text_bytes_for_device(&font_name, bytes);
                        let text_matrix = self.current_state().text_matrix;
                        let rendered_width = self.device.draw_text(
                            mapped_bytes.as_ref(),
                            &font_name,
                            font_size,
                            &paint,
                            &text_matrix,
                            horizontal_scaling,
                            text_rise,
                            character_spacing,
                            word_spacing,
                        )?;

                        if is_text_trace_enabled() {
                            eprintln!(
                                "[text-trace] TJ text bytes={:?} width={} Tc={} Tw={} hscale={}",
                                bytes,
                                rendered_width,
                                character_spacing,
                                word_spacing,
                                horizontal_scaling
                            );
                        }

                        // Advance text matrix by the actual rendered width
                        let state = self.current_state_mut();
                        state.text_matrix =
                            post_concat_translate(state.text_matrix, rendered_width, 0.0);
                    }
                }
                PDFObject::Number(offset) => {
                    // TJ numbers are spacing adjustments in thousandths of an em.
                    // Positive numbers tighten spacing, so the translation is negative.
                    let adjust = -(offset * font_size * (horizontal_scaling / 100.0)) / 1000.0;
                    if is_text_trace_enabled() {
                        eprintln!(
                            "[text-trace] TJ adjust raw={} translated={}",
                            offset, adjust
                        );
                    }
                    let state = self.current_state_mut();
                    state.text_matrix = post_concat_translate(state.text_matrix, adjust, 0.0);
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

            // Try to load the font from resources if not already loaded
            // We only need to load it once per font name
            // Extract resolver and resources to avoid borrow issues
            let (resolver_opt, resources_opt) = (self.resolver.take(), self.resources.clone());
            if let (Some(mut resolver), Some(resources)) = (resolver_opt, resources_opt) {
                // Check if device already has this font (to avoid re-loading)
                let font_key = name.as_str();
                // We can't directly check the device's font cache, so we try to load
                // and let the device handle duplicates
                let result = self.load_font_from_resources(font_key, &resources, &mut *resolver);
                // Restore resolver
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

        let (dict, data) = match fetched {
            PDFObject::Stream { dict, data } => (dict, data),
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
                match &_fetched_font_dict {
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
        let _fetched_font_obj: PDFObject;
        let font_obj = resolver.resolve_if_ref(font_obj_ref)?;

        // Parse the font dictionary
        let font_dict_info = match crate::core::font::FontDict::from_pdf_object(&font_obj) {
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
            // Try to load the font data with encoding, but fall back to system fonts if it fails
            // (e.g., CFF fonts can't be parsed by ttf_parser)
            // Dereference encoding if it's a reference
            let encoding_obj = if let Some(enc) = &font_dict_info.encoding {
                match resolver.resolve_if_ref(enc) {
                    Ok(fetched) => Some(fetched),
                    Err(_) => Some(enc.clone()),
                }
            } else {
                None
            };
            let encoding_ref = encoding_obj.as_ref();
            match self.device.load_font_data(font_name, data, encoding_ref) {
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

                    #[cfg(feature = "debug-logging")]
                    #[cfg(feature = "debug-logging")]
                    eprintln!(
                        "DEBUG: Successfully loaded font '{}' into device",
                        font_name
                    );
                    return Ok(());
                }
                Err(e) => {
                    #[cfg(feature = "debug-logging")]
                    #[cfg(feature = "debug-logging")]
                    eprintln!(
                        "DEBUG: Failed to load font data for '{}': {:?}, falling back to system fonts",
                        font_name, e
                    );
                    // Fall through to try system fonts
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

        // Try to load as a standard font
        // Pass font_name as the cache key (e.g., "F0") and base_font for system font mapping
        self.load_standard_font(font_name, &font_dict_info.base_font)?;

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

    /// Extract font data from a FontDict.
    ///
    /// Returns the raw font data (TrueType, CFF, etc.) if available.
    fn extract_font_data(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<Vec<u8>> {
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: extract_font_data - font_descriptor exists: {}",
            font_dict.font_descriptor.is_some()
        );

        // Check for embedded font data in FontDescriptor
        if let Some(descriptor_ref) = &font_dict.font_descriptor {
            // Fetch the descriptor (might be a reference)
            let _fetched_descriptor;
            let descriptor = match descriptor_ref {
                PDFObject::Dictionary(d) => d,
                PDFObject::Ref(r) => {
                    _fetched_descriptor = resolver.resolve_ref(r.num, r.generation).ok()?;
                    match &_fetched_descriptor {
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
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!("DEBUG: Checking for key '{}'", key);
                if let Some(font_file_ref) = descriptor.get(key) {
                    #[cfg(feature = "debug-logging")]
                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: Found font file ref for '{}'", key);
                    // Fetch the font file stream
                    let font_file_obj = resolver.resolve_if_ref(font_file_ref).ok()?;
                    if let PDFObject::Stream { dict, data } = font_file_obj {
                        #[cfg(feature = "debug-logging")]
                        eprintln!("DEBUG: Font file stream found, data length: {}", data.len());

                        // Decode if needed
                        if let Some(filter) = dict.get("Filter") {
                            match filter {
                                PDFObject::Name(filter_name) => {
                                    if filter_name == "FlateDecode" || filter_name == "Fl" {
                                        if let Ok(decoded) =
                                            crate::core::decode::decode_flate(&data)
                                        {
                                            #[cfg(feature = "debug-logging")]
                                            eprintln!(
                                                "DEBUG: Successfully decoded {} bytes",
                                                decoded.len()
                                            );
                                            return Some(decoded);
                                        }
                                    }
                                }
                                PDFObject::Array(filters) => {
                                    // Try first filter
                                    if !filters.is_empty() {
                                        if let Some(PDFObject::Name(filter_name)) =
                                            filters.get(0).map(|f| f.as_ref())
                                        {
                                            if filter_name == "FlateDecode" || filter_name == "Fl" {
                                                if let Ok(decoded) =
                                                    crate::core::decode::decode_flate(&data)
                                                {
                                                    #[cfg(feature = "debug-logging")]
                                                    eprintln!(
                                                        "DEBUG: Successfully decoded {} bytes",
                                                        decoded.len()
                                                    );
                                                    return Some(decoded);
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }

                        // No filter or decode failed, return raw data
                        #[cfg(feature = "debug-logging")]
                        eprintln!("DEBUG: Returning raw font data: {} bytes", data.len());
                        return Some(data.clone());
                    } else {
                        #[cfg(feature = "debug-logging")]
                        eprintln!("DEBUG: Font file ref is not a stream");
                    }
                } else {
                    #[cfg(feature = "debug-logging")]
                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: Key '{}' not found in descriptor", key);
                }
            }
        }

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: No font data found in extract_font_data");
        None
    }

    /// Load a standard PDF font by name (e.g., "Helvetica", "Times-Roman").
    ///
    /// This attempts to use system fonts as a fallback for standard PDF fonts.
    /// It also handles subset prefixes (e.g., "AKXEFC+DogmaBold" -> "DogmaBold").
    ///
    /// # Arguments
    /// * `cache_key` - The PDF font resource name to use as cache key (e.g., "F0")
    /// * `base_font` - The BaseFont name from the PDF for system font mapping
    fn load_standard_font(&mut self, cache_key: &str, base_font: &str) -> PDFResult<()> {
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Attempting to load system font for cache_key='{}', base_font='{}'",
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

        // Map PDF font names to system font names
        let system_font = match clean_font {
            // Standard PDF fonts
            "Times-Roman" | "TimesNewRoman" | "Times" => "Times New Roman",
            "Times-Bold" => "Times New Roman Bold",
            "Times-Italic" => "Times New Roman Italic",
            "Times-BoldItalic" => "Times New Roman Bold Italic",
            "Helvetica" => "Arial",
            "Helvetica-Bold" => "Arial Bold",
            "Helvetica-Oblique" | "Helvetica-Italic" => "Arial Italic",
            "Helvetica-BoldOblique" | "Helvetica-BoldItalic" => "Arial Bold Italic",
            "Courier" | "CourierNew" => "Courier New",
            "Courier-Bold" => "Courier New Bold",
            "Courier-Oblique" | "Courier-Italic" => "Courier New Italic",
            "Courier-BoldOblique" | "Courier-BoldItalic" => "Courier New Bold Italic",

            // Nimbus fonts (PostScript clones of standard fonts)
            s if s.starts_with("NimbusRomNo9L") => "Times New Roman",
            s if s.starts_with("NimbusSans") => "Arial",
            s if s.starts_with("NimbusMono") => "Courier New",

            // Liberation fonts (open source clones)
            s if s.starts_with("LiberationSerif") => "Times New Roman",
            s if s.starts_with("LiberationSans") => "Arial",
            s if s.starts_with("LiberationMono") => "Courier New",

            // TeX Gyre fonts (modern replacements)
            s if s.starts_with("TeXGyreTermes") => "Times New Roman",
            s if s.starts_with("TeXGyreHeros") => "Arial",
            s if s.starts_with("TeXGyreCursor") => "Courier New",

            // URW fonts (more PostScript clones)
            s if s.starts_with("URWPalladio") => "Times New Roman",
            s if s.starts_with("URWGothic") => "Arial",
            s if s.starts_with("URWCourier") => "Courier New",

            // Computer Modern fonts (TeX/LaTeX)
            s if s.contains("ComputerModern")
                || s.contains("Computer Modern")
                || s.contains("CM")
                || s.starts_with("ComputerModern")
                || s.starts_with("CMR")
                || s.starts_with("CMM")
                || s.starts_with("CMS")
                || s.starts_with("CMT")
                || s.starts_with("CMEX")
                || s.starts_with("CMB") =>
            {
                "Times New Roman"
            }

            // Latin Modern fonts (modern Computer Modern)
            s if s.starts_with("LatinModern")
                || s.starts_with("LMRoman")
                || s.starts_with("LMSans")
                || s.starts_with("LMTypewriter") =>
            {
                match clean_font {
                    s if s.contains("Sans") => "Arial",
                    s if s.contains("Mono") || s.contains("Typewriter") => "Courier New",
                    _ => "Times New Roman",
                }
            }

            // Any font with "Roman" or "Serif" in name -> Times
            s if s.contains("Roman") || s.contains("Serif") || s.contains("Book") => {
                "Times New Roman"
            }

            // Any font with "Sans" in name -> Arial
            s if s.contains("Sans") => "Arial",

            // Any font with "Mono" or "Code" in name -> Courier
            s if s.contains("Mono") || s.contains("Code") || s.contains("Typewriter") => {
                "Courier New"
            }

            _ => {
                // Last resort: try to guess font family from name
                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Unknown font '{}', attempting smart fallback",
                    clean_font
                );

                // Default to Times for serif-like fonts, Arial for sans-serif
                let fallback = if clean_font.contains("Bold") || clean_font.contains("Light") {
                    "Times New Roman" // Serif default
                } else {
                    "Times New Roman" // Safe default
                };

                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Using fallback '{}' for font '{}' (cache_key='{}')",
                    fallback, clean_font, cache_key
                );

                // Continue with the fallback instead of returning early
                fallback
            }
        };

        // Try to load the system font
        #[cfg(target_os = "linux")]
        {
            // Map system font names to actual Linux font files
            // Try multiple common locations and font families
            let font_files: Vec<&str> = match system_font.as_ref() {
                "Times New Roman" => vec![
                    // Liberation fonts (RHEL/CentOS)
                    "LiberationSerif-Regular.ttf",
                    "LiberationSerif.ttf",
                    // DejaVu fonts (most distros)
                    "DejaVuSerif.ttf",
                    // Free fonts
                    "FreeSerif.ttf",
                    // Noto fonts (modern distros)
                    "NotoSerif-Regular.ttf",
                    // URW fonts (TeX Live)
                    "NimbusRomNo9L-Regular.ttf",
                    "NimbusRoman-Regular.ttf",
                ],
                "Times New Roman Bold" => vec![
                    "LiberationSerif-Bold.ttf",
                    "DejaVuSerif-Bold.ttf",
                    "FreeSerifBold.ttf",
                    "NotoSerif-Bold.ttf",
                ],
                "Times New Roman Italic" => vec![
                    "LiberationSerif-Italic.ttf",
                    "DejaVuSerif-Italic.ttf",
                    "FreeSerifItalic.ttf",
                    "NotoSerif-Italic.ttf",
                ],
                "Times New Roman Bold Italic" => vec![
                    "LiberationSerif-BoldItalic.ttf",
                    "DejaVuSerif-BoldItalic.ttf",
                    "FreeSerifBoldItalic.ttf",
                    "NotoSerif-BoldItalic.ttf",
                ],
                "Arial" => vec![
                    "LiberationSans-Regular.ttf",
                    "LiberationSans.ttf",
                    "DejaVuSans.ttf",
                    "FreeSans.ttf",
                    "NotoSans-Regular.ttf",
                    "NimbusSans-Regular.ttf",
                ],
                "Arial Bold" => vec![
                    "LiberationSans-Bold.ttf",
                    "DejaVuSans-Bold.ttf",
                    "FreeSansBold.ttf",
                    "NotoSans-Bold.ttf",
                ],
                "Arial Italic" => vec![
                    "LiberationSans-Italic.ttf",
                    "DejaVuSans-Oblique.ttf",
                    "FreeSansOblique.ttf",
                    "NotoSans-Italic.ttf",
                ],
                "Arial Bold Italic" => vec![
                    "LiberationSans-BoldItalic.ttf",
                    "DejaVuSans-BoldOblique.ttf",
                    "FreeSansBoldOblique.ttf",
                    "NotoSans-BoldItalic.ttf",
                ],
                "Courier New" => vec![
                    "LiberationMono-Regular.ttf",
                    "LiberationMono.ttf",
                    "DejaVuSansMono.ttf",
                    "FreeMono.ttf",
                    "NotoMono-Regular.ttf",
                    "NimbusMono-Regular.ttf",
                    "Courier10PitchBT-Roman.ttf",
                ],
                "Courier New Bold" => vec![
                    "LiberationMono-Bold.ttf",
                    "DejaVuSansMono-Bold.ttf",
                    "FreeMonoBold.ttf",
                    "NotoMono-Bold.ttf",
                ],
                "Courier New Italic" => vec![
                    "LiberationMono-Italic.ttf",
                    "DejaVuSansMono-Oblique.ttf",
                    "FreeMonoOblique.ttf",
                ],
                "Courier New Bold Italic" => vec![
                    "LiberationMono-BoldItalic.ttf",
                    "DejaVuSansMono-BoldOblique.ttf",
                    "FreeMonoBoldOblique.ttf",
                ],
                _ => vec![], // No mapping
            };

            // Try each font file in multiple directories
            let font_dirs = vec![
                "/usr/share/fonts/truetype",
                "/usr/share/fonts/truetype/dejavu",
                "/usr/share/fonts/truetype/liberation",
                "/usr/share/fonts/truetype/freefont",
                "/usr/share/fonts/truetype/noto",
                "/usr/share/fonts/opentype/noto",
                "/usr/share/fonts/truetype/lmodern",
                "/usr/share/fonts/truetype/cmu", // Computer Modern Unicode
                "/usr/share/fonts/truetype/cm-unicode", // Alternative CM path
                "/usr/share/fonts/opentype/cm-unicode",
                "/usr/share/fonts/truetype/computer-modern",
                "/usr/share/fonts/opentype/public-lm",
                "/usr/share/fonts/TTF",
                "/usr/share/fonts",
                "/usr/local/share/fonts",
            ];

            for font_file in font_files {
                for dir in &font_dirs {
                    let path = format!("{}/{}", dir, font_file);
                    if let Ok(data) = std::fs::read(&path) {
                        #[cfg(feature = "debug-logging")]
                        eprintln!("DEBUG: Loaded font '{}' from {}", cache_key, path);
                        // System fonts don't have custom encodings
                        return self.device.load_font_data(cache_key, data, None);
                    }
                }
            }
        }

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Could not find system font for cache_key='{}', base_font='{}'",
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
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: set_fill_rgb: r={}, g={}, b={}, args={:?}",
            r, g, b, args
        );
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
        self.current_state_mut().fill_color = Color::CMYK(c, m, y, k);
        Ok(())
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

        let ext_gstate_dict = match ext_gstate_dict_obj {
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

        if let PDFObject::Dictionary(gs_dict) = gs_obj {
            if let Some(mode) = gs_dict.get("BM").and_then(parse_blend_mode) {
                self.current_state_mut().blend_mode = mode;
                self.device.set_blend_mode(mode);
            }
        }

        self.resolver = Some(resolver);
        Ok(())
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
                match &_fetched_xobject_dict {
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
        let (xobject_dict, xobject_data) = match &xobject {
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

        // Extract image properties
        let width = match xobject_dict.get("Width") {
            Some(PDFObject::Number(w)) => *w as u32,
            _ => return Ok(()),
        };

        let height = match xobject_dict.get("Height") {
            Some(PDFObject::Number(h)) => *h as u32,
            _ => return Ok(()),
        };

        let bits_per_component = match xobject_dict.get("BitsPerComponent") {
            Some(PDFObject::Number(b)) => *b as u8,
            _ => 8, // Default to 8
        };

        let color_space_name = match xobject_dict.get("ColorSpace") {
            Some(PDFObject::Name(name)) => Some(name.as_str()),
            Some(PDFObject::Array(_)) => {
                // TODO: Handle complex color spaces
                Some("DeviceRGB") // Default fallback
            }
            _ => Some("DeviceRGB"), // Default
        };

        // Determine if image has alpha
        let has_alpha = match color_space_name {
            Some("DeviceRGB") | Some("DeviceGray") | Some("CalRGB") | Some("CalGray") => false,
            _ => true, // Assume other color spaces might have alpha
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
                            "FlateDecode" | "Fl" => (
                                decode::decode_flate(image_data)
                                    .unwrap_or_else(|_| image_data.clone()),
                                width,
                                height,
                                bits_per_component,
                                has_alpha,
                            ),
                            "DCTDecode" | "DCT" => {
                                // JPEG data - decode it using zune-jpeg
                                #[cfg(feature = "jpeg-decoding")]
                                {
                                    match crate::core::image::ImageDecoder::decode_image(
                                        image_data,
                                        crate::core::image::ImageFormat::JPEG,
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
                                // TODO: Implement CCITT decoding
                                (
                                    image_data.clone(),
                                    width,
                                    height,
                                    bits_per_component,
                                    has_alpha,
                                )
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
                    PDFObject::Array(filters) => {
                        // TODO: Handle multiple filters (apply in order)
                        use crate::core::decode;
                        // For now, try to apply the first filter if it's FlateDecode
                        if !filters.is_empty() {
                            match filters.get(0) {
                                Some(filter_obj) => match filter_obj.as_ref() {
                                    PDFObject::Name(filter_name) => match filter_name.as_str() {
                                        "FlateDecode" | "Fl" => (
                                            decode::decode_flate(image_data)
                                                .unwrap_or_else(|_| image_data.clone()),
                                            width,
                                            height,
                                            bits_per_component,
                                            has_alpha,
                                        ),
                                        _ => (
                                            image_data.clone(),
                                            width,
                                            height,
                                            bits_per_component,
                                            has_alpha,
                                        ),
                                    },
                                    _ => (
                                        image_data.clone(),
                                        width,
                                        height,
                                        bits_per_component,
                                        has_alpha,
                                    ),
                                },
                                None => (
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

        // Create image data for the device
        let image = super::ImageData {
            width: decoded_width,
            height: decoded_height,
            data: decoded_data,
            has_alpha: decoded_has_alpha,
            bits_per_component: decoded_bpc,
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

        self.device.draw_image(image, &transform)?;

        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Image drawn successfully");

        Ok(())
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
        let form_resources = if let Some(resources_obj) = form_dict.get("Resources") {
            match resources_obj {
                PDFObject::Ref(_) => {
                    let fetched = if let Some(resolver) = self.resolver.as_mut() {
                        resolver.resolve_if_ref(resources_obj).ok()
                    } else {
                        None
                    };
                    fetched
                }
                _ => Some(resources_obj.clone()),
            }
        } else {
            None
        };

        let matrix = parse_six_number_array(form_dict.get("Matrix"))
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let bbox = parse_four_number_array(form_dict.get("BBox"));

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: PaintXObject: Entering form '{}' ({} bytes), matrix={:?}, bbox={:?}",
            xobject_name,
            decoded_data.len(),
            matrix,
            bbox
        );

        self.save()?;
        let previous_resources = self.resources.clone();

        // Keep restore/resource rollback deterministic even on early errors.
        let result = (|| -> PDFResult<()> {
            // Apply form matrix to map the form space into the current CTM.
            self.current_state_mut().concat_matrix(&matrix);
            self.device.concat_matrix(&matrix);

            // Clip to the form's BBox when provided.
            if let Some([x0, y0, x1, y1]) = bbox {
                self.device.begin_path();
                self.device.move_to(x0, y0);
                self.device.line_to(x1, y0);
                self.device.line_to(x1, y1);
                self.device.line_to(x0, y1);
                self.device.close_path();
                self.device.clip_path(FillRule::NonZero)?;
            }

            if let Some(resources_obj) = form_resources {
                self.resources = Some(resources_obj);
            }

            self.process_content_stream_bytes(decoded_data)
        })();

        self.resources = previous_resources;
        let restore_result = self.restore();

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

        loop {
            match evaluator.read_operation() {
                Ok(Some(op)) => {
                    if let Err(e) = self.process_operation(&op) {
                        #[cfg(feature = "debug-logging")]
                        eprintln!(
                            "DEBUG: Failed to process form XObject operator {:?}: {}",
                            op.op, e
                        );
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: Failed to read form XObject operation: {}", e);
                    break;
                }
            }
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

fn text_paint_for_rendering_mode(
    mode: super::graphics_state::TextRenderingMode,
    fill_color: Color,
) -> Paint {
    use super::graphics_state::TextRenderingMode;

    match mode {
        // Invisible/clip-only text should not be painted but still advance the text matrix.
        TextRenderingMode::Invisible | TextRenderingMode::Clip => Paint::Transparent,
        _ => Paint::from_color(fill_color),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::parser::PDFObject;
    use crate::rendering::device::TestDevice;
    use smallvec::smallvec;

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
}
