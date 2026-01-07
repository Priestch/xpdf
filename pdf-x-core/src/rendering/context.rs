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

/// Rendering context for processing PDF content streams.
///
/// The context maintains the graphics state stack, current path, and device
/// for rendering. It processes content stream operations and forwards them
/// to the device.
///
/// This follows the same pattern as PDF.js's CanvasGraphics and hayro's Context.
pub struct RenderingContext<D: Device> {
    /// The device for rendering
    device: D,

    /// Graphics state stack
    state_stack: Vec<GraphicsState>,

    /// Current path being constructed
    current_path: Path,

    /// Clip path stack (for nesting)
    clip_stack: Vec<FillRule>,

    /// Whether we're in a text object (BT...ET)
    in_text_object: bool,
}

impl<D: Device> RenderingContext<D> {
    /// Create a new rendering context.
    ///
    /// # Arguments
    /// * `device` - The rendering device to use
    pub fn new(device: D) -> Self {
        RenderingContext {
            device,
            state_stack: vec![GraphicsState::default()],
            current_path: Path::new(),
            clip_stack: Vec::new(),
            in_text_object: false,
        }
    }

    /// Get the current graphics state.
    pub fn current_state(&self) -> &GraphicsState {
        self.state_stack.last().expect("Graphics state stack underflow")
    }

    /// Get mutable reference to the current graphics state.
    pub fn current_state_mut(&mut self) -> &mut GraphicsState {
        self.state_stack.last_mut().expect("Graphics state stack underflow")
    }

    /// Get the device.
    pub fn device(&mut self) -> &mut D {
        &mut self.device
    }

    /// Process a content stream operation.
    ///
    /// This is the main entry point for interpreting PDF content streams.
    /// It dispatches to appropriate handler methods based on the operator.
    pub fn process_operation(&mut self, op: &Operation) -> PDFResult<()> {
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
                // Log warning but don't fail
                eprintln!("Warning: Operator {:?} not yet implemented", op.op);
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

        self.current_path.curve_to(current.0, current.1, cp2x, cp2y, x, y);
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
        self.device.draw_path(PathDrawMode::Stroke, &paint, &stroke_props)?;
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
        self.device.draw_path(PathDrawMode::Fill(rule), &paint, &stroke_props)?;
        self.current_path.begin();
        Ok(())
    }

    fn fill_and_stroke(&mut self, rule: FillRule) -> PDFResult<()> {
        let state = self.current_state();
        // For fill and stroke, we use fill color for fill, stroke color for stroke
        // But our Device trait only takes one paint, so we use fill color
        let paint = Paint::from_color(state.fill_color.clone());
        let stroke_props = state.stroke_props.clone();
        self.device.draw_path(PathDrawMode::FillStroke(rule), &paint, &stroke_props)?;
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
        let font_name = state.font_name.clone().unwrap_or_else(|| "Default".to_string());
        let font_size = state.font_size.unwrap_or(12.0);
        let paint = Paint::from_color(state.fill_color.clone());

        // Extract text string
        if let crate::core::parser::PDFObject::String(bytes) = &args[0] {
            // Decode text (simplified - should use font encoding)
            let text = String::from_utf8_lossy(bytes);
            self.device.draw_text(&text, &font_name, font_size, &paint)?;
        }

        Ok(())
    }

    fn show_spaced_text(&mut self, _args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        // TJ operator - more complex text with individual glyph positioning
        // For now, just show that we're handling it
        eprintln!("Warning: TJ operator not fully implemented");
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
        }

        self.current_state_mut().font_size = Some(extract_number(args, 1)?);
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

    fn set_text_rendering_mode(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
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
            _ => return Err(PDFError::content_stream_error(format!("Invalid text rendering mode: {}", mode))),
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
            _ => return Err(PDFError::content_stream_error(format!("Invalid line cap: {}", cap))),
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
            _ => return Err(PDFError::content_stream_error(format!("Invalid line join: {}", join))),
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
                        ))
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

    fn paint_xobject(&mut self, _args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        // Do operator - paint an XObject (image, form, etc.)
        // For now, just show that we're handling it
        eprintln!("Warning: Do operator not fully implemented");
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

    fn create_context() -> RenderingContext<TestDevice> {
        let device = TestDevice::new(612.0, 792.0);
        RenderingContext::new(device)
    }

    #[test]
    fn test_context_creation() {
        let ctx = create_context();
        assert_eq!(ctx.state_stack.len(), 1);
        assert!(ctx.current_path.is_empty());
    }

    #[test]
    fn test_save_restore() {
        let mut ctx = create_context();

        ctx.current_state_mut().stroke_color = Color::red();
        ctx.save().unwrap();

        ctx.current_state_mut().stroke_color = Color::blue();
        assert_eq!(ctx.current_state().stroke_color, Color::RGB(1.0, 0.0, 0.0));

        ctx.restore().unwrap();
        assert_eq!(ctx.current_state().stroke_color, Color::RGB(1.0, 0.0, 0.0));
    }

    #[test]
    fn test_move_to_operator() {
        let mut ctx = create_context();

        let op = Operation::new(
            OpCode::MoveTo,
            vec![PDFObject::Number(10.0), PDFObject::Number(20.0)],
        );

        ctx.process_operation(&op).unwrap();
        assert_eq!(ctx.current_path.current_point(), Some((10.0, 20.0)));
    }
}
