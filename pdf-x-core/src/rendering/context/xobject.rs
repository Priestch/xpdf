//! XObject painting: forms, images, inline images, and annotation appearance streams.

use super::*;

impl<'a, D: Device> RenderingContext<'a, D> {
    pub(super) fn paint_xobject(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        // Do operator - paint an XObject (image, form, etc.)
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "Do operator requires 1 argument (XObject name)".to_string(),
            ));
        }

        // Extract XObject name
        let xobject_name = match &args[0] {
            PDFObject::Name(name) => name,
            _ => {
                return Err(PDFError::content_stream_error(
                    "Do operator argument must be a name".to_string(),
                ));
            }
        };

        let resources = match self.resources.as_ref() {
            Some(r) => r,
            None => return Ok(()),
        };

        // Get resolver and resources
        let resolver = match &mut self.resolver {
            Some(x) => x,
            None => return Ok(()),
        };

        // Get the XObject dictionary from resources
        let resources_dict = match resources {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(()), // No resources dictionary
        };

        // Fetch the XObject dictionary (may be a reference)
        let xobject_dict_value = match resources_dict.get("XObject") {
            Some(xo) => xo,
            None => return Ok(()),
        };

        // Store fetched XObject dict to keep it alive during the match
        let _fetched_xobject_dict;

        let xobject_dict = match xobject_dict_value {
            PDFObject::Dictionary(d) => d,
            PDFObject::Ref(ref_obj) => {
                _fetched_xobject_dict = resolver.resolve_ref(ref_obj.num, ref_obj.generation)?;
                match &*_fetched_xobject_dict {
                    PDFObject::Dictionary(d) => d,
                    _ => return Ok(()),
                }
            }
            _ => return Ok(()),
        };

        // Get the specific XObject
        let xobject_ref = match xobject_dict.get(xobject_name) {
            Some(xobj) => xobj,
            None => return Ok(()),
        };

        let xobject = resolver.resolve_if_ref(xobject_ref)?;

        // XObjects are always stream objects.
        let (xobject_dict, xobject_data) = match &*xobject {
            PDFObject::Stream { dict, data } => (dict, data),
            _ => return Ok(()),
        };

        let subtype = match xobject_dict.get("Subtype") {
            Some(PDFObject::Name(name)) => name,
            _ => return Ok(()),
        };

        if subtype == "Form" {
            return self.paint_form_xobject(xobject_name, xobject_dict, xobject_data);
        }

        if subtype != "Image" {
            return Ok(());
        }

        // Check for image mask (1-bpp stencil: 1 = paint with current fill color).
        // PDFium also treats image dictionaries without ColorSpace as image masks.
        let is_image_mask = xobject_dict
            .get("ImageMask")
            .and_then(|v| match v {
                PDFObject::Boolean(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(false)
            || (!xobject_dict.contains_key("ColorSpace")
                && !filter_contains_any(xobject_dict.get("Filter"), &["JPXDecode"]));

        // Extract image properties
        let width = match xobject_dict.get("Width") {
            Some(PDFObject::Number(w)) => *w as u32,
            _ => return Ok(()),
        };

        let height = match xobject_dict.get("Height") {
            Some(PDFObject::Number(h)) => *h as u32,
            _ => return Ok(()),
        };

        let bits_per_component = if is_image_mask {
            1 // Image masks are always 1 bpc
        } else {
            match xobject_dict.get("BitsPerComponent") {
                Some(PDFObject::Number(b)) => *b as u8,
                _ => 8,
            }
        };

        let color_info = {
            let mut resolver = self.resolver.take();
            let info = image_color_space_info(xobject_dict, &mut resolver);
            self.resolver = resolver;
            info
        };
        let color_space_name = color_info.name.or(Some("DeviceRGB"));

        // Determine if image has alpha
        let has_alpha = match color_space_name {
            Some("DeviceRGB") | Some("DeviceGray") | Some("CalRGB") | Some("CalGray")
            | Some("DeviceCMYK") => false,
            _ => true,
        };

        // Get the image data stream
        let image_data = xobject_data;

        // Decode the image data if needed
        // For JPEG, we also need to update the image metadata
        let (decoded_data, decoded_width, decoded_height, decoded_bpc, decoded_has_alpha) =
            if let Some(filter) = xobject_dict.get("Filter") {
                match filter {
                    PDFObject::Name(filter_name) => {
                        use crate::core::decode;
                        match filter_name.as_str() {
                            "FlateDecode" | "Fl" => {
                                let decoded = decode::decode_flate(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                let (decoded, predictor_bpc) =
                                    apply_predictor_from_parms(decoded, xobject_dict, width);
                                (
                                    decoded,
                                    width,
                                    height,
                                    predictor_bpc.unwrap_or(bits_per_component),
                                    has_alpha,
                                )
                            }
                            // PDF §7.4.4 LZWDecode — used by older PageMaker-era
                            // XObject images (e.g. pdftc_900k_0974 Im1 DeviceGray).
                            // Without this arm, compressed bytes are treated as
                            // raw pixels and the image never paints (dual lag
                            // hay−mut − ours−mut ≈ +10% on that fixture).
                            "LZWDecode" | "LZW" => {
                                let decoded = decode::decode_lzw(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                let (decoded, predictor_bpc) =
                                    apply_predictor_from_parms(decoded, xobject_dict, width);
                                (
                                    decoded,
                                    width,
                                    height,
                                    predictor_bpc.unwrap_or(bits_per_component),
                                    has_alpha,
                                )
                            }
                            "ASCIIHexDecode" | "AHx" => {
                                let decoded = decode::decode_ascii_hex(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                            "ASCII85Decode" | "A85" => {
                                let decoded = decode::decode_ascii85(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                            "RunLengthDecode" | "RL" => {
                                let decoded = decode::decode_run_length(image_data)
                                    .unwrap_or_else(|_| image_data.clone());
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                            "DCTDecode" | "DCT" => {
                                // JPEG data - decode it using zune-jpeg
                                #[cfg(feature = "jpeg-decoding")]
                                {
                                    match crate::core::image::ImageDecoder::decode_pdf_jpeg(
                                        image_data,
                                        jpeg_decode_options(
                                            xobject_dict,
                                            color_info.components,
                                            color_info.name,
                                        ),
                                    ) {
                                        Ok(decoded) => {
                                            // Use decoded image's metadata since JPEG decoder knows best
                                            (
                                                decoded.data,
                                                decoded.metadata.width,
                                                decoded.metadata.height,
                                                decoded.metadata.bits_per_component,
                                                decoded.metadata.has_alpha,
                                            )
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "Warning: Failed to decode JPEG image: {}",
                                                e
                                            );
                                            (
                                                image_data.clone(),
                                                width,
                                                height,
                                                bits_per_component,
                                                has_alpha,
                                            )
                                        }
                                    }
                                }
                                #[cfg(not(feature = "jpeg-decoding"))]
                                {
                                    eprintln!("Warning: JPEG decoding not enabled, skipping image");
                                    // Return empty data to prevent crash
                                    (Vec::new(), width, height, bits_per_component, has_alpha)
                                }
                            }
                            "CCITTFaxDecode" | "CCF" => {
                                #[cfg(feature = "hayro-ccitt")]
                                {
                                    match decode_ccitt(image_data, xobject_dict, width, height) {
                                        Some(data) => (data, width, height, 1, false),
                                        None => (
                                            image_data.clone(),
                                            width,
                                            height,
                                            bits_per_component,
                                            has_alpha,
                                        ),
                                    }
                                }
                                #[cfg(not(feature = "hayro-ccitt"))]
                                {
                                    (
                                        image_data.clone(),
                                        width,
                                        height,
                                        bits_per_component,
                                        has_alpha,
                                    )
                                }
                            }
                            "JPXDecode" => {
                                #[cfg(feature = "advanced-image-formats")]
                                {
                                    match decode_jpx(
                                        image_data,
                                        jpx_decode_options(xobject_dict, color_info.components),
                                    ) {
                                        Some((d, w, h, bpc, a)) => (d, w, h, bpc, a),
                                        None => (
                                            image_data.clone(),
                                            width,
                                            height,
                                            bits_per_component,
                                            has_alpha,
                                        ),
                                    }
                                }
                                #[cfg(not(feature = "advanced-image-formats"))]
                                {
                                    (
                                        image_data.clone(),
                                        width,
                                        height,
                                        bits_per_component,
                                        has_alpha,
                                    )
                                }
                            }
                            "JBIG2Decode" => {
                                #[cfg(feature = "advanced-image-formats")]
                                {
                                    let mut resolver = self.resolver.take();
                                    let globals = jbig2_globals_data(
                                        xobject_dict,
                                        xobject_dict.get("Filter"),
                                        &mut resolver,
                                    );
                                    self.resolver = resolver;
                                    match decode_jbig2(image_data, globals.as_deref()) {
                                        Some(data) => (data, width, height, 1, false),
                                        None => (
                                            image_data.clone(),
                                            width,
                                            height,
                                            bits_per_component,
                                            has_alpha,
                                        ),
                                    }
                                }
                                #[cfg(not(feature = "advanced-image-formats"))]
                                {
                                    (
                                        image_data.clone(),
                                        width,
                                        height,
                                        bits_per_component,
                                        has_alpha,
                                    )
                                }
                            }
                            _ => {
                                // Unsupported filter - try raw data
                                (
                                    image_data.clone(),
                                    width,
                                    height,
                                    bits_per_component,
                                    has_alpha,
                                )
                            }
                        }
                    }
                    PDFObject::Array(_) => {
                        // Apply non-image filters first (FlateDecode, ASCIIHex, etc.)
                        let filter = xobject_dict.get("Filter").unwrap();
                        let decoded = crate::core::decode::apply_filters(image_data, filter)
                            .unwrap_or_else(|_| image_data.clone());

                        // Check if an image-specific filter is in the chain —
                        // apply_filters skips these, so decode the image separately
                        // after the preceding non-image filters have run.
                        let filter_names: Vec<&str> = match filter {
                            PDFObject::Array(arr) => arr
                                .iter()
                                .filter_map(|f| match &**f {
                                    PDFObject::Name(n) => Some(n.as_str()),
                                    _ => None,
                                })
                                .collect(),
                            _ => Vec::new(),
                        };

                        if filter_names.contains(&"DCTDecode") || filter_names.contains(&"DCT") {
                            #[cfg(feature = "jpeg-decoding")]
                            {
                                match crate::core::image::ImageDecoder::decode_pdf_jpeg(
                                    &decoded,
                                    jpeg_decode_options(
                                        xobject_dict,
                                        color_info.components,
                                        color_info.name,
                                    ),
                                ) {
                                    Ok(img) => (
                                        img.data,
                                        img.metadata.width,
                                        img.metadata.height,
                                        8,
                                        img.metadata.has_alpha,
                                    ),
                                    Err(_) => {
                                        (decoded, width, height, bits_per_component, has_alpha)
                                    }
                                }
                            }
                            #[cfg(not(feature = "jpeg-decoding"))]
                            {
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                        } else if filter_names.contains(&"CCITTFaxDecode")
                            || filter_names.contains(&"CCF")
                        {
                            #[cfg(feature = "hayro-ccitt")]
                            {
                                match decode_ccitt(&decoded, xobject_dict, width, height) {
                                    Some(data) => (data, width, height, 1, false),
                                    None => (decoded, width, height, bits_per_component, has_alpha),
                                }
                            }
                            #[cfg(not(feature = "hayro-ccitt"))]
                            {
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                        } else if filter_names.contains(&"JPXDecode") {
                            #[cfg(feature = "advanced-image-formats")]
                            {
                                match decode_jpx(
                                    &decoded,
                                    jpx_decode_options(xobject_dict, color_info.components),
                                ) {
                                    Some((d, w, h, bpc, a)) => (d, w, h, bpc, a),
                                    None => (decoded, width, height, bits_per_component, has_alpha),
                                }
                            }
                            #[cfg(not(feature = "advanced-image-formats"))]
                            {
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                        } else if filter_names.contains(&"JBIG2Decode") {
                            // Filter array e.g. [/ASCIIHexDecode /JBIG2Decode]:
                            // apply_filters already ran non-image filters; decode JBIG2
                            // with optional /JBIG2Globals (ISO 32000-1 §7.4.7).
                            #[cfg(feature = "advanced-image-formats")]
                            {
                                let mut resolver = self.resolver.take();
                                let globals = jbig2_globals_data(
                                    xobject_dict,
                                    xobject_dict.get("Filter"),
                                    &mut resolver,
                                );
                                self.resolver = resolver;
                                match decode_jbig2(&decoded, globals.as_deref()) {
                                    Some(data) => (data, width, height, 1, false),
                                    None => (decoded, width, height, bits_per_component, has_alpha),
                                }
                            }
                            #[cfg(not(feature = "advanced-image-formats"))]
                            {
                                (decoded, width, height, bits_per_component, has_alpha)
                            }
                        } else {
                            // Apply PNG predictor from DecodeParms if present.
                            let (decoded, predictor_bpc) =
                                apply_predictor_from_parms(decoded, xobject_dict, width);
                            (
                                decoded,
                                width,
                                height,
                                predictor_bpc.unwrap_or(bits_per_component),
                                has_alpha,
                            )
                        }
                    }
                    _ => (
                        image_data.clone(),
                        width,
                        height,
                        bits_per_component,
                        has_alpha,
                    ),
                }
            } else {
                (
                    image_data.clone(),
                    width,
                    height,
                    bits_per_component,
                    has_alpha,
                )
            };

        // Remember the original bpc before expansion so the device can
        // skip bilinear filtering on bilevel images.
        let original_bpc = decoded_bpc;

        // Expand 1-bpc sampled images to 8-bpc before color conversion. PDF image rows
        // are byte-aligned, so the padding bits at the end of each row must be skipped.
        let expand_1bpc_sampled = should_expand_1bpc_sampled_image(xobject_dict.get("Filter"));
        let expand_1bpc_as_indexed = if decoded_bpc == 1 && !is_image_mask && expand_1bpc_sampled {
            let cs_obj = xobject_dict.get("ColorSpace").cloned();
            let mut resolver = self.resolver.take();
            let is_indexed = is_indexed_color_space(cs_obj.as_ref(), &mut resolver);
            self.resolver = resolver;
            is_indexed
        } else {
            false
        };
        let (decoded_data, decoded_bpc) =
            if decoded_bpc == 1 && !is_image_mask && expand_1bpc_sampled {
                (
                    expand_1bpc_image_to_8bpc(
                        &decoded_data,
                        decoded_width,
                        decoded_height,
                        if expand_1bpc_as_indexed { 1 } else { 255 },
                    ),
                    8,
                )
            } else if decoded_bpc > 8 {
                (downscale_16_to_8(&decoded_data), 8)
            } else {
                (decoded_data, decoded_bpc)
            };

        // Handle Indexed color space: [/Indexed baseCS hival lookup_table]
        let (decoded_data, color_expanded) = {
            let cs_obj = xobject_dict.get("ColorSpace").cloned();
            let mut resolver = self.resolver.take();
            let result = expand_indexed_cs(
                &decoded_data,
                cs_obj.as_ref(),
                &mut resolver,
                decoded_bpc,
                decoded_width,
                decoded_height,
            );
            self.resolver = resolver;
            result
        };
        let decoded_bpc = if color_expanded { 8 } else { decoded_bpc };

        // If indexed CS was expanded, update the effective color space.
        let color_space_name = if color_expanded {
            Some("DeviceRGB")
        } else {
            color_space_name
        };

        // Handle Lab color space: convert L*a*b* → sRGB.
        let (decoded_data, color_space_name) =
            if let Some(PDFObject::Array(cs_arr)) = xobject_dict.get("ColorSpace") {
                if cs_arr.len() >= 2 {
                    if let PDFObject::Name(n) = &*cs_arr[0] {
                        if n == "Lab" {
                            // Parse Lab parameters
                            let (xw, yw, zw, amin, amax, bmin, bmax) = parse_lab_params(&cs_arr);
                            let rgb = lab_to_rgb(&decoded_data, xw, yw, zw, amin, amax, bmin, bmax);
                            (rgb, Some("DeviceRGB"))
                        } else {
                            (decoded_data, color_space_name)
                        }
                    } else {
                        (decoded_data, color_space_name)
                    }
                } else {
                    (decoded_data, color_space_name)
                }
            } else {
                (decoded_data, color_space_name)
            };

        let decode_arr = match xobject_dict.get("Decode") {
            Some(PDFObject::Array(decode_arr)) => Some(decode_arr),
            _ => None,
        };

        // /Decode maps raw image samples before color-space conversion. CMYK JPEGs
        // commonly use [1 0 1 0 1 0 1 0], so apply it while the data is still CMYK.
        let was_cmyk = color_space_name == Some("DeviceCMYK");
        let decoded_data = if was_cmyk {
            if let Some(decode_arr) = decode_arr {
                apply_decode_inversion(&decoded_data, decode_arr, decoded_bpc)
            } else {
                decoded_data
            }
        } else {
            decoded_data
        };

        // Handle DeviceCMYK images: convert CMYK → RGB via ICC profile.
        // (only when the `rendering` feature is enabled — which provides moxcms)
        #[cfg(feature = "rendering")]
        let (decoded_data, color_space_name) = if color_space_name == Some("DeviceCMYK") {
            let rgb = crate::rendering::cmyk_icc::cmyk_image_to_rgb(&decoded_data);
            (rgb, Some("DeviceRGB"))
        } else {
            (decoded_data, color_space_name)
        };

        // Apply /Decode for non-CMYK, non-mask images that were not handled
        // before conversion. Image masks resolve /Decode only via
        // `mask_paint_one` (which bit paints); also inverting samples here
        // double-applies Decode (image_mask.pdf Decode [1 0] painted ~6% of
        // the page instead of ~55%). Inline masks already skip this path.
        let decoded_data = if !was_cmyk && !is_image_mask {
            if let Some(decode_arr) = decode_arr {
                apply_decode_inversion(&decoded_data, decode_arr, decoded_bpc)
            } else {
                decoded_data
            }
        } else {
            decoded_data
        };

        let has_image_smask = xobject_dict
            .get("SMask")
            .is_some_and(|smask| !matches!(smask, PDFObject::Name(name) if name == "None"));
        let (decoded_data, decoded_has_alpha) =
            if !is_image_mask && has_image_smask && decoded_bpc == 8 {
                let mut resolver = self.resolver.take();
                let result = image_smask_alpha(
                    &decoded_data,
                    color_space_name,
                    decoded_width,
                    decoded_height,
                    xobject_dict.get("SMask"),
                    &mut resolver,
                );
                self.resolver = resolver;
                match result {
                    Some(rgba) => (rgba, true),
                    None => (decoded_data, decoded_has_alpha),
                }
            } else {
                (decoded_data, decoded_has_alpha)
            };

        // Create image data for the device
        let interpolate = matches!(
            xobject_dict.get("Interpolate"),
            Some(PDFObject::Boolean(true))
        );
        let image = crate::rendering::ImageData {
            width: decoded_width,
            height: decoded_height,
            data: decoded_data,
            has_alpha: decoded_has_alpha,
            bits_per_component: decoded_bpc,
            original_bits_per_component: original_bpc,
            is_mask: is_image_mask,
            mask_paint_one: image_mask_paint_one(xobject_dict),
            interpolate,
            mask_color: {
                let c = self.effective_fill_color();
                match c {
                    crate::rendering::Color::Gray(v) => {
                        let g = (v * 255.0).clamp(0.0, 255.0) as u8;
                        [g, g, g]
                    }
                    crate::rendering::Color::RGB(r, g, b) => [
                        (r * 255.0).clamp(0.0, 255.0) as u8,
                        (g * 255.0).clamp(0.0, 255.0) as u8,
                        (b * 255.0).clamp(0.0, 255.0) as u8,
                    ],
                    crate::rendering::Color::CMYK(_, _, _, _) => [0, 0, 0],
                }
            },
        };

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: PaintXObject: Drawing image '{}' ({}x{}, {} bpc, {} bytes)",
            xobject_name,
            width,
            height,
            bits_per_component,
            image_data.len()
        );

        // Use identity transform - the CTM already maps unit square to screen correctly
        // The content stream operators set up the CTM to position and scale the image
        let transform = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

        if has_image_smask {
            self.device.save_state();
            let _ = self.device.set_soft_mask(None);
            let result = self.device.draw_image(image, &transform);
            self.device.restore_state();
            result?;
        } else {
            self.device.draw_image(image, &transform)?;
        }

        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Image drawn successfully");

        Ok(())
    }
    pub(super) fn paint_inline_image(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        let Some(PDFObject::Stream { dict, data }) = args.first() else {
            return Ok(());
        };

        let is_image_mask = dict
            .get("ImageMask")
            .and_then(PDFObject::as_boolean)
            .unwrap_or(false)
            || (!dict.contains_key("ColorSpace")
                && !filter_contains_any(dict.get("Filter"), &["JPXDecode"]));

        let width = match dict.get("Width").and_then(PDFObject::as_number) {
            Some(w) => w as u32,
            None => return Ok(()),
        };
        let height = match dict.get("Height").and_then(PDFObject::as_number) {
            Some(h) => h as u32,
            None => return Ok(()),
        };
        let bits_per_component = if is_image_mask {
            1
        } else {
            dict.get("BitsPerComponent")
                .and_then(PDFObject::as_number)
                .unwrap_or(8.0) as u8
        };

        let original_bpc = bits_per_component;

        let decoded =
            decode_inline_image_data(data, dict, width, height).unwrap_or_else(|| data.clone());

        let (decoded, bpc) = if bits_per_component == 1
            && !is_image_mask
            && should_expand_1bpc_sampled_image(dict.get("Filter"))
        {
            (expand_1bpc_image_to_8bpc(&decoded, width, height, 255), 8)
        } else {
            (decoded, bits_per_component)
        };

        let (decoded, bpc) = if !is_image_mask {
            let (expanded, color_expanded) = expand_indexed_cs(
                &decoded,
                dict.get("ColorSpace"),
                &mut None,
                bpc,
                width,
                height,
            );
            if color_expanded {
                (expanded, 8)
            } else {
                (decoded, bpc)
            }
        } else {
            (decoded, bpc)
        };

        let decoded = if !is_image_mask {
            if let Some(PDFObject::Array(decode_arr)) = dict.get("Decode") {
                apply_decode_inversion(&decoded, decode_arr, bpc)
            } else {
                decoded
            }
        } else {
            decoded
        };

        let image = crate::rendering::ImageData {
            width,
            height,
            data: decoded,
            has_alpha: false,
            bits_per_component: bpc,
            original_bits_per_component: original_bpc,
            is_mask: is_image_mask,
            mask_paint_one: image_mask_paint_one(dict),
            interpolate: matches!(dict.get("Interpolate"), Some(PDFObject::Boolean(true))),
            mask_color: {
                let c = self.effective_fill_color();
                match c {
                    crate::rendering::Color::Gray(v) => {
                        let g = (v * 255.0).clamp(0.0, 255.0) as u8;
                        [g, g, g]
                    }
                    crate::rendering::Color::RGB(r, g, b) => [
                        (r * 255.0).clamp(0.0, 255.0) as u8,
                        (g * 255.0).clamp(0.0, 255.0) as u8,
                        (b * 255.0).clamp(0.0, 255.0) as u8,
                    ],
                    crate::rendering::Color::CMYK(_, _, _, _) => [0, 0, 0],
                }
            },
        };

        let transform = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Inline image {}x{} is_mask={} mask_paint_one={} mask_color={:?} fill={:?} type3_shape_color={:?}",
            image.width,
            image.height,
            image.is_mask,
            image.mask_paint_one,
            image.mask_color,
            self.current_state().fill_color,
            self.type3_shape_color
        );
        self.device.draw_image(image, &transform)
    }

    /// Paint Normal appearance streams for page annotations (PDF §12.5.5).
    ///
    /// Skips annotations with the Hidden flag. Maps each Form XObject `/BBox`
    /// through `/Matrix` into the annotation `/Rect`, matching hayro's
    /// `interpret_page` annotation pass.
    /// Source: hayro/hayro-interpret/src/interpret/mod.rs (render_annotations).
    /// Paint Normal appearance streams for page annotations (PDF §12.5.5).
    ///
    /// Skips annotations with the Hidden flag. Maps each Form XObject `/BBox`
    /// through `/Matrix` into the annotation `/Rect`, matching hayro's
    /// `interpret_page` annotation pass.
    /// Source: hayro/hayro-interpret/src/interpret/mod.rs (render_annotations).
    pub fn paint_annotation_appearances(&mut self, annotations: &[PDFObject]) -> PDFResult<()> {
        for annot in annotations {
            let PDFObject::Dictionary(annot_dict) = annot else {
                continue;
            };

            // F bit 1 (value 2) = Hidden — do not paint.
            let flags = annot_dict
                .get("F")
                .and_then(|o| match o {
                    PDFObject::Number(n) => Some(*n as u32),
                    _ => None,
                })
                .unwrap_or(0);
            if flags & 2 != 0 {
                continue;
            }

            let Some(rect) = parse_four_number_array(annot_dict.get("Rect")) else {
                continue;
            };
            let [llx, lly, urx, ury] = rect;
            let annot_w = (urx - llx).abs();
            let annot_h = (ury - lly).abs();
            if annot_w < 1e-9 || annot_h < 1e-9 {
                continue;
            }

            let Some((form_dict, form_data)) = self.resolve_annotation_appearance(annot_dict)
            else {
                continue;
            };

            let matrix = parse_six_number_array(form_dict.get("Matrix"))
                .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
            let bbox =
                parse_four_number_array(form_dict.get("BBox")).unwrap_or([0.0, 0.0, 1.0, 1.0]);

            // Transform BBox corners by Matrix; take axis-aligned bounds.
            let corners = [
                transform_point(matrix, bbox[0], bbox[1]),
                transform_point(matrix, bbox[2], bbox[1]),
                transform_point(matrix, bbox[0], bbox[3]),
                transform_point(matrix, bbox[2], bbox[3]),
            ];
            let mut x_min = corners[0].0;
            let mut x_max = corners[0].0;
            let mut y_min = corners[0].1;
            let mut y_max = corners[0].1;
            for &(x, y) in &corners[1..] {
                x_min = x_min.min(x);
                x_max = x_max.max(x);
                y_min = y_min.min(y);
                y_max = y_max.max(y);
            }
            let box_w = (x_max - x_min).abs().max(1e-9);
            let box_h = (y_max - y_min).abs().max(1e-9);

            // Matrix A: map transformed appearance box → annotation Rect.
            // sx/sy scale; tx/ty place lower-left at Rect lower-left.
            // PDF 32000-1 §12.5.5 steps 2–3.
            let sx = annot_w / box_w;
            let sy = annot_h / box_h;
            let tx = llx.min(urx) - x_min * sx;
            let ty = lly.min(ury) - y_min * sy;
            let a_matrix = [sx, 0.0, 0.0, sy, tx, ty];

            self.save()?;
            self.current_state_mut().concat_matrix(&a_matrix);
            self.device.concat_matrix(&a_matrix);
            self.push_root_ctm();

            let paint_result = self.paint_form_xobject("AnnotAP", &form_dict, &form_data);

            self.pop_root_ctm();
            let restore_result = self.restore();
            paint_result?;
            restore_result?;
        }
        Ok(())
    }

    /// Resolve `/AP /N` to a Form XObject stream (dict + bytes).
    /// Resolve `/AP /N` to a Form XObject stream (dict + bytes).
    pub(super) fn resolve_annotation_appearance(
        &mut self,
        annot_dict: &HashMap<String, PDFObject>,
    ) -> Option<(HashMap<String, PDFObject>, Vec<u8>)> {
        let ap = annot_dict.get("AP")?;
        let mut resolver = self.resolver.take();
        let ap_resolved = match resolver.as_mut() {
            Some(r) => r.resolve_if_ref(ap).ok(),
            None => Some(std::sync::Arc::new(ap.clone())),
        };
        let Some(ap_obj) = ap_resolved else {
            self.resolver = resolver;
            return None;
        };

        let n_obj = match ap_obj.as_ref() {
            PDFObject::Dictionary(ap_dict) => {
                let n = ap_dict.get("N")?;
                match resolver.as_mut() {
                    Some(r) => r.resolve_if_ref(n).ok(),
                    None => Some(std::sync::Arc::new(n.clone())),
                }
            }
            // Rare: AP is itself a stream used as the Normal appearance.
            PDFObject::Stream { .. } => Some(ap_obj.clone()),
            _ => None,
        };
        let Some(n_obj) = n_obj else {
            self.resolver = resolver;
            return None;
        };

        // N may be a stream or a dict of named states (use first stream entry).
        let stream_obj = match n_obj.as_ref() {
            PDFObject::Stream { .. } => Some(n_obj.clone()),
            PDFObject::Dictionary(states) => {
                let mut found = None;
                for v in states.values() {
                    let resolved = match resolver.as_mut() {
                        Some(r) => r.resolve_if_ref(v).ok(),
                        None => Some(std::sync::Arc::new(v.clone())),
                    };
                    if let Some(s) = resolved {
                        if matches!(s.as_ref(), PDFObject::Stream { .. }) {
                            found = Some(s);
                            break;
                        }
                    }
                }
                found
            }
            _ => None,
        };
        self.resolver = resolver;

        let stream_obj = stream_obj?;
        match stream_obj.as_ref() {
            PDFObject::Stream { dict, data } => Some((dict.clone(), data.clone())),
            _ => None,
        }
    }
    pub(super) fn paint_form_xobject(
        &mut self,
        xobject_name: &str,
        form_dict: &HashMap<String, PDFObject>,
        form_data: &[u8],
    ) -> PDFResult<()> {
        // C.3 render budget: stop descending at the nesting cap — a self- or
        // mutually-recursive Form would otherwise overflow the stack. Matches
        // the legacy Page::render path, which stops silently at the same depth.
        if self.form_render_depth >= Self::FORM_XOBJECT_MAX_DEPTH {
            return Ok(());
        }
        self.form_render_depth += 1;
        let result = self.paint_form_xobject_inner(xobject_name, form_dict, form_data);
        self.form_render_depth -= 1;
        result
    }

    fn paint_form_xobject_inner(
        &mut self,
        xobject_name: &str,
        form_dict: &HashMap<String, PDFObject>,
        form_data: &[u8],
    ) -> PDFResult<()> {
        // Decode the form stream if it uses filters.
        let decoded_data = if let Some(filter) = form_dict.get("Filter") {
            crate::core::decode::apply_filters(form_data, filter)
                .unwrap_or_else(|_| form_data.to_vec())
        } else {
            form_data.to_vec()
        };

        // Form XObjects can define their own resource dictionary; otherwise they
        // inherit the current one.
        let form_resources = form_dict.get("Resources").cloned();

        let matrix = parse_six_number_array(form_dict.get("Matrix"))
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let bbox = parse_four_number_array(form_dict.get("BBox"));
        let is_transparency_group = self.soft_mask_render_depth == 0
            && form_has_transparency_group(form_dict, &mut self.resolver);

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: PaintXObject: Entering form '{}' ({} bytes), matrix={:?}, bbox={:?}",
            xobject_name,
            decoded_data.len(),
            matrix,
            bbox
        );

        let group_opacity = self.current_state().fill_alpha;
        if is_transparency_group {
            self.device.save_state();
            let _ = self.device.set_soft_mask(None);
            self.device.set_opacity(1.0);
            self.device.begin_offscreen()?;
        }

        self.save()?;
        let previous_resources = self.resources.clone();

        // Keep restore/resource rollback deterministic even on early errors.
        let result = (|| -> PDFResult<()> {
            if is_transparency_group {
                self.current_state_mut().fill_alpha = 1.0;
                self.current_state_mut().stroke_alpha = 1.0;
            }

            // Apply form matrix to map the form space into the current CTM.
            self.current_state_mut().concat_matrix(&matrix);
            self.device.concat_matrix(&matrix);
            self.push_root_ctm();

            // Clip to the form's BBox when provided.
            let form_result = (|| -> PDFResult<()> {
                if let Some([x0, y0, x1, y1]) = bbox {
                    self.device.begin_path();
                    self.device.move_to(x0, y0);
                    self.device.line_to(x1, y0);
                    self.device.line_to(x1, y1);
                    self.device.line_to(x0, y1);
                    self.device.close_path();
                    self.device.clip_path(FillRule::NonZero)?;
                }

                self.resources = merge_pattern_resources(
                    form_resources,
                    previous_resources.clone(),
                    &mut self.resolver,
                );

                self.process_content_stream_bytes(decoded_data)
            })();
            self.pop_root_ctm();
            form_result
        })();

        self.resources = previous_resources;
        let restore_result = self.restore();

        if is_transparency_group {
            self.device.restore_state();
            self.device.set_opacity(group_opacity);
            self.device.end_offscreen_composite(group_opacity)?;
        }

        if let Err(e) = result {
            return Err(e);
        }
        restore_result
    }
}
