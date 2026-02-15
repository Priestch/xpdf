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
use crate::core::content_stream::{OpCode, Operation};
use crate::core::error::{PDFError, PDFResult};
use crate::core::parser::PDFObject;
use crate::core::xref::XRef;

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

    /// XRef table for fetching objects (for XObject rendering)
    xref: Option<&'a mut XRef>,

    /// Page resources dictionary (for looking up XObjects, fonts, etc.)
    resources: Option<&'a PDFObject>,

    /// Operation counter for debug logging
    operation_count: usize,
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
            xref: None,
            resources: None,
            operation_count: 0,
        }
    }

    /// Set the xref table and page resources for XObject rendering.
    ///
    /// # Arguments
    /// * `xref` - The cross-reference table for fetching objects
    /// * `resources` - The page's resources dictionary
    pub fn set_xobject_resources(&mut self, xref: &'a mut XRef, resources: &'a PDFObject) {
        self.xref = Some(xref);
        self.resources = Some(resources);
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

            // XObject operator
            OpCode::PaintXObject => self.paint_xobject(&op.args)?,

            _ => {
                // Other operators not yet implemented
                // Silently skip to avoid spamming logs
                // These are typically non-critical operators like BeginMarkedContent, SetGState, etc.
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
        let paint = Paint::from_color(state.fill_color.clone());
        let text_matrix = state.text_matrix;
        let horizontal_scaling = state.text_horizontal_scaling;
        let text_rise = state.text_rise;

        // Extract text bytes (using font's encoding, NOT UTF-8)
        if let crate::core::parser::PDFObject::String(bytes) = &args[0] {
            // Draw text and get the actual rendered width
            let rendered_width = self.device.draw_text(
                bytes,
                &font_name,
                font_size,
                &paint,
                &text_matrix,
                horizontal_scaling,
                text_rise,
            )?;

            // CRITICAL: Update text matrix to advance after rendering
            // According to PDF spec, Tj operator advances text matrix by text width
            // The rendered_width is in text space units, so we add it directly
            self.current_state_mut().text_matrix[4] += rendered_width;
            // Also update the line matrix to match
            self.current_state_mut().text_line_matrix[4] += rendered_width;
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
        let paint = Paint::from_color(state.fill_color.clone());
        let horizontal_scaling = state.text_horizontal_scaling;
        let text_rise = state.text_rise;

        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: show_spaced_text: fill_color={:?}, paint={:?}", state.fill_color, paint);
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
                PDFObject::String(bytes) => {
                    // Render this text string (using font encoding, NOT UTF-8)
                    if !bytes.is_empty() {
                        let text_matrix = self.current_state().text_matrix;
                        let rendered_width = self.device.draw_text(
                            bytes,
                            &font_name,
                            font_size,
                            &paint,
                            &text_matrix,
                            horizontal_scaling,
                            text_rise,
                        )?;

                        // Advance text matrix by the actual rendered width
                        // This is critical for correct text positioning in TJ operator
                        self.current_state_mut().text_matrix[4] += rendered_width;
                        self.current_state_mut().text_line_matrix[4] += rendered_width;
                    }
                }
                PDFObject::Number(offset) => {
                    // Offset in thousandths of an em (negative = backspace, positive = space)
                    // Convert to user space: (offset * font_size) / 1000
                    let adjust = (offset * font_size) / 1000.0;
                    self.current_state_mut().text_matrix[4] += adjust;
                    self.current_state_mut().text_line_matrix[4] += adjust;
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
        state.text_line_matrix[4] += tx;
        state.text_line_matrix[5] += ty;
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
        state.text_line_matrix[5] -= state.text_leading;
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
            // Extract xref and resources to avoid borrow issues
            let (xref_opt, resources_opt) = (self.xref.take(), self.resources.clone());
            if let (Some(mut xref), Some(resources)) = (xref_opt, resources_opt) {
                // Check if device already has this font (to avoid re-loading)
                let font_key = name.as_str();
                // We can't directly check the device's font cache, so we try to load
                // and let the device handle duplicates
                let result = self.load_font_from_resources(font_key, &resources, &mut xref);
                // Restore xref
                self.xref = Some(xref);
                result?;
            }
        }

        self.current_state_mut().font_size = Some(extract_number(args, 1)?);
        Ok(())
    }

    /// Load a font from the page's resources dictionary.
    ///
    /// This looks up the font in the Resources/Font dictionary, extracts the font data,
    /// and loads it into the rendering device.
    fn load_font_from_resources(
        &mut self,
        font_name: &str,
        resources: &PDFObject,
        xref: &mut XRef,
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
                _fetched_font_dict = xref.fetch(ref_obj.num, ref_obj.generation)?;
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
        let _fetched_font_obj: PDFObject;
        let font_obj = xref.fetch_if_ref(font_obj_ref)?;

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

        // Try to get font data
        let font_data = self.extract_font_data(&font_dict_info, xref);

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
                match xref.fetch_if_ref(enc) {
                    Ok(fetched) => Some(fetched),
                    Err(_) => Some(enc.clone()),
                }
            } else {
                None
            };
            let encoding_ref = encoding_obj.as_ref();
            match self.device.load_font_data(font_name, data, encoding_ref) {
                Ok(_) => {
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
        Ok(())
    }

    /// Extract font data from a FontDict.
    ///
    /// Returns the raw font data (TrueType, CFF, etc.) if available.
    fn extract_font_data(
        &self,
        font_dict: &crate::core::font::FontDict,
        xref: &mut XRef,
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
                    _fetched_descriptor = xref.fetch(r.num, r.generation).ok()?;
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
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!("DEBUG: Checking for key '{}'", key);
                if let Some(font_file_ref) = descriptor.get(key) {
                    #[cfg(feature = "debug-logging")]
                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: Found font file ref for '{}'", key);
                    // Fetch the font file stream
                    let font_file_obj = xref.fetch_if_ref(font_file_ref).ok()?;
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

        self.current_state_mut().character_spacing = extract_number(args, 0)?;
        Ok(())
    }

    fn set_word_spacing(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tw operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().word_spacing = extract_number(args, 0)?;
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

        // Get xref and resources
        let xref = match &mut self.xref {
            Some(x) => x,
            None => return Ok(()),
        };

        let resources = match &self.resources {
            Some(r) => r,
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
                _fetched_xobject_dict = xref.fetch(ref_obj.num, ref_obj.generation)?;
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

        let xobject = xref.fetch_if_ref(xobject_ref)?;

        // Check if it's an image XObject
        let xobject_dict = match &xobject {
            PDFObject::Stream { dict, .. } => dict,
            _ => return Ok(()),
        };

        let subtype = match xobject_dict.get("Subtype") {
            Some(PDFObject::Name(name)) => name,
            _ => return Ok(()),
        };

        if subtype != "Image" {
            return Ok(()); // Only support images for now
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
        let image_data = match &xobject {
            PDFObject::Stream { data, .. } => data,
            _ => return Ok(()),
        };

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::parser::PDFObject;
    use crate::rendering::device::TestDevice;

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
}
