//! Font loading from resources: widths, descendant CID dictionaries, standard-font fallbacks.

use super::*;

/// Map an arbitrary BaseFont name to a Base-14 style key for bundled substitutes.
///
/// Mirrors hayro's `FallbackFontQuery::pick_standard_font` (serif/sans/mono ×
/// bold × italic). Critical for unembedded Type1s like Garamond-Bold → Times-Bold
/// rather than always falling back to regular serif.
pub(super) fn pick_fallback_standard_font_key(base_font: &str) -> &'static str {
    let lower = base_font.to_ascii_lowercase();
    let bold = lower.contains("bold")
        || lower.contains("black")
        || lower.contains("heavy")
        || lower.contains("medi")
        || lower.contains("demibold")
        || lower.contains("semibold");
    let italic = lower.contains("italic")
        || lower.contains("oblique")
        || lower.contains("slant")
        || lower.contains("kursiv");
    let mono = lower.contains("mono")
        || lower.contains("courier")
        || lower.contains("typewriter")
        || lower.contains("console");
    let sans = lower.contains("sans")
        || lower.contains("helvetica")
        || lower.contains("arial")
        || lower.contains("gothic")
        || lower.contains("ui");

    if mono {
        match (bold, italic) {
            (true, true) => "Courier-BoldOblique",
            (true, false) => "Courier-Bold",
            (false, true) => "Courier-Oblique",
            (false, false) => "Courier",
        }
    } else if sans {
        match (bold, italic) {
            (true, true) => "Helvetica-BoldOblique",
            (true, false) => "Helvetica-Bold",
            (false, true) => "Helvetica-Oblique",
            (false, false) => "Helvetica",
        }
    } else {
        // Default serif family (Times / Garamond / Book / Roman / …).
        match (bold, italic) {
            (true, true) => "Times-BoldItalic",
            (true, false) => "Times-Bold",
            (false, true) => "Times-Italic",
            (false, false) => "Times-Roman",
        }
    }
}

