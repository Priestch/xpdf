//! Text drawing (`Device` methods): shaping, glyph runs, text clipping.

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn draw_text(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        paint: &Paint,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<f64> {
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: draw_text called with font='{}', size={}, bytes={:?}, text_matrix={:?}, hscale={}, rise={}, Tc={}, Tw={}",
            font_name,
            font_size,
            text_bytes,
            text_matrix,
            horizontal_scaling,
            text_rise,
            char_spacing,
            word_spacing
        );

        // Get font, or silently skip if not available
        // Check if Type1 first
        let is_type1 = if let Some(font) = self.font_cache.get(font_name) {
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Font cache hit for '{}', is_type1={}",
                font_name,
                font.is_type1()
            );
            font.is_type1()
        } else {
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Font cache MISS for '{}' - skipping text rendering",
                font_name
            );
            return Ok(0.0); // Silent failure - return 0 width
        };

        if !is_type1 {
            // TrueType font rendering
            let uses_raw_cid_codes = self
                .font_cache
                .get(font_name)
                .map(StoredFont::uses_raw_cid_codes)
                .unwrap_or(false);
            if uses_raw_cid_codes {
                // skrifa/ttf_parser outlines are placed in 1000-em space (hayro
                // UNITS_PER_EM). PDF `/W`/`/DW` are already thousandths of text
                // space (§9.7.4.3), so path advances use pdf_to_font = 1.
                let default_width = {
                    let font = match self.font_cache.get(font_name) {
                        Some(f) => f,
                        None => return Ok(0.0),
                    };
                    font.pdf_default_width
                };
                let units_per_em = OUTLINE_UPEM;
                let units_per_em_f64 = units_per_em as f64;
                let pdf_to_font = 1.0f32;
                let horizontal_scale = horizontal_scaling / 100.0;
                let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
                    units_per_em_f64 / font_size
                } else {
                    0.0
                };

                let mut text_path_builder = PathBuilder::new();
                let mut current_x = 0.0f32;
                let mut total_advance_user = 0.0f64;

                for cid in decode_be_cids(text_bytes) {
                    let width_pdf = self
                        .font_cache
                        .get(font_name)
                        .map(|f| f.pdf_width_for_code(cid))
                        .unwrap_or(default_width);
                    let width_font = width_pdf * pdf_to_font;
                    let spacing_font_units = (char_spacing * spacing_to_font_units) as f32;

                    if let Some(glyph_path) = self
                        .font_cache
                        .get_mut(font_name)
                        .unwrap()
                        .cached_cid_glyph_path(cid)
                    {
                        let transform = Transform::from_row(1.0, 0.0, 0.0, 1.0, current_x, 0.0);
                        if let Some(path_transformed) = glyph_path.transform(transform) {
                            text_path_builder.push_path(&path_transformed);
                        }
                    }

                    current_x += width_font + spacing_font_units;
                    // Text-space advance: w/1000 * Tfs (+ Tc). Independent of upem.
                    total_advance_user += (width_pdf as f64 * font_size / 1000.0) + char_spacing;
                }

                let total_rendered_width = total_advance_user * horizontal_scale;

                if let Some(path) = text_path_builder.finish() {
                    let ctm = self.current_state().transform;
                    let full_text = compose_text_transform(
                        text_matrix,
                        font_size,
                        horizontal_scaling,
                        text_rise,
                        units_per_em,
                    );
                    let final_transform = full_text.post_concat(ctm);
                    let clip_mask = self.get_clip_mask();

                    self.paint_text_path(&path, paint, final_transform, ctm, clip_mask.as_ref());
                }

                return Ok(total_rendered_width);
            }

            let font = match self.font_cache.get(font_name) {
                Some(f) => f,
                None => return Ok(0.0),
            };

            let face = font.face().expect("TrueType font should have face");
            let units_per_em = face.units_per_em() as f32;
            let units_per_em_f64 = units_per_em as f64;
            let horizontal_scale = horizontal_scaling / 100.0;
            let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
                units_per_em_f64 / font_size
            } else {
                0.0
            };

            // Simple fonts with `/Widths`: honor PDF metrics and Encoding instead of
            // FreeType/harfbuzz advances. Critical for unembedded Type1 substitutes
            // (Garamond → Liberation) whose face metrics disagree with the PDF /Widths.
            // PDF 32000-1:2008 §9.6.3 / §9.7.4.3.
            if font.pdf_widths.is_some() {
                let pdf_to_font = units_per_em / 1000.0;
                let encoding = font.simple_encoding.clone();
                let default_width = font.pdf_default_width;

                let mut text_path_builder = PathBuilder::new();
                let mut current_x = 0.0f32;
                let mut total_advance_user = 0.0f64;

                for &byte in text_bytes {
                    let width_pdf = self
                        .font_cache
                        .get(font_name)
                        .map(|f| f.pdf_width_for_code(byte as u16))
                        .unwrap_or(default_width);
                    let width_font = width_pdf * pdf_to_font;

                    let mut spacing_user = char_spacing;
                    if byte == b' ' {
                        spacing_user += word_spacing;
                    }
                    let spacing_font = (spacing_user * spacing_to_font_units) as f32;

                    if let Some(gid) = self.font_cache.get(font_name).and_then(|f| {
                        f.face().and_then(|face| {
                            true_type_glyph_id_for_code(face, byte, encoding.as_ref())
                        })
                    }) {
                        let mut converter = PathConverter(PathBuilder::new());
                        let _ = self
                            .font_cache
                            .get(font_name)
                            .and_then(|f| f.face())
                            .map(|face| face.outline_glyph(gid, &mut converter));
                        if let Some(p) = converter.0.finish() {
                            let transform = Transform::from_row(1.0, 0.0, 0.0, 1.0, current_x, 0.0);
                            if let Some(path_transformed) = p.transform(transform) {
                                text_path_builder.push_path(&path_transformed);
                            }
                        }
                    }

                    current_x += width_font + spacing_font;
                    total_advance_user += (width_pdf as f64 * font_size / 1000.0) + spacing_user;
                }

                let total_rendered_width = total_advance_user * horizontal_scale;

                if let Some(path) = text_path_builder.finish() {
                    let ctm = self.current_state().transform;
                    let full_text = compose_text_transform(
                        text_matrix,
                        font_size,
                        horizontal_scaling,
                        text_rise,
                        units_per_em,
                    );
                    let final_transform = full_text.post_concat(ctm);
                    let clip_mask = self.get_clip_mask();
                    self.paint_text_path(&path, paint, final_transform, ctm, clip_mask.as_ref());
                }

                return Ok(total_rendered_width);
            }

            // Decode text bytes for shaping. Prefer UTF-16 when bytes look like
            // composite-font character codes; otherwise keep legacy Latin-1 mapping.
            let decoded_run = decode_text_run(text_bytes);
            let text = decoded_run.text.as_str();

            let shaped_buffer = match font.shape(&text) {
                Some(buffer) => buffer,
                None => return Ok(0.0), // Shaping failed
            };
            let glyph_infos = shaped_buffer.glyph_infos();
            let glyph_positions = shaped_buffer.glyph_positions();

            let mut text_path_builder = PathBuilder::new();

            // Keep glyph positions local to the text run; text_matrix applies run placement.
            let mut current_x = 0.0f32;
            let mut current_y = 0.0f32;
            let mut total_advance_user = 0.0f64;

            for (idx, (info, pos)) in glyph_infos.iter().zip(glyph_positions.iter()).enumerate() {
                let mut converter = PathConverter(PathBuilder::new());
                // Keep outlines in font units and only translate by glyph advances.
                let transform = Transform::from_row(
                    1.0,
                    0.0,
                    0.0,
                    1.0,
                    current_x + pos.x_offset as f32,
                    current_y + pos.y_offset as f32,
                );

                let _ = font
                    .face()
                    .expect("TrueType font should have face")
                    .outline_glyph(ttf_parser::GlyphId(info.glyph_id as u16), &mut converter);

                if let Some(p) = converter.0.finish() {
                    if let Some(path_transformed) = p.transform(transform) {
                        text_path_builder.push_path(&path_transformed);
                    }
                }

                let spacing_user = decoded_run
                    .spacing_codes
                    .get(idx)
                    .map(|&code| {
                        let mut s = char_spacing;
                        // Per PDF spec, word spacing is defined for simple single-byte
                        // codes and should not be applied to multibyte CIDs.
                        if !decoded_run.is_multibyte_codes && code == 0x20 {
                            s += word_spacing;
                        }
                        s
                    })
                    .unwrap_or(0.0);
                let spacing_font_units = (spacing_user * spacing_to_font_units) as f32;

                current_x += pos.x_advance as f32 + spacing_font_units;
                current_y += pos.y_advance as f32;
                total_advance_user +=
                    (pos.x_advance as f64 * (font_size / units_per_em_f64)) + spacing_user;
            }

            for &code in decoded_run.spacing_codes.iter().skip(glyph_positions.len()) {
                let mut spacing_user = char_spacing;
                if !decoded_run.is_multibyte_codes && code == 0x20 {
                    spacing_user += word_spacing;
                }
                total_advance_user += spacing_user;
            }

            let total_rendered_width = total_advance_user * horizontal_scale;

            if let Some(path) = text_path_builder.finish() {
                let ctm = self.current_state().transform;
                let full_text = compose_text_transform(
                    text_matrix,
                    font_size,
                    horizontal_scaling,
                    text_rise,
                    units_per_em,
                );

                // Then apply CTM
                let final_transform = full_text.post_concat(ctm);
                let clip_mask = self.get_clip_mask();

                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Text CTM={:?}, text_matrix=[{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}], hscale={:.1}%, rise={:.1}, full_text={:?}, final={:?}",
                    ctm,
                    text_matrix[0],
                    text_matrix[1],
                    text_matrix[2],
                    text_matrix[3],
                    text_matrix[4],
                    text_matrix[5],
                    horizontal_scaling,
                    text_rise,
                    full_text,
                    final_transform
                );

                self.paint_text_path(&path, paint, final_transform, ctm, clip_mask.as_ref());
            }

            return Ok(total_rendered_width);
        }

        // Type1/CFF font rendering
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Type1/CFF font rendering, {:?} bytes",
            text_bytes.len()
        );

        // Get font widths and type1 font ref (clone type1 for width queries).
        // Type1 glyph space is 1000 units/em, matching PDF `/W`/`/DW` units.
        let (type1_font, uses_raw_cid_codes, default_width) = {
            let font = self.font_cache.get(font_name).unwrap();
            (
                font.type1_font().unwrap().clone(),
                font.uses_raw_cid_codes(),
                font.pdf_default_width,
            )
        };

        let units_per_em = 1000.0f32;
        let units_per_em_f64 = units_per_em as f64;
        let horizontal_scale = horizontal_scaling / 100.0;
        let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
            units_per_em_f64 / font_size
        } else {
            0.0
        };

        let mut text_path_builder = PathBuilder::new();

        let mut current_x = 0.0f32;
        let current_y = 0.0f32;
        let mut total_advance_user = 0.0f64;

        if uses_raw_cid_codes {
            for cid in decode_be_cids(text_bytes) {
                let glyph_width = self
                    .font_cache
                    .get(font_name)
                    .map(|f| f.pdf_width_for_code(cid))
                    .unwrap_or(default_width);
                let spacing_font_units = (char_spacing * spacing_to_font_units) as f32;

                let path_opt = self
                    .font_cache
                    .get_mut(font_name)
                    .unwrap()
                    .cached_cid_glyph_path(cid);
                if let Some(glyph_path) = path_opt {
                    let transform = Transform::from_row(1.0, 0.0, 0.0, 1.0, current_x, current_y);
                    if let Some(path_transformed) = glyph_path.transform(transform) {
                        text_path_builder.push_path(&path_transformed);
                    }
                }

                current_x += glyph_width + spacing_font_units;
                total_advance_user += (glyph_width as f64 * font_size / 1000.0) + char_spacing;
            }
        } else {
            for &byte in text_bytes {
                let ch = char::from(byte);
                let glyph_width = self
                    .font_cache
                    .get(font_name)
                    .and_then(|f| {
                        f.pdf_widths
                            .as_ref()
                            .and_then(|w| w.get(byte as usize).copied())
                    })
                    .unwrap_or(type1_font.glyph_width(ch) as f32);
                let mut spacing_user = char_spacing;
                if byte == b' ' {
                    spacing_user += word_spacing;
                }
                let spacing_font_units = (spacing_user * spacing_to_font_units) as f32;

                // Use cached glyph outline — avoids re-parsing Type1 charstrings.
                if let Some(glyph_path) = self
                    .font_cache
                    .get_mut(font_name)
                    .unwrap()
                    .cached_glyph_path(byte)
                {
                    let transform = Transform::from_row(1.0, 0.0, 0.0, 1.0, current_x, current_y);
                    if let Some(path_transformed) = glyph_path.transform(transform) {
                        text_path_builder.push_path(&path_transformed);
                    }
                }

                // Always advance even when there is no outline (e.g. space).
                current_x += glyph_width + spacing_font_units;
                total_advance_user += (glyph_width as f64 * font_size / 1000.0) + spacing_user;
            }
        }

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Text rendering: total advance {:.3}",
            total_advance_user
        );

        let total_rendered_width = total_advance_user * horizontal_scale;

        if let Some(path) = text_path_builder.finish() {
            #[cfg(feature = "debug-logging")]
            eprintln!("DEBUG: Text path created successfully");

            let ctm = self.current_state().transform;

            let full_text = compose_text_transform(
                text_matrix,
                font_size,
                horizontal_scaling,
                text_rise,
                units_per_em,
            );

            // Then apply CTM
            let final_transform = full_text.post_concat(ctm);
            let clip_mask = self.get_clip_mask();

            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: CTM={:?}, text_matrix=[{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}], hscale={:.1}%, rise={:.1}, full_text={:?}, final={:?}",
                ctm,
                text_matrix[0],
                text_matrix[1],
                text_matrix[2],
                text_matrix[3],
                text_matrix[4],
                text_matrix[5],
                horizontal_scaling,
                text_rise,
                full_text,
                final_transform
            );

            self.paint_text_path(&path, paint, final_transform, ctm, clip_mask.as_ref());

            #[cfg(feature = "debug-logging")]
            eprintln!("DEBUG: Text path drawn to pixmap");
        } else {
            #[cfg(feature = "debug-logging")]
            eprintln!("DEBUG: Failed to create text path from path builder");
        }

        Ok(total_rendered_width)
    }

    pub(super) fn draw_text_advance(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        paint: &pdf_x_core::rendering::Paint,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<(f64, f64)> {
        let uses_vertical_raw_cid = self
            .font_cache
            .get(font_name)
            .map(|font| font.uses_raw_cid_codes() && font.cid_vertical())
            .unwrap_or(false);

        if !uses_vertical_raw_cid {
            let width = self.draw_text(
                text_bytes,
                font_name,
                font_size,
                paint,
                text_matrix,
                horizontal_scaling,
                text_rise,
                char_spacing,
                word_spacing,
            )?;
            return Ok((width, 0.0));
        }

        // TT outlines (skrifa/ttf_parser) live in OUTLINE_UPEM=1000 space.
        let (units_per_em, default_width) = {
            let Some(font) = self.font_cache.get(font_name) else {
                return Ok((0.0, 0.0));
            };
            let units_per_em = match &font.font_type {
                FontType::TrueType { .. } => OUTLINE_UPEM,
                FontType::Type1 { .. } => 1000.0,
            };
            (units_per_em, font.pdf_default_width)
        };

        let units_per_em_f64 = units_per_em as f64;
        let pdf_to_font = units_per_em / 1000.0;
        let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
            units_per_em_f64 / font_size
        } else {
            0.0
        };

        let mut text_path_builder = PathBuilder::new();
        let mut current_y = 0.0f32;
        let mut total_advance_user = 0.0f64;

        for cid in decode_be_cids(text_bytes) {
            // `/W`, `/DW`, `/W2`, `/DW2` are all in 1/1000 text-space units.
            let horizontal_width_pdf = self
                .font_cache
                .get(font_name)
                .map(|f| f.pdf_width_for_code(cid))
                .unwrap_or(default_width);
            let spacing_font_units = (char_spacing * spacing_to_font_units) as f32;
            let (vertical_advance_pdf, origin_pdf) = {
                let font = self.font_cache.get(font_name).unwrap();
                (
                    font.cid_vertical_advance(cid),
                    font.cid_vertical_origin(cid, horizontal_width_pdf),
                )
            };
            let vertical_advance_font = vertical_advance_pdf * pdf_to_font;
            let origin = (origin_pdf.0 * pdf_to_font, origin_pdf.1 * pdf_to_font);

            if let Some(glyph_path) = self
                .font_cache
                .get_mut(font_name)
                .unwrap()
                .cached_cid_glyph_path(cid)
            {
                let transform =
                    Transform::from_row(1.0, 0.0, 0.0, 1.0, origin.0, current_y + origin.1);
                if let Some(path_transformed) = glyph_path.transform(transform) {
                    text_path_builder.push_path(&path_transformed);
                }
            }

            current_y += vertical_advance_font + spacing_font_units;
            // Text-space vertical advance: w1/1000 * Tfs (+ Tc).
            total_advance_user += (vertical_advance_pdf as f64 * font_size / 1000.0) + char_spacing;
        }

        if let Some(path) = text_path_builder.finish() {
            let ctm = self.current_state().transform;
            let full_text = compose_text_transform(
                text_matrix,
                font_size,
                horizontal_scaling,
                text_rise,
                units_per_em,
            );
            let final_transform = full_text.post_concat(ctm);
            let clip_mask = self.get_clip_mask();

            self.paint_text_path(&path, paint, final_transform, ctm, clip_mask.as_ref());
        }

        Ok((0.0, total_advance_user))
    }

    pub(super) fn draw_text_advance_with_mode(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        fill_paint: &Paint,
        stroke_paint: &Paint,
        stroke_props: &StrokeProps,
        rendering_mode: TextRenderingMode,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<(f64, f64)> {
        let previous = self.text_draw_override.replace(TextDrawOverride {
            stroke_paint: stroke_paint.clone(),
            stroke_props: stroke_props.clone(),
            rendering_mode,
        });
        let result = self.draw_text_advance(
            text_bytes,
            font_name,
            font_size,
            fill_paint,
            text_matrix,
            horizontal_scaling,
            text_rise,
            char_spacing,
            word_spacing,
        );
        self.text_draw_override = previous;
        result
    }

    pub(super) fn draw_type0_glyph_run(
        &mut self,
        run: &Type0GlyphRun,
        font_name: &str,
        font_size: f64,
        paint: &Paint,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
    ) -> PDFResult<Option<(f64, f64)>> {
        if !self.font_cache.contains_key(font_name) {
            return Ok(Some((0.0, 0.0)));
        }
        // cached_cid/gid paths are always 1000-em for TT; Type1 is already 1000.
        let units_per_em = OUTLINE_UPEM;
        let pdf_to_font = 1.0f32;
        let char_spacing_font = if font_size.abs() > f64::EPSILON {
            (run.character_spacing * units_per_em as f64 / font_size) as f32
        } else {
            0.0
        };

        let mut text_path_builder = PathBuilder::new();
        let mut current_x = 0.0f32;
        let mut current_y = 0.0f32;
        let mut advance_x = 0.0f64;
        let mut advance_y = 0.0f64;

        for glyph in &run.glyphs {
            let glyph_path = if let Some(glyph_id) = glyph.glyph_id {
                self.font_cache
                    .get_mut(font_name)
                    .and_then(|font| font.cached_gid_glyph_path(glyph_id))
            } else {
                self.font_cache
                    .get_mut(font_name)
                    .and_then(|font| font.cached_cid_glyph_path(glyph.cid))
            };

            if let Some(path) = glyph_path {
                let origin_x = (glyph.origin.0 as f32) * pdf_to_font;
                let origin_y = (glyph.origin.1 as f32) * pdf_to_font;
                let transform = Transform::from_row(
                    1.0,
                    0.0,
                    0.0,
                    1.0,
                    current_x + origin_x,
                    current_y + origin_y,
                );
                if let Some(path_transformed) = path.transform(transform) {
                    text_path_builder.push_path(&path_transformed);
                }
            }

            if run.vertical {
                current_y += (glyph.advance.1 as f32) * pdf_to_font + char_spacing_font;
                advance_y += glyph.advance.1 * font_size / 1000.0 + run.character_spacing;
            } else {
                current_x += (glyph.advance.0 as f32) * pdf_to_font + char_spacing_font;
                advance_x += glyph.advance.0 * font_size / 1000.0 + run.character_spacing;
            }
        }

        if let Some(path) = text_path_builder.finish() {
            let ctm = self.current_state().transform;
            let full_text = compose_text_transform(
                text_matrix,
                font_size,
                horizontal_scaling,
                text_rise,
                units_per_em,
            );
            let final_transform = full_text.post_concat(ctm);
            let clip_mask = self.get_clip_mask();

            self.paint_text_path(&path, paint, final_transform, ctm, clip_mask.as_ref());
        }

        if run.vertical {
            Ok(Some((0.0, advance_y)))
        } else {
            Ok(Some((advance_x * horizontal_scaling / 100.0, 0.0)))
        }
    }

    pub(super) fn draw_type0_glyph_run_with_mode(
        &mut self,
        run: &Type0GlyphRun,
        font_name: &str,
        font_size: f64,
        fill_paint: &Paint,
        stroke_paint: &Paint,
        stroke_props: &StrokeProps,
        rendering_mode: TextRenderingMode,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
    ) -> PDFResult<Option<(f64, f64)>> {
        let previous = self.text_draw_override.replace(TextDrawOverride {
            stroke_paint: stroke_paint.clone(),
            stroke_props: stroke_props.clone(),
            rendering_mode,
        });
        let result = self.draw_type0_glyph_run(
            run,
            font_name,
            font_size,
            fill_paint,
            text_matrix,
            horizontal_scaling,
            text_rise,
        );
        self.text_draw_override = previous;
        result
    }

    pub(super) fn clip_text(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) {
        // Mirror draw_text path construction (without painting) so Pattern fill
        // via clip+shading matches filled-glyph geometry. Critical for Type0
        // Identity-H CID TrueType (pattern_shading_on_text.pdf).
        let Some(font) = self.font_cache.get(font_name) else {
            return;
        };

        let mut text_path = PathBuilder::new();
        let units_per_em = if font.is_type1() {
            // Build the glyph path — same logic as draw_text's Type1 branch but
            // without painting; just collect outlines into one PathBuilder.
            let (type1_font, uses_raw_cid_codes, default_width) = {
                let font = self.font_cache.get(font_name).unwrap();
                (
                    font.type1_font().unwrap().clone(),
                    font.uses_raw_cid_codes(),
                    font.pdf_default_width,
                )
            };

            let units_per_em = 1000.0f32;
            let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
                units_per_em as f64 / font_size
            } else {
                0.0
            };

            let mut current_x = 0.0f32;
            if uses_raw_cid_codes {
                for cid in decode_be_cids(text_bytes) {
                    let glyph_width = self
                        .font_cache
                        .get(font_name)
                        .map(|f| f.pdf_width_for_code(cid))
                        .unwrap_or(default_width);
                    let spacing_font = (char_spacing * spacing_to_font_units) as f32;

                    if let Some(glyph_path) = self
                        .font_cache
                        .get_mut(font_name)
                        .unwrap()
                        .cached_cid_glyph_path(cid)
                    {
                        let transform = Transform::from_row(1.0, 0.0, 0.0, 1.0, current_x, 0.0);
                        if let Some(t) = glyph_path.transform(transform) {
                            text_path.push_path(&t);
                        }
                    }

                    current_x += glyph_width + spacing_font;
                }
            } else {
                for &byte in text_bytes {
                    let ch = char::from(byte);
                    let glyph_width = self
                        .font_cache
                        .get(font_name)
                        .and_then(|f| {
                            f.pdf_widths
                                .as_ref()
                                .and_then(|w| w.get(byte as usize).copied())
                        })
                        .unwrap_or(type1_font.glyph_width(ch) as f32);
                    let mut spacing_user = char_spacing;
                    if byte == b' ' {
                        spacing_user += word_spacing;
                    }
                    let spacing_font = (spacing_user * spacing_to_font_units) as f32;

                    // Use cached glyph outline
                    if let Some(glyph_path) = self
                        .font_cache
                        .get_mut(font_name)
                        .unwrap()
                        .cached_glyph_path(byte)
                    {
                        let transform = Transform::from_row(1.0, 0.0, 0.0, 1.0, current_x, 0.0);
                        if let Some(t) = glyph_path.transform(transform) {
                            text_path.push_path(&t);
                        }
                    }

                    current_x += glyph_width + spacing_font;
                }
            }

            units_per_em
        } else if font.uses_raw_cid_codes() {
            // Type0 / CID TrueType (Identity-H): same as draw_text raw-CID branch.
            // Outlines are in OUTLINE_UPEM (1000) space.
            let default_width = self.font_cache.get(font_name).unwrap().pdf_default_width;
            let units_per_em = OUTLINE_UPEM;
            let pdf_to_font = 1.0f32;
            let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
                units_per_em as f64 / font_size
            } else {
                0.0
            };

            let mut current_x = 0.0f32;
            for cid in decode_be_cids(text_bytes) {
                let width_pdf = self
                    .font_cache
                    .get(font_name)
                    .map(|f| f.pdf_width_for_code(cid))
                    .unwrap_or(default_width);
                let width_font = width_pdf * pdf_to_font;
                let spacing_font_units = (char_spacing * spacing_to_font_units) as f32;

                if let Some(glyph_path) = self
                    .font_cache
                    .get_mut(font_name)
                    .unwrap()
                    .cached_cid_glyph_path(cid)
                {
                    let transform = Transform::from_row(1.0, 0.0, 0.0, 1.0, current_x, 0.0);
                    if let Some(t) = glyph_path.transform(transform) {
                        text_path.push_path(&t);
                    }
                }

                current_x += width_font + spacing_font_units;
            }

            units_per_em
        } else {
            let decoded_run = decode_text_run(text_bytes);
            let Some(shaped_buffer) = font.shape(decoded_run.text.as_str()) else {
                return;
            };
            let glyph_infos = shaped_buffer.glyph_infos();
            let glyph_positions = shaped_buffer.glyph_positions();
            let Some(face) = font.face() else {
                return;
            };
            let units_per_em = face.units_per_em() as f32;
            let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
                units_per_em as f64 / font_size
            } else {
                0.0
            };

            let mut current_x = 0.0f32;
            let mut current_y = 0.0f32;
            for (idx, (info, pos)) in glyph_infos.iter().zip(glyph_positions.iter()).enumerate() {
                let mut converter = PathConverter(PathBuilder::new());
                let transform = Transform::from_row(
                    1.0,
                    0.0,
                    0.0,
                    1.0,
                    current_x + pos.x_offset as f32,
                    current_y + pos.y_offset as f32,
                );

                let _ =
                    face.outline_glyph(ttf_parser::GlyphId(info.glyph_id as u16), &mut converter);

                if let Some(p) = converter.0.finish() {
                    if let Some(path_transformed) = p.transform(transform) {
                        text_path.push_path(&path_transformed);
                    }
                }

                let spacing_user = decoded_run
                    .spacing_codes
                    .get(idx)
                    .map(|&code| {
                        let mut s = char_spacing;
                        if !decoded_run.is_multibyte_codes && code == 0x20 {
                            s += word_spacing;
                        }
                        s
                    })
                    .unwrap_or(0.0);
                let spacing_font = (spacing_user * spacing_to_font_units) as f32;

                current_x += pos.x_advance as f32 + spacing_font;
                current_y += pos.y_advance as f32;
            }

            units_per_em
        };

        if let Some(path) = text_path.finish() {
            let full_text = compose_text_transform(
                text_matrix,
                font_size,
                horizontal_scaling,
                text_rise,
                units_per_em,
            );
            let ctm = self.current_state().transform;
            let final_transform = full_text.post_concat(ctm);

            let Some(path) = path.transform(final_transform) else {
                return;
            };
            self.pending_text_clip
                .get_or_insert_with(PathBuilder::new)
                .push_path(&path);
        }
    }

    pub(super) fn flush_text_clip(&mut self) -> PDFResult<()> {
        let Some(builder) = self.pending_text_clip.take() else {
            return Ok(());
        };

        if let Some(path) = builder.finish() {
            if let Some(mut mask) = Mask::new(self.pixmap.width(), self.pixmap.height()) {
                mask.fill_path(&path, SkiaFillRule::Winding, false, Transform::identity());

                self.current_state_mut().clip_mask = match self.current_state().clip_mask.as_ref() {
                    Some(existing) => intersect_masks(existing, &mask),
                    None => Some(mask),
                };
            }
        }

        Ok(())
    }
}
