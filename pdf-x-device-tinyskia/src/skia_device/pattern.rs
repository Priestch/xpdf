//! Pattern-cell fill (`Device` methods).

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn supports_pattern_cell_fill(&self) -> bool {
        true
    }

    pub(super) fn fill_path_with_pattern_cell(
        &mut self,
        rule: FillRule,
        rgba: &[u8],
        width: u32,
        height: u32,
        _image_to_device: &[f64; 6],
        pattern_to_device: &[f64; 6],
        bbox: &[f64; 4],
        x_step: f64,
        y_step: f64,
        tile_bounds: (i32, i32, i32, i32),
    ) -> PDFResult<bool> {
        if pattern_to_device[1].abs() > 1e-6 || pattern_to_device[2].abs() > 1e-6 {
            return Ok(false);
        }
        if width == 0 || height == 0 || x_step.abs() <= f64::EPSILON || y_step.abs() <= f64::EPSILON
        {
            return Ok(false);
        }

        let path = std::mem::replace(&mut self.path_builder, PathBuilder::new())
            .finish()
            .ok_or(PDFError::Generic("Invalid path".into()))?;

        #[cfg(feature = "debug-logging")]
        trace_pattern_cell_fill(
            rgba,
            width,
            height,
            _image_to_device,
            path.bounds(),
            self.current_state().transform,
        );

        let mut path_mask = Mask::new(self.pixmap.width(), self.pixmap.height())
            .ok_or_else(|| PDFError::Generic("Failed to allocate tiling pattern mask".into()))?;
        path_mask.fill_path(
            &path,
            to_skia_fill_rule(rule),
            true,
            self.current_state().transform,
        );
        let clip_mask = self.get_clip_mask();
        let combined_mask = match clip_mask.as_ref() {
            Some(clip) => intersect_masks(&path_mask, clip),
            None => Some(path_mask),
        };

        let cell_bbox = transform_rect((bbox[0], bbox[1], bbox[2], bbox[3]), pattern_to_device);
        let left_offset = cell_bbox.0 - pattern_to_device[4];
        let top_offset = cell_bbox.1 - pattern_to_device[5];
        let mut pixmap_paint = tiny_skia::PixmapPaint::default();
        pixmap_paint.opacity = self.current_state().opacity;
        pixmap_paint.blend_mode = to_skia_blend_mode(self.current_state().blend_mode);
        pixmap_paint.quality = FilterQuality::Nearest;

        let Some(pattern_pixmap) = PixmapRef::from_bytes(rgba, width, height) else {
            return Ok(false);
        };
        let (ix0, ix1, iy0, iy1) = tile_bounds;
        let mut tile_count = 0usize;
        for ix in ix0..=ix1 {
            for iy in iy0..=iy1 {
                if tile_count >= 20_000 {
                    return Ok(true);
                }
                tile_count += 1;

                let original_x = pattern_to_device[0] * (ix as f64 * x_step) + pattern_to_device[4];
                let original_y = pattern_to_device[3] * (iy as f64 * y_step) + pattern_to_device[5];
                let draw_x = original_x + left_offset;
                let draw_y = original_y + top_offset;
                let int_x = draw_x.floor();
                let int_y = draw_y.floor();
                let transform =
                    Transform::from_translate((draw_x - int_x) as f32, (draw_y - int_y) as f32);
                self.pixmap.draw_pixmap(
                    int_x as i32,
                    int_y as i32,
                    pattern_pixmap,
                    &pixmap_paint,
                    transform,
                    combined_mask.as_ref(),
                );
            }
        }
        Ok(true)
    }
}