pub(super) fn bundled_standard_font_data(base_font: &str) -> Option<Vec<u8>> {
    // Unknown names go through style-aware Base-14 mapping first so
    // "Garamond-Bold" resolves to Times-Bold, not Times-Roman.
    let key = match base_font {
        "Helvetica"
        | "ArialMT"
        | "Arial"
        | "LiberationSans"
        | "NimbusSanL-Regu"
        | "Helvetica-Bold"
        | "Arial-BoldMT"
        | "Arial-Bold"
        | "Arial,Bold"
        | "LiberationSans-Bold"
        | "NimbusSanL-Bold"
        | "Helvetica-Oblique"
        | "Arial-ItalicMT"
        | "Arial-Italic"
        | "Helvetica-Italic"
        | "LiberationSans-Italic"
        | "NimbusSanL-ReguItal"
        | "Helvetica-BoldOblique"
        | "Arial-BoldItalicMT"
        | "Helvetica-BoldItalic"
        | "LiberationSans-BoldItalic"
        | "NimbusSanL-BoldItal"
        | "Times-Roman"
        | "Times"
        | "Times New Roman"
        | "TimesNewRomanPSMT"
        | "TimesNewRoman"
        | "TimesNewRomanPS"
        | "LiberationSerif"
        | "NimbusRomNo9L-Regu"
        | "Times-Bold"
        | "TimesNewRomanPS-BoldMT"
        | "TimesNewRomanPS-Bold"
        | "TimesNewRoman-Bold"
        | "LiberationSerif-Bold"
        | "NimbusRomNo9L-Medi"
        | "Times-Italic"
        | "TimesNewRomanPS-ItalicMT"
        | "TimesNewRomanPS-Italic"
        | "TimesNewRoman-Italic"
        | "LiberationSerif-Italic"
        | "NimbusRomNo9L-ReguItal"
        | "Times-BoldItalic"
        | "TimesNewRomanPS-BoldItalicMT"
        | "TimesNewRomanPS-BoldItalic"
        | "TimesNewRoman-BoldItalic"
        | "LiberationSerif-BoldItalic"
        | "NimbusRomNo9L-MediItal"
        | "Courier"
        | "CourierNew"
        | "CourierNewPSMT"
        | "LiberationMono"
        | "NimbusMonL-Regu"
        | "Courier-Bold"
        | "CourierNewPS-BoldMT"
        | "CourierNew-Bold"
        | "LiberationMono-Bold"
        | "NimbusMonL-Bold"
        | "Courier-Oblique"
        | "CourierNewPS-ItalicMT"
        | "CourierNew-Italic"
        | "LiberationMono-Italic"
        | "NimbusMonL-ReguObli"
        | "Courier-BoldOblique"
        | "CourierNewPS-BoldItalicMT"
        | "CourierNew-BoldItalic"
        | "LiberationMono-BoldItalic"
        | "NimbusMonL-BoldObli" => base_font,
        _ => pick_fallback_standard_font_key(base_font),
    };

    // Prefer Foxit Base14 CFF (same programs as pdf.js / hayro) so unembedded
    // standard fonts share outlines and 1000-unit glyph space with those engines.
    // Liberation TTF remains on disk for optional system-font experiments only.
    // Source: hayro/hayro-interpret/src/font/standard_font.rs (`StandardFont::get_font_data`)
    let font = match key {
        "Helvetica" | "ArialMT" | "Arial" | "LiberationSans" | "NimbusSanL-Regu" => {
            include_bytes!("../../../assets/fonts/FoxitSans.pfb").as_slice()
        }
        "Helvetica-Bold"
        | "Arial-BoldMT"
        | "Arial-Bold"
        | "Arial,Bold"
        | "LiberationSans-Bold"
        | "NimbusSanL-Bold" => include_bytes!("../../../assets/fonts/FoxitSansBold.pfb").as_slice(),
        "Helvetica-Oblique"
        | "Arial-ItalicMT"
        | "Arial-Italic"
        | "Helvetica-Italic"
        | "LiberationSans-Italic"
        | "NimbusSanL-ReguItal" => {
            include_bytes!("../../../assets/fonts/FoxitSansItalic.pfb").as_slice()
        }
        "Helvetica-BoldOblique"
        | "Arial-BoldItalicMT"
        | "Helvetica-BoldItalic"
        | "LiberationSans-BoldItalic"
        | "NimbusSanL-BoldItal" => {
            include_bytes!("../../../assets/fonts/FoxitSansBoldItalic.pfb").as_slice()
        }
        "Times-Roman" | "Times" | "Times New Roman" | "TimesNewRomanPSMT" | "TimesNewRoman"
        | "TimesNewRomanPS" | "LiberationSerif" | "NimbusRomNo9L-Regu" => {
            include_bytes!("../../../assets/fonts/FoxitSerif.pfb").as_slice()
        }
        "Times-Bold"
        | "TimesNewRomanPS-BoldMT"
        | "TimesNewRomanPS-Bold"
        | "TimesNewRoman-Bold"
        | "LiberationSerif-Bold"
        | "NimbusRomNo9L-Medi" => {
            include_bytes!("../../../assets/fonts/FoxitSerifBold.pfb").as_slice()
        }
        "Times-Italic"
        | "TimesNewRomanPS-ItalicMT"
        | "TimesNewRomanPS-Italic"
        | "TimesNewRoman-Italic"
        | "LiberationSerif-Italic"
        | "NimbusRomNo9L-ReguItal" => {
            include_bytes!("../../../assets/fonts/FoxitSerifItalic.pfb").as_slice()
        }
        "Times-BoldItalic"
        | "TimesNewRomanPS-BoldItalicMT"
        | "TimesNewRomanPS-BoldItalic"
        | "TimesNewRoman-BoldItalic"
        | "LiberationSerif-BoldItalic"
        | "NimbusRomNo9L-MediItal" => {
            include_bytes!("../../../assets/fonts/FoxitSerifBoldItalic.pfb").as_slice()
        }
        "Courier" | "CourierNew" | "CourierNewPSMT" | "LiberationMono" | "NimbusMonL-Regu" => {
            include_bytes!("../../../assets/fonts/FoxitFixed.pfb").as_slice()
        }
        "Courier-Bold"
        | "CourierNewPS-BoldMT"
        | "CourierNew-Bold"
        | "LiberationMono-Bold"
        | "NimbusMonL-Bold" => {
            include_bytes!("../../../assets/fonts/FoxitFixedBold.pfb").as_slice()
        }
        "Courier-Oblique"
        | "CourierNewPS-ItalicMT"
        | "CourierNew-Italic"
        | "LiberationMono-Italic"
        | "NimbusMonL-ReguObli" => {
            include_bytes!("../../../assets/fonts/FoxitFixedItalic.pfb").as_slice()
        }
        "Courier-BoldOblique"
        | "CourierNewPS-BoldItalicMT"
        | "CourierNew-BoldItalic"
        | "LiberationMono-BoldItalic"
        | "NimbusMonL-BoldObli" => {
            include_bytes!("../../../assets/fonts/FoxitFixedBoldItalic.pfb").as_slice()
        }
        "Symbol" => include_bytes!("../../../assets/fonts/FoxitSymbol.pfb").as_slice(),
        "ZapfDingbats" => include_bytes!("../../../assets/fonts/FoxitDingbats.pfb").as_slice(),
        // pick_fallback always returns a Base-14 family key above.
        _ => include_bytes!("../../../assets/fonts/FoxitSerif.pfb").as_slice(),
    };

    Some(font.to_vec())
}

