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
        image_to_device: &[f64; 6],
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
            image_to_device,
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

        // Build the transform that maps the cell bitmap (rendered with
        // a simple scale+translate) into device space.  This carries the
        // Y-flip and any rotation that `image_to_device` encodes.
        // tiny_skia Transform fields: sx, ky, kx, sy, tx, ty.
        let cell_transform = Transform::from_row(
            image_to_device[0] as f32,
            image_to_device[1] as f32,
            image_to_device[2] as f32,
            image_to_device[3] as f32,
            image_to_device[4] as f32,
            image_to_device[5] as f32,
        );

        let (ix0, ix1, iy0, iy1) = tile_bounds;
        let mut tile_count = 0usize;
        for ix in ix0..=ix1 {
            for iy in iy0..=iy1 {
                if tile_count >= 20_000 {
                    return Ok(true);
                }
                tile_count += 1;

                // The per-tile offset is applied through the step-size
                // component of pattern_to_device.  Since we already have
                // image_to_device which includes the base transform, we
                // only need to add the step-based translation.
                let tile_tx = pattern_to_device[0] * (ix as f64 * x_step);
                let tile_ty = pattern_to_device[3] * (iy as f64 * y_step);
                let stamp_transform = Transform::from_translate(
                    tile_tx as f32,
                    tile_ty as f32,
                );
                let combined = cell_transform.pre_translate(stamp_transform.tx, stamp_transform.ty);

                self.pixmap.draw_pixmap(
                    0,
                    0,
                    pattern_pixmap,
                    &pixmap_paint,
                    combined,
                    combined_mask.as_ref(),
                );
            }
        }
        Ok(true)
    }
}
