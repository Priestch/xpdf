//! Path construction, painting, and clipping (`Device` methods).

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn begin_path(&mut self) {
        self.path_builder = PathBuilder::new();
    }

    pub(super) fn move_to(&mut self, x: f64, y: f64) {
        self.path_builder.move_to(x as f32, y as f32);
    }

    pub(super) fn line_to(&mut self, x: f64, y: f64) {
        self.path_builder.line_to(x as f32, y as f32);
    }

    pub(super) fn curve_to(&mut self, cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64) {
        self.path_builder.cubic_to(
            cp1x as f32,
            cp1y as f32,
            cp2x as f32,
            cp2y as f32,
            x as f32,
            y as f32,
        );
    }

    pub(super) fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        // Handle invalid rectangles gracefully
        // PDFs may have negative or zero dimensions, or NaN values
        let xf = x as f32;
        let yf = y as f32;
        let wf = width as f32;
        let hf = height as f32;

        // Create a valid rectangle, clamping to reasonable bounds
        if wf > 0.0
            && hf > 0.0
            && xf.is_finite()
            && yf.is_finite()
            && wf.is_finite()
            && hf.is_finite()
        {
            if let Some(rect) = Rect::from_xywh(xf, yf, wf, hf) {
                self.path_builder.push_rect(rect);
                return;
            }
        }

        // Fallback: draw rectangle using lines for invalid rects
        self.move_to(x, y);
        self.line_to(x + width, y);
        self.line_to(x + width, y + height);
        self.line_to(x, y + height);
        self.close_path();
    }

    pub(super) fn close_path(&mut self) {
        self.path_builder.close();
    }

    pub(super) fn draw_path(
        &mut self,
        mode: PathDrawMode,
        paint: &Paint,
        stroke_props: &StrokeProps,
    ) -> PDFResult<()> {
        // Take ownership of the path_builder and create a new one
        let path = std::mem::replace(&mut self.path_builder, PathBuilder::new())
            .finish()
            .ok_or(PDFError::Generic("Invalid path".into()))?;

        let transform = self.current_state().transform;

        // Debug logging for path info
        #[cfg(feature = "debug-logging")]
        {
            if self.draw_count < 5 {
                eprintln!(
                    "DEBUG: draw_path #{} path.bounds={:?}",
                    self.draw_count,
                    path.bounds()
                );
                eprintln!(
                    "DEBUG: draw_path #{} transform={:?}",
                    self.draw_count, transform
                );
                eprintln!(
                    "DEBUG: draw_path #{} pixmap.size={}x{}",
                    self.draw_count,
                    self.pixmap.width(),
                    self.pixmap.height()
                );
            }
        }

        // Track colors
        let color_str = format!("{:?}", paint);
        *self.colors_seen.entry(color_str.clone()).or_insert(0) += 1;

        // Debug logging (first 20 draws only, always log non-white)
        #[cfg(feature = "debug-logging")]
        {
            let is_white = matches!(
                paint,
                Paint::Solid(pdf_x_core::rendering::Color::RGB(1.0, 1.0, 1.0))
            );
            if self.draw_count < 20 || !is_white {
                eprintln!(
                    "DEBUG: draw_path #{} mode={:?}, paint={}",
                    self.draw_count, mode, color_str
                );
            }

            // Milestone logging
            if self.draw_count == 100 || self.draw_count == 500 || self.draw_count == 1000 {
                eprintln!("DEBUG: {} draws completed, color summary:", self.draw_count);
                let mut colors: Vec<_> = self.colors_seen.iter().collect();
                colors.sort_by(|a, b| b.1.cmp(a.1));
                for (color, count) in colors.iter().take(5) {
                    eprintln!("  {}: {} draws", color, count);
                }
            }
        }

        self.draw_count += 1;

        let blend_mode = self.current_state().blend_mode;
        let sk_paint = to_skia_paint(paint, blend_mode, self.current_state().opacity);
        let clip_mask = self.get_clip_mask();

        match mode {
            PathDrawMode::Fill(fill_rule) => {
                self.pixmap.fill_path(
                    &path,
                    &sk_paint,
                    to_skia_fill_rule(fill_rule),
                    transform,
                    clip_mask.as_ref(),
                );
            }
            PathDrawMode::Stroke => {
                let sk_stroke = to_skia_stroke(
                    stroke_props,
                    transform,
                    stroke_props.min_device_width as f32,
                );
                self.pixmap.stroke_path(
                    &path,
                    &sk_paint,
                    &sk_stroke,
                    transform,
                    clip_mask.as_ref(),
                );
            }
            PathDrawMode::FillStroke(fill_rule) => {
                self.pixmap.fill_path(
                    &path,
                    &sk_paint,
                    to_skia_fill_rule(fill_rule),
                    transform,
                    clip_mask.as_ref(),
                );
                let sk_stroke = to_skia_stroke(
                    stroke_props,
                    transform,
                    stroke_props.min_device_width as f32,
                );
                self.pixmap.stroke_path(
                    &path,
                    &sk_paint,
                    &sk_stroke,
                    transform,
                    clip_mask.as_ref(),
                );
            }
        }

        Ok(())
    }

    pub(super) fn clip_path(&mut self, rule: FillRule) -> PDFResult<()> {
        // Take ownership of the path_builder and create a new one
        let path = std::mem::replace(&mut self.path_builder, PathBuilder::new())
            .finish()
            .ok_or(PDFError::Generic("Invalid path".into()))?;

        // Create the clip mask immediately using the current CTM
        // This ensures the mask is in the correct coordinate space
        let transform = self.current_state().transform;
        let fill_rule = match rule {
            FillRule::NonZero => SkiaFillRule::Winding,
            FillRule::EvenOdd => SkiaFillRule::EvenOdd,
        };

        if let Some(mut mask) = Mask::new(self.pixmap.width(), self.pixmap.height()) {
            mask.fill_path(&path, fill_rule, true, transform);
            self.current_state_mut().clip_mask = match self.current_state().clip_mask.as_ref() {
                Some(existing) => intersect_masks(existing, &mask),
                None => Some(mask),
            };
        } else {
            // If mask creation failed, just don't clip
            self.current_state_mut().clip_mask = None;
        }

        Ok(())
    }

    pub(super) fn clip_stroke_path(&mut self, stroke_props: &StrokeProps) -> PDFResult<()> {
        let path = std::mem::replace(&mut self.path_builder, PathBuilder::new())
            .finish()
            .ok_or(PDFError::Generic("Invalid path".into()))?;

        let transform = self.current_state().transform;
        let Some(mut stroke_pixmap) = Pixmap::new(self.pixmap.width(), self.pixmap.height()) else {
            self.current_state_mut().clip_mask = None;
            return Ok(());
        };

        let mut paint = SkiaPaint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;
        let stroke = to_skia_stroke(
            stroke_props,
            transform,
            stroke_props.min_device_width as f32,
        );
        stroke_pixmap.stroke_path(&path, &paint, &stroke, transform, None);

        let stroke_mask = Mask::from_pixmap(stroke_pixmap.as_ref(), MaskType::Alpha);
        self.current_state_mut().clip_mask = match self.current_state().clip_mask.as_ref() {
            Some(existing) => {
                if existing.width() == stroke_mask.width()
                    && existing.height() == stroke_mask.height()
                {
                    let combined: Vec<u8> = existing
                        .data()
                        .iter()
                        .zip(stroke_mask.data().iter())
                        .map(|(&a, &b)| a.min(b))
                        .collect();
                    Mask::from_vec(
                        combined,
                        tiny_skia::IntSize::from_wh(existing.width(), existing.height())
                            .ok_or(PDFError::Generic("Invalid clip mask size".into()))?,
                    )
                } else {
                    Some(stroke_mask)
                }
            }
            None => Some(stroke_mask),
        };

        Ok(())
    }
}
