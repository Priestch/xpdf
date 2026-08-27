//! Text operators (BT/ET, show variants, Td/TD/Tm/TL/Tc/Tw/Tz/Ts/Tr) and per-run glyph drawing.

use super::*;

impl<'a, D: Device> RenderingContext<'a, D> {
    pub(super) fn begin_text(&mut self) -> PDFResult<()> {
        self.in_text_object = true;
        // Reset text matrices
        self.current_state_mut().text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        self.current_state_mut().text_line_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        Ok(())
    }
    pub(super) fn end_text(&mut self) -> PDFResult<()> {
        // PDF 1.7 §9.3.6: glyph outlines accumulated in a clipping text mode
        // (Tr 4–7) are combined into a single path and intersected with the
        // current clipping path when the text object ends (ET).
        self.device.flush_text_clip()?;
        self.in_text_object = false;
        Ok(())
    }

    // === Text Operators ===
    pub(super) fn show_text(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
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
        let rendering_mode = state.text_rendering_mode;
        // Fill/stroke paints must both be available for Tr modes 1/2/5/6 (stroke /
        // fill-stroke ± clip). Backends with outline access paint via
        // draw_text_advance_with_mode; fill-only backends fall back to fill paint.
        let fill_paint = text_paint_for_rendering_mode(rendering_mode, self.effective_fill_color());
        let stroke_paint =
            text_stroke_paint_for_rendering_mode(rendering_mode, self.effective_stroke_color());
        let stroke_props = state.stroke_props.clone();
        let text_matrix = state.text_matrix;
        let horizontal_scaling = state.text_horizontal_scaling;
        let text_rise = state.text_rise;
        let character_spacing = state.character_spacing;
        let word_spacing = state.word_spacing;

        // Extract text bytes (using font's encoding, NOT UTF-8).
        // PDF text strings can be literal strings (...) or hex strings <...>.
        if let Some(bytes) = text_bytes(&args[0]) {
            if self.type3_fonts.get(&font_name).is_some() {
                let rendered_width = self.show_type3_text(
                    &font_name,
                    bytes,
                    font_size,
                    &text_matrix,
                    horizontal_scaling,
                    text_rise,
                    character_spacing,
                    word_spacing,
                )?;

                let state = self.current_state_mut();
                state.text_matrix = post_concat_translate(state.text_matrix, rendered_width, 0.0);
                return Ok(());
            }

            let mapped_bytes = self.map_text_bytes_for_device(&font_name, bytes);
            let (rendered_width, rendered_height) = self.draw_text_run(
                mapped_bytes.as_ref(),
                &font_name,
                font_size,
                &fill_paint,
                &stroke_paint,
                &stroke_props,
                rendering_mode,
                &text_matrix,
                horizontal_scaling,
                text_rise,
                character_spacing,
                word_spacing,
            )?;

            if is_text_trace_enabled() {
                eprintln!(
                    "[text-trace] Tj bytes={:?} advance=({}, {}) Tc={} Tw={} hscale={}",
                    bytes,
                    rendered_width,
                    rendered_height,
                    character_spacing,
                    word_spacing,
                    horizontal_scaling
                );
            }

            // Advance the current text matrix (Tm = Tm * T(tx, ty)).
            // Horizontal text uses ty=0; Identity-V vertical uses tx=0.
            // The text line matrix (Tlm) must stay unchanged for T*/' operators.
            let state = self.current_state_mut();
            state.text_matrix =
                post_concat_translate(state.text_matrix, rendered_width, rendered_height);
        }

        Ok(())
    }