impl<'a, D: Device> RenderingContext<'a, D> {
    /// Load a font from the page's resources dictionary.
    ///
    /// This looks up the font in the Resources/Font dictionary, extracts the font data,
    /// and loads it into the rendering device.
    pub(super) fn load_font_from_resources(
        &mut self,
        font_name: &str,
        resources: &PDFObject,
        resolver: &mut dyn PageResolver,
    ) -> PDFResult<()> {
        // Get the Font dictionary from resources
        let resources_dict = match resources {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(()), // No resources dictionary
        };

        let font_dict_value = match resources_dict.get("Font") {
            Some(f) => f,
            None => return Ok(()), // No Font dictionary
        };

        // Fetch the Font dictionary (might be a reference)
        let _fetched_font_dict;
        let font_dict = match font_dict_value {
            PDFObject::Dictionary(d) => d,
            PDFObject::Ref(ref_obj) => {
                _fetched_font_dict = resolver.resolve_ref(ref_obj.num, ref_obj.generation)?;
                match &*_fetched_font_dict {
                    PDFObject::Dictionary(d) => d,
                    _ => return Ok(()),
                }
            }
            _ => return Ok(()),
        };

        // Get the specific font object
        let font_obj_ref = match font_dict.get(font_name) {
            Some(f) => f,
            None => {
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!("DEBUG: Font '{}' not found in Font dictionary", font_name);
                return Ok(());
            }
        };

        // Fetch the font object (might be a reference)
        let font_obj = resolver.resolve_if_ref(font_obj_ref)?;

        // Parse the font dictionary
        let mut font_dict_info = match crate::core::font::FontDict::from_pdf_object(&*font_obj) {
            Ok(f) => f,
            Err(e) => {
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Failed to parse font dictionary for '{}': {:?}",
                    font_name, e
                );
                return Ok(()); // Don't fail rendering just because font parsing failed
            }
        };

