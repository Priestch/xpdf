//! Path construction, painting, and clipping operators (m/l/c/h/re, S/s/f/f*/B/b*/n, W/W*).

use super::*;

impl<'a, D: Device> RenderingContext<'a, D> {
    pub(super) fn move_to(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
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
    pub(super) fn line_to(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
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
    pub(super) fn curve_to(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
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
    pub(super) fn curve_to_2(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
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
    pub(super) fn curve_to_3(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
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
    pub(super) fn close_path(&mut self) -> PDFResult<()> {
        self.current_path.close_path();
        self.device.close_path();
        Ok(())
    }
    pub(super) fn rectangle(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
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
    pub(super) fn stroke(&mut self) -> PDFResult<()> {
        // PDF spec §6.2.11: /None colorant produces no visible output.
        if self.current_state().stroke_color_is_none {
            self.current_path.begin();
            return Ok(());
        }
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
    pub(super) fn close_and_stroke(&mut self) -> PDFResult<()> {
        self.close_path()?;
        self.stroke()
    }
    pub(super) fn fill(&mut self, rule: FillRule) -> PDFResult<()> {
        // PDF spec §6.2.11: /None colorant produces no visible output.
        if self.current_state().fill_color_is_none {
            self.current_path.begin();
            return Ok(());
        }

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
    pub(super) fn fill_and_stroke(&mut self, rule: FillRule) -> PDFResult<()> {
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
    pub(super) fn close_fill_stroke(&mut self, rule: FillRule) -> PDFResult<()> {
        self.close_path()?;
        self.fill_and_stroke(rule)
    }
    pub(super) fn end_path(&mut self) -> PDFResult<()> {
        self.apply_pending_clip()?;
        self.current_path.begin();
        self.device.begin_path();
        Ok(())
    }
    pub(super) fn effective_fill_color(&self) -> Color {
        self.type3_shape_color
            .or(self.stencil_pattern_color)
            .unwrap_or_else(|| self.current_state().fill_color)
    }
    pub(super) fn effective_stroke_color(&self) -> Color {
        self.type3_shape_color
            .or(self.stencil_pattern_color)
            .unwrap_or_else(|| self.current_state().stroke_color)
    }
    pub(super) fn replay_path_to_device(&mut self, path: &Path) {
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
    pub(super) fn clip(&mut self, rule: FillRule) -> PDFResult<()> {
        self.pending_clip = Some(rule);
        Ok(())
    }
    pub(super) fn apply_pending_clip(&mut self) -> PDFResult<()> {
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
}
