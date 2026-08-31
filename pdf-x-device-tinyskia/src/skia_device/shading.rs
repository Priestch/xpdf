//! Shading fill (`Device` method).

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn draw_shading(
        &mut self,
        shading: &pdf_x_core::rendering::shading::Shading,
        ctm: &[f64; 6],
    ) -> PDFResult<()> {
        let w = self.pixmap.width();
        let h = self.pixmap.height();
        let clip_mask = self.get_clip_mask();

        // Inverse CTM to map device pixels → shading space.
        let (a, b, c, d, e, f) = (
            ctm[0] as f32,
            ctm[1] as f32,
            ctm[2] as f32,
            ctm[3] as f32,
            ctm[4] as f32,
            ctm[5] as f32,
        );
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            return Ok(());
        }
        let inv_det = 1.0 / det;

        let clip_data = clip_mask.as_ref().map(|m| m.data());
        let opacity = self.current_state().opacity.clamp(0.0, 1.0);
        let sample_offset = if shading.uses_pixel_center_sampling() {
            0.5
        } else {
            0.0
        };

        for y in 0..h {
            for x in 0..w {
                let px = x as usize;
                let py = y as usize;
                let pixel_idx = py * w as usize + px;

                // Check clip mask
                let mask_alpha = if let Some(cd) = &clip_data {
                    let alpha = cd[pixel_idx] as f32 / 255.0;
                    if alpha <= 0.0 {
                        continue;
                    }
                    alpha
                } else {
                    1.0
                };

                // Device pixel → shading space via inverse CTM
                let dx = x as f32 + sample_offset;
                let dy = y as f32 + sample_offset;
                let sx = (d * (dx - e) - c * (dy - f)) * inv_det;
                let sy = (-b * (dx - e) + a * (dy - f)) * inv_det;

                if let Some((r, g, b_val, a_val)) = shading.sample(sx as f64, sy as f64) {
                    let offset = pixel_idx * 4;
                    let data = self.pixmap.data_mut();
                    if offset + 3 < data.len() {
                        // Alpha blend with existing pixel (source-over)
                        let alpha = (a_val as f32 / 255.0) * opacity * mask_alpha;
                        if alpha <= 0.0 {
                            continue;
                        }
                        let inv_alpha = 1.0 - alpha;
                        data[offset] = (r as f32 * alpha + data[offset] as f32 * inv_alpha)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                        data[offset + 1] = (g as f32 * alpha + data[offset + 1] as f32 * inv_alpha)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                        data[offset + 2] = (b_val as f32 * alpha
                            + data[offset + 2] as f32 * inv_alpha)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                        data[offset + 3] = ((alpha + (data[offset + 3] as f32 / 255.0) * inv_alpha)
                            * 255.0)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }

        Ok(())
    }
}