    /// Draw a simple-font / Type0 glyph run, applying Pattern Type 2 fill
    /// shading when the current fill colour is a pattern (sticky `scn`).
    ///
    /// Source: PDF 1.7 §8.7.3 / §9.3.6; fixture `pattern_shading_on_text.pdf`
    /// fills text with `/Pattern cs /p0 scn` + `0 Tr`.
    /// Draw a simple-font / Type0 glyph run, applying Pattern Type 2 fill
    /// shading when the current fill colour is a pattern (sticky `scn`).
    ///
    /// Source: PDF 1.7 §8.7.3 / §9.3.6; fixture `pattern_shading_on_text.pdf`
    /// fills text with `/Pattern cs /p0 scn` + `0 Tr`.
    pub(super) fn draw_text_run(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        fill_paint: &Paint,
        stroke_paint: &Paint,
        stroke_props: &crate::rendering::graphics_state::StrokeProps,
        rendering_mode: crate::rendering::graphics_state::TextRenderingMode,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        character_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<(f64, f64)> {
        use crate::rendering::graphics_state::TextRenderingMode;

        let pattern_fill = mode_fills(rendering_mode) && self.pending_fill_shading.is_some();

        if pattern_fill {
            if let Some(shading) = self.pending_fill_shading.clone() {
                // Clip to glyph interiors, paint the shading through the clip,
                // then restore so subsequent drawing is not clipped.
                self.device.save_state();
                self.device.clip_text(
                    text_bytes,
                    font_name,
                    font_size,
                    text_matrix,
                    horizontal_scaling,
                    text_rise,
                    character_spacing,
                    word_spacing,
                );
                self.device.flush_text_clip()?;
                let base_ctm = if shading.use_root_ctm {
                    self.root_ctm()
                } else {
                    self.current_state().ctm
                };
                let combined = post_concat_matrix(base_ctm, shading.matrix);
                let fill_alpha = self.current_state().fill_alpha;
                self.device.set_opacity(fill_alpha);
                self.device.draw_shading(&shading, &combined)?;
                self.device.restore_state();
            }

            // Advance (and optional stroke) without a solid fill.
            let advance_mode = if mode_strokes(rendering_mode) {
                if mode_clips(rendering_mode) {
                    TextRenderingMode::StrokeClip
                } else {
                    TextRenderingMode::Stroke
                }
            } else if mode_clips(rendering_mode) {
                TextRenderingMode::Clip
            } else {
                TextRenderingMode::Invisible
            };

            let (w, h) = self.device.draw_text_advance_with_mode(
                text_bytes,
                font_name,
                font_size,
                &Paint::Transparent,
                stroke_paint,
                stroke_props,
                advance_mode,
                text_matrix,
                horizontal_scaling,
                text_rise,
                character_spacing,
                word_spacing,
            )?;

            // Clip modes still accumulate glyph outlines for ET flush.
            if mode_clips(rendering_mode) {
                self.device.clip_text(
                    text_bytes,
                    font_name,
                    font_size,
                    text_matrix,
                    horizontal_scaling,
                    text_rise,
                    character_spacing,
                    word_spacing,
                );
            }

            return Ok((w, h));
        }

        let (w, h) = self.device.draw_text_advance_with_mode(
            text_bytes,
            font_name,
            font_size,
            fill_paint,
            stroke_paint,
            stroke_props,
            rendering_mode,
            text_matrix,
            horizontal_scaling,
            text_rise,
            character_spacing,
            word_spacing,
        )?;

        if mode_clips(rendering_mode) {
            self.device.clip_text(
                text_bytes,
                font_name,
                font_size,
                text_matrix,
                horizontal_scaling,
                text_rise,
                character_spacing,
                word_spacing,
            );
        }

        Ok((w, h))
    }
    pub(super) fn show_spaced_text(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
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
        let rendering_mode = state.text_rendering_mode;
        let fill_paint = text_paint_for_rendering_mode(rendering_mode, self.effective_fill_color());
        let stroke_paint =
            text_stroke_paint_for_rendering_mode(rendering_mode, self.effective_stroke_color());
        let stroke_props = state.stroke_props.clone();
        let horizontal_scaling = state.text_horizontal_scaling;
        let text_rise = state.text_rise;
        let character_spacing = state.character_spacing;
        let word_spacing = state.word_spacing;

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: show_spaced_text: fill_color={:?}, stroke_color={:?}, mode={:?}",
            self.effective_fill_color(),
            self.effective_stroke_color(),
            rendering_mode
        );
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
                PDFObject::String(bytes) | PDFObject::HexString(bytes) => {
                    // Render this text string (using font encoding, NOT UTF-8)
                    if !bytes.is_empty() {
                        if self.type3_fonts.get(&font_name).is_some() {
                            let text_matrix = self.current_state().text_matrix;
                            let rendered_width = self.show_type3_text(
                                &font_name,
                                bytes,
                                font_size,
                                &text_matrix,
                                horizontal_scaling,
                                text_rise,
                                character_spacing,
                                word_spacing,
                            )?;

                            let state = self.current_state_mut();
                            state.text_matrix =
                                post_concat_translate(state.text_matrix, rendered_width, 0.0);
                            continue;
                        }

                        let mapped_bytes = self.map_text_bytes_for_device(&font_name, bytes);
                        let text_matrix = self.current_state().text_matrix;
                        let (rendered_width, rendered_height) = self.draw_text_run(
                            mapped_bytes.as_ref(),
                            &font_name,
                            font_size,
                            &fill_paint,
                            &stroke_paint,
                            &stroke_props,
                            rendering_mode,
                            &text_matrix,
                            horizontal_scaling,
                            text_rise,
                            character_spacing,
                            word_spacing,
                        )?;

                        if is_text_trace_enabled() {
                            eprintln!(
                                "[text-trace] TJ text bytes={:?} advance=({}, {}) Tc={} Tw={} hscale={}",
                                bytes,
                                rendered_width,
                                rendered_height,
                                character_spacing,
                                word_spacing,
                                horizontal_scaling
                            );
                        }

                        // Advance text matrix by the actual rendered advance.
                        let state = self.current_state_mut();
                        state.text_matrix = post_concat_translate(
                            state.text_matrix,
                            rendered_width,
                            rendered_height,
                        );
                    }
                }
                PDFObject::Number(offset) => {
                    // TJ numbers are spacing adjustments in thousandths of an em.
                    // Positive numbers tighten spacing, so the translation is negative.
                    // Vertical writing mode (Identity-V): advance along ty and do not
                    // apply Tz — matches PDF 32000-1:2008 §9.4.4 and hayro
                    // `TextState::apply_adjustment`.
                    // Source: hayro/hayro-interpret/src/interpret/state.rs
                    let vertical = self.device.font_is_vertical(&font_name);
                    let scale = if vertical {
                        1.0
                    } else {
                        horizontal_scaling / 100.0
                    };
                    let adjust = -(offset * font_size * scale) / 1000.0;
                    if is_text_trace_enabled() {
                        eprintln!(
                            "[text-trace] TJ adjust raw={} translated={} vertical={}",
                            offset, adjust, vertical
                        );
                    }
                    let state = self.current_state_mut();
                    if vertical {
                        state.text_matrix = post_concat_translate(state.text_matrix, 0.0, adjust);
                    } else {
                        state.text_matrix = post_concat_translate(state.text_matrix, adjust, 0.0);
                    }
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
    pub(super) fn next_line_show_text(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        self.next_line()?;
        self.show_text(args)
    }
    pub(super) fn next_line_set_spacing_show_text(
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
    pub(super) fn show_type3_text(
        &mut self,
        font_name: &str,
        bytes: &[u8],
        font_size: f64,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        character_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<f64> {
        let Some(font) = self.type3_fonts.get(font_name).cloned() else {
            return Ok(0.0);
        };

        let mut cursor = 0.0;
        for &code in bytes {
            let spacing = character_spacing + if code == b' ' { word_spacing } else { 0.0 };
            let glyph_width = font.width_for_code(code);
            let gname = font.encoding.glyph_name(code);
            let has = gname
                .as_ref()
                .map(|n| font.char_procs.contains_key(n))
                .unwrap_or(false);

            if let Some(glyph_name) = font.encoding.glyph_name(code) {
                if let Some(glyph) = font.char_procs.get(&glyph_name) {
                    self.render_type3_glyph(
                        &font,
                        glyph,
                        text_matrix,
                        font_size,
                        horizontal_scaling,
                        text_rise,
                        cursor,
                    )?;
                }
            }

            cursor += (glyph_width * font_size + spacing) * (horizontal_scaling / 100.0);
        }

        Ok(cursor)
    }
    pub(super) fn render_type3_glyph(
        &mut self,
        font: &Type3Font,
        glyph: &Type3Glyph,
        text_matrix: &[f64; 6],
        font_size: f64,
        horizontal_scaling: f64,
        text_rise: f64,
        cursor: f64,
    ) -> PDFResult<()> {
        let base_ctm = self.current_state().ctm;
        let glyph_text_matrix = post_concat_translate(*text_matrix, cursor, 0.0);
        let text_transform = compose_type3_text_transform(
            glyph_text_matrix,
            font_size,
            horizontal_scaling,
            text_rise,
        );
        let glyph_ctm = post_concat_matrix(
            post_concat_matrix(base_ctm, text_transform),
            font.font_matrix,
        );
        let previous_resources = self.resources.clone();
        let glyph_resources = {
            let mut resolver_opt = self.resolver.take();
            let merged = if resolver_opt.is_some() {
                let font_resources = merge_pattern_resources(
                    font.resources.clone(),
                    previous_resources.clone(),
                    &mut resolver_opt,
                );
                merge_pattern_resources(glyph.resources.clone(), font_resources, &mut resolver_opt)
            } else {
                glyph
                    .resources
                    .clone()
                    .or_else(|| font.resources.clone())
                    .or_else(|| previous_resources.clone())
            };
            self.resolver = resolver_opt;
            merged
        };

        self.save()?;
        if let Some(resources) = glyph_resources {
            self.resources = Some(resources);
        }
        self.current_state_mut().set_matrix(&glyph_ctm);
        self.device.set_matrix(&glyph_ctm);

        let previous_type3_shape_color = self.type3_shape_color;
        if glyph.is_shape_glyph {
            self.type3_shape_color = Some(self.current_state().fill_color);
        }
        self.device.begin_type3_glyph();
        let result = self.process_content_stream_bytes(glyph.data.clone());
        self.device.end_type3_glyph();
        self.type3_shape_color = previous_type3_shape_color;

        self.resources = previous_resources;
        let restore_result = self.restore();
        result?;
        restore_result
    }
    pub(super) fn move_text(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "Td operator requires 2 arguments".to_string(),
            ));
        }

        let tx = extract_number(args, 0)?;
        let ty = extract_number(args, 1)?;

        let state = self.current_state_mut();
        state.text_line_matrix = post_concat_translate(state.text_line_matrix, tx, ty);
        state.text_matrix = state.text_line_matrix;

        Ok(())
    }
    pub(super) fn set_leading_move_text(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "TD operator requires 2 arguments".to_string(),
            ));
        }

        let ty = extract_number(args, 1)?;
        self.current_state_mut().text_leading = -ty; // Leading is negative of ty

        self.move_text(args)
    }
    pub(super) fn set_text_matrix(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
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
    pub(super) fn next_line(&mut self) -> PDFResult<()> {
        let state = self.current_state_mut();
        state.text_line_matrix =
            post_concat_translate(state.text_line_matrix, 0.0, -state.text_leading);
        state.text_matrix = state.text_line_matrix;
        Ok(())
    }
    pub(super) fn set_font(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "Tf operator requires 2 arguments".to_string(),
            ));
        }

        if let crate::core::parser::PDFObject::Name(name) = &args[0] {
            self.current_state_mut().font_name = Some(name.clone());

            // Skip font loading entirely if the device already has it (from a
            // previous render or earlier in this render). This is the single most
            // impactful render optimization: Type1/CFF parsing is ~2ms per font
            // and a page may switch fonts dozens of times.
            if self.device.has_font(name.as_str()) {
                self.current_state_mut().font_size = Some(extract_number(args, 1)?);
                return Ok(());
            }

            // Font not yet in device cache — load from resources.
            let (resolver_opt, resources_opt) = (self.resolver.take(), self.resources.clone());
            if let (Some(mut resolver), Some(resources)) = (resolver_opt, resources_opt) {
                let result =
                    self.load_font_from_resources(name.as_str(), &resources, &mut *resolver);
                self.resolver = Some(resolver);
                result?;
            }
        }

        self.current_state_mut().font_size = Some(extract_number(args, 1)?);
        Ok(())
    }
    pub(super) fn map_text_bytes_for_device<'b>(
        &self,
        font_name: &str,
        bytes: &'b [u8],
    ) -> Cow<'b, [u8]> {
        if !self
            .font_requires_cmap_remap
            .get(font_name)
            .copied()
            .unwrap_or(false)
        {
            return Cow::Borrowed(bytes);
        }

        let Some(cmap) = self.font_to_unicode.get(font_name) else {
            return Cow::Borrowed(bytes);
        };

        if let Some(mapped) = remap_text_bytes_with_cmap(bytes, cmap) {
            Cow::Owned(mapped)
        } else {
            Cow::Borrowed(bytes)
        }
    }
    pub(super) fn load_to_unicode_cmap(
        &mut self,
        font_name: &str,
        font_dict_info: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) {
        self.font_to_unicode.remove(font_name);

        let Some(to_unicode_obj) = font_dict_info.to_unicode.as_ref() else {
            return;
        };

        let fetched = match resolver.resolve_if_ref(to_unicode_obj) {
            Ok(obj) => obj,
            Err(_) => return,
        };

        let (dict, data) = match &*fetched {
            PDFObject::Stream { dict, data } => (dict.clone(), data.clone()),
            _ => return,
        };

        let decoded = if let Some(filters) = dict.get("Filter") {
            crate::core::decode::apply_filters(&data, filters).unwrap_or(data)
        } else {
            data
        };

        if let Ok(cmap) = CMap::parse(&decoded) {
            self.font_to_unicode.insert(font_name.to_string(), cmap);
        }
    }

    /// Load a font from the page's resources dictionary.
    ///
    /// This looks up the font in the Resources/Font dictionary, extracts the font data,
    /// and loads it into the rendering device.
    pub(super) fn set_char_spacing(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tc operator requires 1 argument".to_string(),
            ));
        }

        let value = extract_number(args, 0)?;
        self.current_state_mut().character_spacing = value;
        if is_text_trace_enabled() {
            eprintln!("[text-trace] Tc set to {}", value);
        }
        Ok(())
    }
    pub(super) fn set_word_spacing(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tw operator requires 1 argument".to_string(),
            ));
        }

        let value = extract_number(args, 0)?;
        self.current_state_mut().word_spacing = value;
        if is_text_trace_enabled() {
            eprintln!("[text-trace] Tw set to {}", value);
        }
        Ok(())
    }
    pub(super) fn set_horizontal_scaling(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Tz operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().text_horizontal_scaling = extract_number(args, 0)?;
        Ok(())
    }
    pub(super) fn set_leading(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "TL operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().text_leading = extract_number(args, 0)?;
        Ok(())
    }
    pub(super) fn set_text_rendering_mode(
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
            0 => crate::rendering::graphics_state::TextRenderingMode::Fill,
            1 => crate::rendering::graphics_state::TextRenderingMode::Stroke,
            2 => crate::rendering::graphics_state::TextRenderingMode::FillStroke,
            3 => crate::rendering::graphics_state::TextRenderingMode::Invisible,
            4 => crate::rendering::graphics_state::TextRenderingMode::FillClip,
            5 => crate::rendering::graphics_state::TextRenderingMode::StrokeClip,
            6 => crate::rendering::graphics_state::TextRenderingMode::FillStrokeClip,
            7 => crate::rendering::graphics_state::TextRenderingMode::Clip,
            _ => {
                return Err(PDFError::content_stream_error(format!(
                    "Invalid text rendering mode: {}",
                    mode
                )));
            }
        };
        Ok(())
    }
    pub(super) fn set_text_rise(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Ts operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().text_rise = extract_number(args, 0)?;
        Ok(())
    }

    // === Color Operators ===
}
