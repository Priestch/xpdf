//! Offscreen soft-mask rendering (`Device` methods).

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn begin_offscreen(&mut self) -> PDFResult<()> {
        self.offscreen_stack.push(self.pixmap.data_mut().to_vec());
        self.pixmap.data_mut().fill(0);
        Ok(())
    }

    pub(super) fn end_offscreen_as_mask(
        &mut self,
        luminosity: bool,
        backdrop: Option<[u8; 3]>,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let saved = self.offscreen_stack.pop()?;
        let w = self.pixmap.width();
        let h = self.pixmap.height();
        let src = self.pixmap.data_mut();

        let mask_data: Vec<u8> = (0..(w * h) as usize)
            .map(|i| {
                let offset = i * 4;
                let a = src[offset + 3] as f32 / 255.0;
                if luminosity {
                    // Un-premultiply, composite over the SMask backdrop color,
                    // then compute PDF luminosity. Alpha masks ignore /BC.
                    if a > 0.0 {
                        let mut r = (src[offset] as f32 / 255.0) / a;
                        let mut g = (src[offset + 1] as f32 / 255.0) / a;
                        let mut b = (src[offset + 2] as f32 / 255.0) / a;
                        if let Some([br, bg, bb]) = backdrop {
                            let inv_a = 1.0 - a;
                            r = r * a + (br as f32 / 255.0) * inv_a;
                            g = g * a + (bg as f32 / 255.0) * inv_a;
                            b = b * a + (bb as f32 / 255.0) * inv_a;
                        }
                        (0.299 * r + 0.587 * g + 0.114 * b)
                            .mul_add(255.0, 0.0)
                            .round()
                            .clamp(0.0, 255.0) as u8
                    } else if let Some([br, bg, bb]) = backdrop {
                        (0.299 * br as f32 + 0.587 * bg as f32 + 0.114 * bb as f32)
                            .round()
                            .clamp(0.0, 255.0) as u8
                    } else {
                        0
                    }
                } else {
                    (a * 255.0) as u8
                }
            })
            .collect();

        // Restore original pixels
        self.pixmap.data_mut().copy_from_slice(&saved);
        Some((mask_data, w, h))
    }

    pub(super) fn end_offscreen_as_rgba_region(
        &mut self,
        width: u32,
        height: u32,
    ) -> Option<Vec<u8>> {
        let saved = self.offscreen_stack.pop()?;
        let w = self.pixmap.width();
        let h = self.pixmap.height();
        let copy_w = width.min(w);
        let copy_h = height.min(h);
        let src = self.pixmap.data_mut().to_vec();
        let mut rgba = vec![
            0;
            (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(4)
        ];

        for y in 0..copy_h as usize {
            let src_start = y * w as usize * 4;
            let src_end = src_start + copy_w as usize * 4;
            let dst_start = y * width as usize * 4;
            let dst_end = dst_start + copy_w as usize * 4;
            rgba[dst_start..dst_end].copy_from_slice(&src[src_start..src_end]);
        }

        self.pixmap.data_mut().copy_from_slice(&saved);
        Some(rgba)
    }

    pub(super) fn end_offscreen_composite(&mut self, opacity: f64) -> PDFResult<()> {
        let Some(saved) = self.offscreen_stack.pop() else {
            return Ok(());
        };
        let opacity = (opacity as f32).clamp(0.0, 1.0);
        let clip_mask = self.get_clip_mask();
        let clip_data = clip_mask.as_ref().map(|mask| mask.data().to_vec());
        let src = self.pixmap.data_mut().to_vec();
        let dst = self.pixmap.data_mut();

        dst.copy_from_slice(&saved);
        for (pixel_index, (src_px, dst_px)) in
            src.chunks_exact(4).zip(dst.chunks_exact_mut(4)).enumerate()
        {
            let mask_alpha = clip_data
                .as_ref()
                .and_then(|data| data.get(pixel_index).copied())
                .unwrap_or(255) as f32
                / 255.0;
            let group_alpha = opacity * mask_alpha;
            let src_alpha = (src_px[3] as f32 / 255.0) * group_alpha;
            if src_alpha <= 0.0 {
                continue;
            }
            let inv_alpha = 1.0 - src_alpha;
            dst_px[0] = (src_px[0] as f32 * group_alpha + dst_px[0] as f32 * inv_alpha)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst_px[1] = (src_px[1] as f32 * group_alpha + dst_px[1] as f32 * inv_alpha)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst_px[2] = (src_px[2] as f32 * group_alpha + dst_px[2] as f32 * inv_alpha)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst_px[3] = ((src_alpha + (dst_px[3] as f32 / 255.0) * inv_alpha) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }

        Ok(())
    }
}