        // `/Widths` is frequently an indirect object (e.g. pdfTeX
        // `font_type1_1.pdf`: `/Widths 7 0 R`). `FontDict::from_pdf_object`
        // only accepts an immediate array, so resolve the ref here before
        // `set_font_widths`. Without this, Type1 advances fall back to a flat
        // 500 and letterspaced titles compress vs mutool/hayro.
        if font_dict_info.widths.is_none() {
            if let PDFObject::Dictionary(dict) = font_obj.as_ref() {
                if let Some(widths_obj) = dict.get("Widths") {
                    if let Ok(resolved) = resolver.resolve_if_ref(widths_obj) {
                        if let PDFObject::Array(arr) = resolved.as_ref() {
                            let mut widths = Vec::with_capacity(arr.len());
                            for item in arr {
                                if let PDFObject::Number(w) = item.as_ref() {
                                    widths.push(*w);
                                }
                            }
                            if !widths.is_empty() {
                                font_dict_info.widths = Some(widths);
                            }
                        }
                    }
                }
            }
        }

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Font '{}' - BaseFont: {}, Type: {:?}",
            font_name, font_dict_info.base_font, font_dict_info.font_type
        );

        let requires_cmap_remap = font_needs_to_unicode_remap(&font_dict_info);
        self.font_requires_cmap_remap
            .insert(font_name.to_string(), requires_cmap_remap);
        if requires_cmap_remap {
            // Cache /ToUnicode mapping for composite/CID-like fonts where raw
            // shown bytes are not directly usable by the drawing backend.
            self.load_to_unicode_cmap(font_name, &font_dict_info, resolver);
        } else {
            self.font_to_unicode.remove(font_name);
        }

        if matches!(font_dict_info.font_type, crate::core::font::FontType::Type3) {
            if let Some(type3) = build_type3_font(&font_obj, &font_dict_info, resolver) {
                self.type3_fonts.insert(font_name.to_string(), type3);
                return Ok(());
            }
            self.type3_fonts.remove(font_name);
        } else {
            self.type3_fonts.remove(font_name);
        }

        // Try to get font data
        let font_data = self.extract_font_data(&font_dict_info, resolver);

        if let Some(data) = font_data {
            #[cfg(feature = "debug-logging")]
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Loading font '{}' with {} bytes of data",
                font_name,
                data.len()
            );
            // Try to load the font data with encoding, but fall back to bundled standard fonts if it fails
            // (e.g., CFF fonts can't be parsed by ttf_parser)
            // Dereference encoding if it's a reference
            let encoding_obj = if let Some(enc) = &font_dict_info.encoding {
                match resolver.resolve_if_ref(enc) {
                    Ok(fetched) => Some(fetched),
                    Err(_) => Some(Arc::new(enc.clone())),
                }
            } else {
                None
            };
            let encoding_ref = encoding_obj.as_ref();
            match self
                .device
                .load_font_data(font_name, data, encoding_ref.map(|v| &**v))
            {
                Ok(_) => {
                    if let (Some(first_char), Some(widths)) =
                        (font_dict_info.first_char, font_dict_info.widths.as_ref())
                    {
                        let _ = self.device.set_font_widths(
                            font_name,
                            first_char,
                            widths,
                            font_dict_info.default_width,
                        );
                    }

                    if font_uses_identity_cid_to_gid(&font_obj, &font_dict_info, resolver) {
                        let _ = self.device.set_font_raw_cid_codes(font_name, true);
                    }

                    if self.device.font_wants_raw_cid_codes(font_name) {
                        self.font_requires_cmap_remap
                            .insert(font_name.to_string(), false);

                        if font_dict_info.widths.is_none() {
                            if let Some((first_cid, widths, default_width)) =
                                self.extract_descendant_cid_widths(&font_dict_info, resolver)
                            {
                                let _ = self.device.set_font_widths(
                                    font_name,
                                    first_cid,
                                    &widths,
                                    default_width,
                                );
                            } else if let Some(dw) =
                                self.extract_descendant_cid_dw(&font_dict_info, resolver)
                            {
                                // Identity CIDFont with only `/DW` (no `/W`).
                                let _ = self.device.set_font_widths(font_name, 0, &[], dw);
                            }
                        }

                        // Identity-V: vertical writing mode uses `/DW2` + `/W2`.
                        // PDF 32000-1:2008 §9.7.4.3.
                        if font_encoding_is_identity_v(&font_dict_info) {
                            let (dw2, widths2) = self
                                .extract_descendant_cid_vertical_metrics(&font_dict_info, resolver)
                                .unwrap_or(((880.0, -1000.0), Vec::new()));
                            let _ = self
                                .device
                                .set_font_cid_vertical_metrics(font_name, true, dw2, &widths2);
                        }
                    }

                    #[cfg(feature = "debug-logging")]
                    #[cfg(feature = "debug-logging")]
                    eprintln!(
                        "DEBUG: Successfully loaded font '{}' into device",
                        font_name
                    );
                    return Ok(());
                }
                Err(_e) => {
                    // Fall through to try bundled standard fonts.
                }
            }
        } else {
            #[cfg(feature = "debug-logging")]
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: No font data available for '{}', attempting standard font",
                font_name
            );
        }

        // Try to load as a bundled standard font.
        // Pass font_name as the cache key (e.g., "F0") and base_font for Base14 mapping.
        // Encoding is often an indirect ref (e.g. Distiller `/Encoding 40 0 R` with
        // Differences); resolve before load so WinAnsi+Differences apply to substitutes.
        let encoding_resolved = if let Some(enc) = &font_dict_info.encoding {
            match resolver.resolve_if_ref(enc) {
                Ok(fetched) => Some(fetched),
                Err(_) => Some(std::sync::Arc::new(enc.clone())),
            }
        } else {
            None
        };
        self.load_standard_font(
            font_name,
            &font_dict_info.base_font,
            encoding_resolved.as_ref().map(|a| a.as_ref()),
        )?;

        // Type0 Identity-H may still need raw-CID mode when we only reached
        // the standard-font fallback (embedded CFF load failed earlier).
        if font_uses_identity_cid_to_gid(&font_obj, &font_dict_info, resolver) {
            let _ = self.device.set_font_raw_cid_codes(font_name, true);
            if let Some((first_cid, widths, default_width)) =
                self.extract_descendant_cid_widths(&font_dict_info, resolver)
            {
                let _ = self
                    .device
                    .set_font_widths(font_name, first_cid, &widths, default_width);
            }
            self.font_requires_cmap_remap
                .insert(font_name.to_string(), false);
        }

        if let (Some(first_char), Some(widths)) =
            (font_dict_info.first_char, font_dict_info.widths.as_ref())
        {
            let _ = self.device.set_font_widths(
                font_name,
                first_char,
                widths,
                font_dict_info.default_width,
            );
        }

        Ok(())
    }

    /// Resolve Type0 `/DescendantFonts` to the first CIDFont dictionary.
    ///
    /// The array is often an indirect ref (`/DescendantFonts 41 0 R`).
    /// Resolve Type0 `/DescendantFonts` to the first CIDFont dictionary.
    ///
    /// The array is often an indirect ref (`/DescendantFonts 41 0 R`).
    pub(super) fn resolve_first_descendant_cid_dict(
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<std::sync::Arc<PDFObject>> {
        let desc_fonts = font_dict.descendant_fonts.as_ref()?;
        let resolved = resolver.resolve_if_ref(desc_fonts).ok()?;
        let PDFObject::Array(descendants) = &*resolved else {
            return None;
        };
        let cid_ref = descendants.first()?;
        let cid_obj = resolver.resolve_if_ref(cid_ref).ok()?;
        match cid_obj.as_ref() {
            PDFObject::Dictionary(_) | PDFObject::Stream { .. } => Some(cid_obj),
            _ => None,
        }
    }
    pub(super) fn extract_descendant_cid_widths(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<(u32, Vec<f64>, f64)> {
        let cid_obj = Self::resolve_first_descendant_cid_dict(font_dict, resolver)?;
        let cid_dict = match cid_obj.as_ref() {
            PDFObject::Dictionary(dict) => dict,
            PDFObject::Stream { dict, .. } => dict,
            _ => return None,
        };

        let default_width = cid_dict
            .get("DW")
            .and_then(PDFObject::as_number)
            .unwrap_or(1000.0);
        // `/W` is often an indirect reference (e.g. xdvipdfmx); resolve before parse.
        // PDF 32000-1:2008 §9.7.4.3.
        let w_obj = cid_dict.get("W")?;
        let w_resolved = resolver.resolve_if_ref(w_obj).ok()?;
        let width_entries = match &*w_resolved {
            PDFObject::Array(arr) => arr,
            _ => return None,
        };

        let mut table = Vec::new();
        let mut saw_width = false;
        let mut index = 0;

        while index < width_entries.len() {
            let Some(first_cid) = width_entries[index].as_number().map(|n| n as u32) else {
                break;
            };
            index += 1;

            let Some(entry) = width_entries.get(index) else {
                break;
            };

            match &**entry {
                PDFObject::Array(widths) => {
                    for (offset, width_obj) in widths.iter().enumerate() {
                        let cid = first_cid.saturating_add(offset as u32);
                        if cid as usize >= table.len() {
                            table.resize(cid as usize + 1, default_width);
                        }
                        if let Some(width) = width_obj.as_number() {
                            table[cid as usize] = width;
                            saw_width = true;
                        }
                    }
                    index += 1;
                }
                PDFObject::Number(last_cid) => {
                    let last_cid = *last_cid as u32;
                    let Some(width) = width_entries.get(index + 1).and_then(|v| v.as_number())
                    else {
                        break;
                    };
                    if last_cid as usize >= table.len() {
                        table.resize(last_cid as usize + 1, default_width);
                    }
                    for cid in first_cid..=last_cid {
                        table[cid as usize] = width;
                        saw_width = true;
                    }
                    index += 2;
                }
                _ => break,
            }
        }

        saw_width.then_some((0, table, default_width))
    }

    /// `/DW` alone from the descendant CIDFont (when `/W` is absent).
    /// `/DW` alone from the descendant CIDFont (when `/W` is absent).
    pub(super) fn extract_descendant_cid_dw(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<f64> {
        let cid_obj = Self::resolve_first_descendant_cid_dict(font_dict, resolver)?;
        let cid_dict = match cid_obj.as_ref() {
            PDFObject::Dictionary(dict) => dict,
            PDFObject::Stream { dict, .. } => dict,
            _ => return None,
        };
        cid_dict.get("DW").and_then(PDFObject::as_number)
    }

    /// Vertical metrics from descendant CIDFont `/DW2` and `/W2`.
    ///
    /// Returns `((v1, w1), entries)` where each entry is `(cid, w1, v1, v2)`.
    /// PDF 32000-1:2008 §9.7.4.3.
    /// Vertical metrics from descendant CIDFont `/DW2` and `/W2`.
    ///
    /// Returns `((v1, w1), entries)` where each entry is `(cid, w1, v1, v2)`.
    /// PDF 32000-1:2008 §9.7.4.3.
    pub(super) fn extract_descendant_cid_vertical_metrics(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<((f64, f64), Vec<(u32, f64, f64, f64)>)> {
        let cid_obj = Self::resolve_first_descendant_cid_dict(font_dict, resolver)?;
        let cid_dict = match cid_obj.as_ref() {
            PDFObject::Dictionary(dict) => dict,
            PDFObject::Stream { dict, .. } => dict,
            _ => return None,
        };

        // DW2 = [v1 w1]; default [880 -1000].
        let dw2 = match cid_dict.get("DW2") {
            Some(PDFObject::Array(arr)) if arr.len() >= 2 => {
                let v1 = arr[0].as_number().unwrap_or(880.0);
                let w1 = arr[1].as_number().unwrap_or(-1000.0);
                (v1, w1)
            }
            _ => (880.0, -1000.0),
        };

        let mut widths2 = Vec::new();
        if let Some(w2_obj) = cid_dict.get("W2") {
            if let Ok(w2_resolved) = resolver.resolve_if_ref(w2_obj) {
                if let PDFObject::Array(entries) = &*w2_resolved {
                    let mut index = 0;
                    while index < entries.len() {
                        let Some(first_cid) = entries[index].as_number().map(|n| n as u32) else {
                            break;
                        };
                        index += 1;
                        let Some(entry) = entries.get(index) else {
                            break;
                        };
                        match &**entry {
                            // c [w1 v1 v2 w1 v1 v2 ...]
                            PDFObject::Array(metrics) => {
                                let mut m = 0;
                                let mut cid = first_cid;
                                while m + 2 < metrics.len() {
                                    let w1 = metrics[m].as_number().unwrap_or(dw2.1);
                                    let v1 = metrics[m + 1].as_number().unwrap_or(dw2.0);
                                    let v2 = metrics[m + 2].as_number().unwrap_or(0.0);
                                    widths2.push((cid, w1, v1, v2));
                                    cid = cid.saturating_add(1);
                                    m += 3;
                                }
                                index += 1;
                            }
                            // cfirst clast w1 v1 v2
                            PDFObject::Number(last_cid) => {
                                let last_cid = *last_cid as u32;
                                let w1 = entries
                                    .get(index + 1)
                                    .and_then(|v| v.as_number())
                                    .unwrap_or(dw2.1);
                                let v1 = entries
                                    .get(index + 2)
                                    .and_then(|v| v.as_number())
                                    .unwrap_or(dw2.0);
                                let v2 = entries
                                    .get(index + 3)
                                    .and_then(|v| v.as_number())
                                    .unwrap_or(0.0);
                                for cid in first_cid..=last_cid {
                                    widths2.push((cid, w1, v1, v2));
                                }
                                index += 4;
                            }
                            _ => break,
                        }
                    }
                }
            }
        }

        Some((dw2, widths2))
    }

    /// Extract font data from a FontDict.
    ///
    /// Returns the raw font data (TrueType, CFF, etc.) if available.
    /// Extract font data from a FontDict.
    ///
    /// Returns the raw font data (TrueType, CFF, etc.) if available.
    pub(super) fn extract_font_data(
        &self,
        font_dict: &crate::core::font::FontDict,
        resolver: &mut dyn PageResolver,
    ) -> Option<Vec<u8>> {
        // For Type0 fonts, FontDescriptor is in the DescendantFonts → CIDFont dict.
        // `/DescendantFonts` is frequently an indirect array ref (e.g. `41 0 R`
        // → `[42 0 R]`); resolve before matching Array.
        let descriptor_ref = if let Some(desc) = &font_dict.font_descriptor {
            desc.clone()
        } else if let Some(desc_fonts) = &font_dict.descendant_fonts {
            let resolved_arr = match resolver.resolve_if_ref(desc_fonts) {
                Ok(obj) => obj,
                Err(_) => return None,
            };
            let PDFObject::Array(descendants) = &*resolved_arr else {
                return None;
            };
            // Type0: traverse DescendantFonts → CIDFont → FontDescriptor
            let cid_ref = descendants.first()?;
            let cid_obj = resolver.resolve_if_ref(cid_ref).ok()?;
            match &*cid_obj {
                PDFObject::Dictionary(cid_dict) => cid_dict.get("FontDescriptor")?.clone(),
                PDFObject::Stream { dict, .. } => dict.get("FontDescriptor")?.clone(),
                _ => return None,
            }
        } else {
            return None;
        };

        // Fetch the descriptor (might be a reference)
        let _fetched_descriptor;
        let descriptor = match &descriptor_ref {
            PDFObject::Dictionary(d) => d,
            PDFObject::Ref(r) => {
                _fetched_descriptor = resolver.resolve_ref(r.num, r.generation).ok()?;
                match &*_fetched_descriptor {
                    PDFObject::Dictionary(d) => d,
                    _ => return None,
                }
            }
            _ => return None,
        };

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: FontDescriptor keys: {:?}",
            descriptor.keys().collect::<Vec<_>>()
        );

        // Try to get font file from FontDescriptor
        // The key depends on font type:
        // - TrueType: /FontFile2
        // - Type1: /FontFile
        // - Type1C (CFF): /FontFile3
        //
        // NOTE: Some PDFs mislabel Type1C as Type1, so we check multiple keys
        let font_file_keys = match font_dict.font_type {
            crate::core::font::FontType::TrueType => vec!["FontFile2"],
            crate::core::font::FontType::Type1 => {
                // Type1 might actually be Type1C (CFF), so check both
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!("DEBUG: Type1 font, checking FontFile and FontFile3");
                vec!["FontFile", "FontFile3"]
            }
            crate::core::font::FontType::Type1C => vec!["FontFile3"],
            crate::core::font::FontType::CIDFontType0 => vec!["FontFile3"],
            crate::core::font::FontType::CIDFontType2 => vec!["FontFile2"],
            _ => vec!["FontFile2", "FontFile3", "FontFile"],
        };

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Looking for font file keys: {:?}", font_file_keys);

        for key in font_file_keys {
            if let Some(font_file_ref) = descriptor.get(key) {
                let font_file_obj = resolver.resolve_if_ref(font_file_ref).ok()?;
                if let PDFObject::Stream { dict, data } = &*font_file_obj {
                    let decoded = if let Some(filter) = dict.get("Filter") {
                        crate::core::decode::apply_filters(data, filter)
                            .unwrap_or_else(|_| data.clone())
                    } else {
                        data.clone()
                    };
                    return Some(decoded);
                }
            }
        }

        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: No font data found in extract_font_data");
        None
    }

    /// Load a standard PDF font by name (e.g., "Helvetica", "Times-Roman").
    ///
    /// This uses deterministic bundled fonts as a fallback for standard PDF fonts.
    /// It also handles subset prefixes (e.g., "AKXEFC+DogmaBold" -> "DogmaBold").
    ///
    /// # Arguments
    /// * `cache_key` - The PDF font resource name to use as cache key (e.g., "F0")
    /// * `base_font` - The BaseFont name from the PDF for bundled standard-font mapping
    /// * `encoding` - Optional PDF Encoding (MacRoman/WinAnsi/…) for substitute faces
    /// Load a standard PDF font by name (e.g., "Helvetica", "Times-Roman").
    ///
    /// This uses deterministic bundled fonts as a fallback for standard PDF fonts.
    /// It also handles subset prefixes (e.g., "AKXEFC+DogmaBold" -> "DogmaBold").
    ///
    /// # Arguments
    /// * `cache_key` - The PDF font resource name to use as cache key (e.g., "F0")
    /// * `base_font` - The BaseFont name from the PDF for bundled standard-font mapping
    /// * `encoding` - Optional PDF Encoding (MacRoman/WinAnsi/…) for substitute faces
    pub(super) fn load_standard_font(
        &mut self,
        cache_key: &str,
        base_font: &str,
        encoding: Option<&PDFObject>,
    ) -> PDFResult<()> {
        eprintln!("[stretch] load_standard_font cache_key={} base_font={}", cache_key, base_font);
        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Attempting to load bundled standard font for cache_key='{}', base_font='{}'",
            cache_key, base_font
        );

        // Strip subset prefix if present (e.g., "AKXEFC+DogmaBold" -> "DogmaBold")
        let clean_font = if let Some(idx) = base_font.find('+') {
            &base_font[idx + 1..]
        } else {
            base_font
        };

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Clean font name: '{}'", clean_font);

        let resolved = self
            .device
            .resolve_standard_font_data(clean_font)
            .or_else(|| {
                if clean_font != base_font {
                    self.device.resolve_standard_font_data(base_font)
                } else {
                    None
                }
            });

        if let Some(data) = resolved {
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Loaded adapter standard font for cache_key='{}', base_font='{}'",
                cache_key, base_font
            );
            let result = self.device.load_font_data(cache_key, data, encoding);
            if result.is_ok() {
                let _ = self.device.mark_font_substituted(cache_key);
            }
            return result;
        }

        if let Some(data) = bundled_standard_font_data(clean_font) {
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Loaded bundled standard font for cache_key='{}', base_font='{}'",
                cache_key, base_font
            );
            let result = self.device.load_font_data(cache_key, data, encoding);
            if result.is_ok() {
                let _ = self.device.mark_font_substituted(cache_key);
            }
            return result;
        }

        #[cfg(feature = "debug-logging")]
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Could not find bundled standard font for cache_key='{}', base_font='{}'",
            cache_key, base_font
        );
        Ok(())
    }
}
