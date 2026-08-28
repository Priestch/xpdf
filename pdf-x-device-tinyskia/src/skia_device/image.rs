//! Image drawing (`Device` method).

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn draw_image(&mut self, image: ImageData, _transform: &[f64; 6]) -> PDFResult<()> {
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: SkiaDevice::draw_image: {}x{}, transform=[{:?}]",
            image.width, image.height, _transform
        );

        // Convert image data to RGBA format if needed
        let pixel_count = (image.width * image.height) as usize;

        // Image mask: 1-bpp stencil. The core resolves /Decode into mask_paint_one.
        let (mut data, _has_alpha) = if image.is_mask {
            let [fr, fg, fb] = image.mask_color;

            let width = image.width as usize;
            let height = image.height as usize;
            let row_bytes = (width + 7) / 8;
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for y in 0..height {
                let row_start = y.saturating_mul(row_bytes);
                let row = image
                    .data
                    .get(row_start..row_start.saturating_add(row_bytes))
                    .unwrap_or(&[]);
                for x in 0..width {
                    let byte = row.get(x / 8).copied().unwrap_or(0);
                    let bit = 7 - (x % 8);
                    let set = ((byte >> bit) & 1) != 0;
                    let paint = set == image.mask_paint_one;
                    if paint {
                        rgba.extend_from_slice(&[fr, fg, fb, 255]);
                    } else {
                        rgba.extend_from_slice(&[0, 0, 0, 0]);
                    }
                }
            }
            (rgba, true)
        } else {
            match (image.bits_per_component, image.data.len()) {
                // Check for 1-bit black and white images first
                (1, data_len)
                    if data_len >= ((image.width as usize * image.height as usize + 7) / 8) =>
                {
                    #[cfg(feature = "debug-logging")]
                    eprintln!(
                        "DEBUG: Detected 1-bpp image, {}x{}, {} bytes",
                        image.width, image.height, data_len
                    );

                    let width = image.width as usize;
                    let height = image.height as usize;
                    let row_bytes = (width + 7) / 8;
                    let mut rgba = Vec::with_capacity(pixel_count * 4);

                    for y in 0..height {
                        let row_start = y.saturating_mul(row_bytes);
                        let row = image
                            .data
                            .get(row_start..row_start.saturating_add(row_bytes))
                            .unwrap_or(&[]);
                        for x in 0..width {
                            let byte = row.get(x / 8).copied().unwrap_or(0);
                            let bit = 7 - (x % 8);
                            let sample = (byte >> bit) & 1;
                            let gray = if sample == 0 { 0 } else { 255 };
                            rgba.extend_from_slice(&[gray, gray, gray, 255]);
                        }
                    }

                    (rgba, false)
                }
                // Already RGBA (8 bpc, 4 channels, has_alpha flag set)
                // Check this FIRST to avoid confusing RGBA with CMYK (both have 4 channels)
                (8, _) if image.has_alpha && image.data.len() >= pixel_count * 4 => {
                    (premultiply_rgba(&image.data[..pixel_count * 4]), true)
                }

                // RGB to RGBA conversion (8 bpc, 3 channels per pixel)
                // CMYK images are converted to RGB by the core before reaching the device.
                (8, data_len) if data_len >= pixel_count * 3 => {
                    let mut rgba = Vec::with_capacity(pixel_count * 4);
                    for chunk in image.data.chunks(3).take(pixel_count) {
                        if chunk.len() == 3 {
                            rgba.push(chunk[0]); // R
                            rgba.push(chunk[1]); // G
                            rgba.push(chunk[2]); // B
                            rgba.push(255); // A (opaque)
                        }
                    }
                    (rgba, true)
                }

                // Grayscale to RGBA conversion (8 bpc, 1 channel per pixel)
                (8, data_len) if data_len >= pixel_count => {
                    let mut rgba = Vec::with_capacity(pixel_count * 4);
                    for &gray in image.data.iter().take(pixel_count) {
                        rgba.push(gray); // R = gray
                        rgba.push(gray); // G = gray
                        rgba.push(gray); // B = gray
                        rgba.push(255); // A (opaque)
                    }
                    (rgba, true)
                }

                // Grayscale to RGBA conversion (4 bpc, 1 channel per pixel)
                // Each byte contains 2 pixels (high nibble = first pixel, low nibble = second pixel)
                (4, data_len) if data_len >= (pixel_count / 2 + pixel_count % 2) => {
                    let mut rgba = Vec::with_capacity(pixel_count * 4);
                    for &byte in image.data.iter() {
                        // High nibble (first pixel)
                        let high = (byte >> 4) * 17; // Scale 4-bit (0-15) to 8-bit (0-255)
                        rgba.push(high);
                        rgba.push(high);
                        rgba.push(high);
                        rgba.push(255);

                        // Low nibble (second pixel) - only if we need more pixels
                        if rgba.len() < pixel_count * 4 {
                            let low = (byte & 0x0F) * 17; // Scale 4-bit (0-15) to 8-bit (0-255)
                            rgba.push(low);
                            rgba.push(low);
                            rgba.push(low);
                            rgba.push(255);
                        } else {
                            // We have all the pixels we need
                            break;
                        }
                    }
                    // Tiny-skia's Pixmap::from_vec requires capacity == length
                    rgba.shrink_to_fit();
                    (rgba, true)
                }

                _ => {
                    eprintln!(
                        "WARNING: Unsupported image format - has_alpha={}, bits={}, data_len={}, pixel_count={}",
                        image.has_alpha,
                        image.bits_per_component,
                        image.data.len(),
                        pixel_count
                    );
                    // For now, skip unsupported formats
                    return Ok(());
                }
            }
        }; // end match + else

        let logical_width = image.width;
        let logical_height = image.height;
        let mut pixmap_width = image.width;
        let mut pixmap_height = image.height;
        let glyph_image_padding = if self.type3_glyph_depth > 0 && !image.is_mask {
            2
        } else {
            0
        };

        let ctm = self.current_state().transform;

        let image_to_unit =
            Transform::from_translate(-(glyph_image_padding as f32), -(glyph_image_padding as f32))
                .post_concat(Transform::from_scale(
                    1.0 / logical_width as f32,
                    1.0 / logical_height as f32,
                ));

        // IMPORTANT: The transform order matters!
        // We want: final = CTM × image_to_unit
        // Which means: apply image_to_unit first, then CTM
        // In tiny-skia, post_concat means: new = old × transform
        // So we need: image_to_unit.post_concat(ctm)

        let final_transform = image_to_unit.post_concat(ctm);

        // PDF image space maps the top image row to user-space y=1 and the
        // bottom row to y=0. Apply that vertical flip in full affine form, so
        // skewed/rotated images update both the x and y basis vectors.
        let mut adjusted_transform = final_transform;
        adjusted_transform.kx = -final_transform.kx;
        adjusted_transform.sy = -final_transform.sy;
        adjusted_transform.tx = final_transform.tx + final_transform.kx * pixmap_height as f32;
        adjusted_transform.ty = final_transform.ty + final_transform.sy * pixmap_height as f32;

        // Sub-pixel image masks (e.g. 1×1 stencil table rules in
        // image_mask_hairline.pdf scaled to 0.48 pt) under-paint with Nearest
        // sampling. Expand each device axis to at least 1 px about the image
        // center so hairlines remain visible like mutool/hayro.
        if image.is_mask {
            // 1.0 device px still under-paints 1×1 hairlines vs mutool/hayro
            // (image_mask_hairline dark-jaccard ~75%); 1.25 closes the gap
            // without thickening normal masks (scale already ≥1).
            expand_image_mask_to_min_device_pixels(
                &mut adjusted_transform,
                pixmap_width,
                pixmap_height,
                1.25,
            );
        }

        // Type3 stencil masks: Catmull-Rom downsample to device size (hayro
        // Renderer::draw_image when in_type3_glyph uses High + CatmullRom).
        // Nearest on 1bpp yields pure-black blocks under ~8× scale
        // (image_inline_4); hayro/mutool produce soft mid-gray coverage.
        // Source: hayro/hayro/src/renderer.rs draw_image resize path.
        let type3_mask_downsample = image.is_mask
            && self.type3_glyph_depth > 0
            && glyph_image_padding == 0
            && should_prefilter_type3_mask(pixmap_width, pixmap_height, adjusted_transform);

        if type3_mask_downsample {
            let dest_width = (adjusted_transform.sx.hypot(adjusted_transform.ky)
                * pixmap_width as f32)
                .abs()
                .ceil()
                .max(1.0) as u32;
            let dest_height = (adjusted_transform.kx.hypot(adjusted_transform.sy)
                * pixmap_height as f32)
                .abs()
                .ceil()
                .max(1.0) as u32;

            let original_data = std::mem::take(&mut data);
            match catmull_rom_resize_rgba(
                original_data,
                pixmap_width,
                pixmap_height,
                dest_width,
                dest_height,
            ) {
                Ok(resized) => {
                    data = resized;
                    adjusted_transform.sx *= pixmap_width as f32 / dest_width as f32;
                    adjusted_transform.ky *= pixmap_width as f32 / dest_width as f32;
                    adjusted_transform.kx *= pixmap_height as f32 / dest_height as f32;
                    adjusted_transform.sy *= pixmap_height as f32 / dest_height as f32;
                    pixmap_width = dest_width;
                    pixmap_height = dest_height;
                }
                Err(original) => {
                    data = original;
                }
            }
        } else if !image.is_mask
            && glyph_image_padding == 0
            && should_prefilter_downsample(&data, pixmap_width, pixmap_height, adjusted_transform)
        {
            // MuPDF-style subsample + separable cubic. Catmull-Rom (hayro) was
            // tried as a single-stage alternative but regressed image_inline_2
            // vs mutool (~73.7% → ~71.0%); keep the MuPDF chain as default.
            if !image.has_alpha {
                let dest_width = (adjusted_transform.sx.hypot(adjusted_transform.ky)
                    * pixmap_width as f32)
                    .abs()
                    .round()
                    .max(1.0) as u32;
                let dest_height = (adjusted_transform.kx.hypot(adjusted_transform.sy)
                    * pixmap_height as f32)
                    .abs()
                    .round()
                    .max(1.0) as u32;
                if let Some((subsampled, factor)) = mupdf_subsample_rgba(
                    &data,
                    pixmap_width,
                    pixmap_height,
                    dest_width,
                    dest_height,
                ) {
                    data = subsampled;
                    let scale = (1u32 << factor) as f32;
                    adjusted_transform.sx *= scale;
                    adjusted_transform.ky *= scale;
                    adjusted_transform.kx *= scale;
                    adjusted_transform.sy *= scale;
                    pixmap_width = pixmap_width.div_ceil(1u32 << factor);
                    pixmap_height = pixmap_height.div_ceil(1u32 << factor);
                }
            }

            let dest_width = (adjusted_transform.sx.hypot(adjusted_transform.ky)
                * pixmap_width as f32)
                .abs()
                .ceil()
                .max(1.0) as u32;
            let dest_height = (adjusted_transform.kx.hypot(adjusted_transform.sy)
                * pixmap_height as f32)
                .abs()
                .ceil()
                .max(1.0) as u32;

            let original_data = std::mem::take(&mut data);
            match mupdf_simple_resize_rgba(
                original_data,
                pixmap_width,
                pixmap_height,
                dest_width,
                dest_height,
            ) {
                Ok(resized) => {
                    data = resized;
                    adjusted_transform.sx *= pixmap_width as f32 / dest_width as f32;
                    adjusted_transform.ky *= pixmap_width as f32 / dest_width as f32;
                    adjusted_transform.kx *= pixmap_height as f32 / dest_height as f32;
                    adjusted_transform.sy *= pixmap_height as f32 / dest_height as f32;
                    pixmap_width = dest_width;
                    pixmap_height = dest_height;
                }
                Err(original) => {
                    data = original;
                }
            }
        }

        if glyph_image_padding > 0 {
            let pad = glyph_image_padding as usize;
            let row_bytes = pixmap_width as usize * 4;
            let padded_width = pixmap_width as usize + pad * 2;
            let padded_height = pixmap_height as usize + pad * 2;
            let mut padded = vec![0; padded_width * padded_height * 4];

            for y in 0..pixmap_height as usize {
                let src_start = y * row_bytes;
                let dst_start = ((y + pad) * padded_width + pad) * 4;
                padded[dst_start..dst_start + row_bytes]
                    .copy_from_slice(&data[src_start..src_start + row_bytes]);
            }

            data = padded;
            pixmap_width = padded_width as u32;
            pixmap_height = padded_height as u32;
        }

        let int_size = tiny_skia::IntSize::from_wh(pixmap_width, pixmap_height)
            .ok_or(PDFError::Generic("Failed to create IntSize".into()))?;

        let image_pixmap = Pixmap::from_vec(data, int_size)
            .ok_or(PDFError::Generic("Failed to create image pixmap".into()))?;

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Image pixmap created: {}x{}",
            image_pixmap.width(),
            image_pixmap.height()
        );

        // Check if the image has any non-white pixels
        #[cfg(feature = "debug-logging")]
        {
            let mut non_white_count = 0;
            let pixel_data = image_pixmap.data();
            for i in 0..(100 * 4).min(pixel_data.len()) {
                if i % 4 == 0
                    && (pixel_data[i] != 255
                        || pixel_data[i + 1] != 255
                        || pixel_data[i + 2] != 255)
                {
                    non_white_count += 1;
                }
            }
            eprintln!(
                "DEBUG: Image pixmap has {} non-white pixels in first 100",
                non_white_count
            );
        }

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: CTM={:?}, final_transform={:?}, adjusted_transform={:?}",
            ctm, final_transform, adjusted_transform
        );

        let clip_mask = self.get_clip_mask();

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: image_to_unit={:?}, final_transform={:?}",
            image_to_unit, final_transform
        );
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: adjusted_transform={:?}", adjusted_transform);

        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: clip_mask.is_some()={}", clip_mask.is_some());

        let mut pixmap_paint = tiny_skia::PixmapPaint::default();
        pixmap_paint.blend_mode = to_skia_blend_mode(self.current_state().blend_mode);
        pixmap_paint.opacity = self.current_state().opacity.clamp(0.0, 1.0);
        if image.interpolate
            || (self.type3_glyph_depth > 0 && !image.is_mask)
            || should_auto_bilinear(&image, adjusted_transform)
        {
            pixmap_paint.quality = FilterQuality::Bilinear;
        }

        self.pixmap.draw_pixmap(
            0,
            0,
            image_pixmap.as_ref(),
            &pixmap_paint,
            adjusted_transform,
            clip_mask.as_ref(),
        );

        Ok(())
    }
}
