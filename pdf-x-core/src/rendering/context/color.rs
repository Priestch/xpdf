//! Color operators (g/rg/k/cs/sc/SC and friends) and color-space resolution.

use super::*;

pub(super) fn is_pattern_color_space(color_space: &PDFObject) -> bool {
    match color_space {
        PDFObject::Name(name) => name == "Pattern",
        PDFObject::Array(arr) => {
            matches!(arr.first().map(|v| v.as_ref()), Some(PDFObject::Name(name)) if name == "Pattern")
        }
        _ => false,
    }
}

impl<'a, D: Device> RenderingContext<'a, D> {
    pub(super) fn set_stroke_gray(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "G operator requires 1 argument".to_string(),
            ));
        }

        let gray = extract_number(args, 0)?;
        // G implicitly selects DeviceGray and replaces any Pattern stroke colour.
        self.pending_stroke_shading = None;
        self.stroke_color_space = Some(PDFObject::Name("DeviceGray".to_string()));
        self.current_state_mut().stroke_color = Color::Gray(gray);
        Ok(())
    }
    pub(super) fn set_fill_gray(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "g operator requires 1 argument".to_string(),
            ));
        }

        let gray = extract_number(args, 0)?;
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: set_fill_gray: gray={:.3}", gray);
        self.pending_fill_shading = None;
        self.pending_fill_tiling = None;
        self.fill_color_space = Some(PDFObject::Name("DeviceGray".to_string()));
        self.current_state_mut().fill_color = Color::Gray(gray);
        Ok(())
    }
    pub(super) fn set_stroke_rgb(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 3 {
            return Err(PDFError::content_stream_error(
                "RG operator requires 3 arguments".to_string(),
            ));
        }

        let r = extract_number(args, 0)?;
        let g = extract_number(args, 1)?;
        let b = extract_number(args, 2)?;
        // RG implicitly selects DeviceRGB and replaces any Pattern stroke colour.
        self.pending_stroke_shading = None;
        self.stroke_color_space = Some(PDFObject::Name("DeviceRGB".to_string()));
        self.current_state_mut().stroke_color = Color::RGB(r, g, b);
        Ok(())
    }
    pub(super) fn set_fill_rgb(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 3 {
            return Err(PDFError::content_stream_error(
                "rg operator requires 3 arguments".to_string(),
            ));
        }

        let r = extract_number(args, 0)?;
        let g = extract_number(args, 1)?;
        let b = extract_number(args, 2)?;
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: set_fill_rgb: r={}, g={}, b={}, args={:?}",
            r, g, b, args
        );
        self.pending_fill_shading = None;
        self.pending_fill_tiling = None;
        self.fill_color_space = Some(PDFObject::Name("DeviceRGB".to_string()));
        self.current_state_mut().fill_color = Color::RGB(r, g, b);
        Ok(())
    }
    pub(super) fn set_stroke_cmyk(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 4 {
            return Err(PDFError::content_stream_error(
                "K operator requires 4 arguments".to_string(),
            ));
        }

        let c = extract_number(args, 0)?;
        let m = extract_number(args, 1)?;
        let y = extract_number(args, 2)?;
        let k = extract_number(args, 3)?;
        self.pending_stroke_shading = None;
        self.stroke_color_space = Some(PDFObject::Name("DeviceCMYK".to_string()));
        self.current_state_mut().stroke_color = Color::CMYK(c, m, y, k);
        Ok(())
    }
    pub(super) fn set_fill_cmyk(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 4 {
            return Err(PDFError::content_stream_error(
                "k operator requires 4 arguments".to_string(),
            ));
        }

        let c = extract_number(args, 0)?;
        let m = extract_number(args, 1)?;
        let y = extract_number(args, 2)?;
        let k = extract_number(args, 3)?;
        self.pending_fill_shading = None;
        self.pending_fill_tiling = None;
        self.fill_color_space = Some(PDFObject::Name("DeviceCMYK".to_string()));
        self.current_state_mut().fill_color = Color::CMYK(c, m, y, k);
        Ok(())
    }
    pub(super) fn set_stroke_color_space(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        self.stroke_color_space = self.resolve_color_space_arg(args)?;
        // Leaving a Pattern stroke colour space clears any sticky stroke pattern.
        if !self
            .stroke_color_space
            .as_ref()
            .is_some_and(is_pattern_color_space)
        {
            self.pending_stroke_shading = None;
        }
        if let Some(color) =
            initial_color_for_color_space(self.stroke_color_space.clone(), &mut self.resolver)
        {
            self.current_state_mut().stroke_color = color;
        }
        Ok(())
    }
    pub(super) fn set_fill_color_space(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        self.fill_color_space = self.resolve_color_space_arg(args)?;
        if !self
            .fill_color_space
            .as_ref()
            .is_some_and(is_pattern_color_space)
        {
            self.pending_fill_shading = None;
            self.pending_fill_tiling = None;
        }
        if let Some(color) =
            initial_color_for_color_space(self.fill_color_space.clone(), &mut self.resolver)
        {
            self.current_state_mut().fill_color = color;
        }
        Ok(())
    }
    pub(super) fn set_stroke_color(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        if let Some(cs) = self.stroke_color_space.clone() {
            if is_pattern_color_space(&cs) {
                return self.set_pattern_stroke(args);
            }
            if let Some(color) = self.color_from_current_space(args, Some(cs))? {
                self.pending_stroke_shading = None;
                self.current_state_mut().stroke_color = color;
            }
            return Ok(());
        }
        self.pending_stroke_shading = None;
        self.set_color_by_component_count(args, false)
    }
    pub(super) fn set_fill_color(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        if let Some(color) = self.stencil_pattern_color {
            self.current_state_mut().fill_color = color;
            return Ok(());
        }

        // Check for Pattern color space
        if let Some(cs) = self.fill_color_space.clone() {
            if is_pattern_color_space(&cs) {
                return self.set_pattern_fill(args);
            }
        }
        if self.fill_color_space.is_some() {
            if let Some(color) =
                self.color_from_current_space(args, self.fill_color_space.clone())?
            {
                self.pending_fill_shading = None;
                self.pending_fill_tiling = None;
                self.current_state_mut().fill_color = color;
            }
            return Ok(());
        }
        self.pending_fill_shading = None;
        self.pending_fill_tiling = None;
        self.set_color_by_component_count(args, true)
    }
    pub(super) fn set_pattern_stroke(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        // Mirror set_pattern_fill for the stroke colour operators (CS/SCN).
        // Source: PDF 1.7 §8.7.3 / text_stroked_complex_paint.pdf.
        if let Some(shading) = self.resolve_pattern_type2_shading(args) {
            self.pending_stroke_shading = Some(shading);
        }
        Ok(())
    }
    pub(super) fn set_pattern_fill(&mut self, args: &[PDFObject]) -> PDFResult<()> {
        let pattern_name = match args.last() {
            Some(PDFObject::Name(n)) => n.clone(),
            _ => return Ok(()),
        };

        let resources = match self.resources.clone() {
            Some(r) => r,
            None => return Ok(()),
        };

        let pattern_obj = {
            let resolved = resolve_object_clone(resources, &mut self.resolver);
            match resolved {
                Some(PDFObject::Dictionary(d)) => d.get("Pattern").cloned(),
                _ => None,
            }
        };

        let pattern_ref = match pattern_obj {
            Some(p) => p,
            None => return Ok(()),
        };

        let pattern_dict = resolve_object_clone(pattern_ref, &mut self.resolver);
        let pattern_entry = match pattern_dict {
            Some(PDFObject::Dictionary(d)) => d.get(&pattern_name).cloned(),
            _ => None,
        };

        let pattern_entry = match pattern_entry {
            Some(p) => p,
            None => return Ok(()),
        };

        let resolved_pattern =
            resolve_object_clone(pattern_entry, &mut self.resolver).unwrap_or(PDFObject::Null);
        let pattern_dict = match &resolved_pattern {
            PDFObject::Dictionary(d) => d,
            PDFObject::Stream { dict, .. } => dict,
            _ => return Ok(()),
        };

        let pattern_type = pattern_dict
            .get("PatternType")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;

        if pattern_type == 1 {
            self.pending_fill_shading = None;
            return self.set_tiling_pattern_fill(pattern_dict, &resolved_pattern, args);
        }

        if pattern_type != 2 {
            return Ok(());
        }

        if let Some(shading) = self.build_type2_pattern_shading(pattern_dict) {
            self.pending_fill_tiling = None;
            self.pending_fill_shading = Some(shading);
        }

        Ok(())
    }

    /// Resolve a Pattern Type 2 shading for stroke (`SCN`) from resource name args.
    /// Resolve a Pattern Type 2 shading for stroke (`SCN`) from resource name args.
    pub(super) fn resolve_pattern_type2_shading(
        &mut self,
        args: &[PDFObject],
    ) -> Option<crate::rendering::shading::Shading> {
        let pattern_name = match args.last() {
            Some(PDFObject::Name(n)) => n.clone(),
            _ => return None,
        };
        let resources = self.resources.clone()?;
        let pattern_res = {
            let resolved = resolve_object_clone(resources, &mut self.resolver)?;
            match resolved {
                PDFObject::Dictionary(d) => d.get("Pattern").cloned()?,
                _ => return None,
            }
        };
        let pattern_dict = resolve_object_clone(pattern_res, &mut self.resolver)?;
        let pattern_entry = match pattern_dict {
            PDFObject::Dictionary(d) => d.get(&pattern_name).cloned()?,
            _ => return None,
        };
        let resolved_pattern =
            resolve_object_clone(pattern_entry, &mut self.resolver).unwrap_or(PDFObject::Null);
        let pattern_dict = match &resolved_pattern {
            PDFObject::Dictionary(d) => d,
            PDFObject::Stream { dict, .. } => dict,
            _ => return None,
        };
        let pattern_type = pattern_dict
            .get("PatternType")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;
        if pattern_type != 2 {
            return None;
        }
        self.build_type2_pattern_shading(pattern_dict)
    }
    pub(super) fn build_type2_pattern_shading(
        &mut self,
        pattern_dict: &HashMap<String, PDFObject>,
    ) -> Option<crate::rendering::shading::Shading> {
        let shading_ref = pattern_dict.get("Shading").cloned()?;
        let shading_obj =
            resolve_object_clone(shading_ref, &mut self.resolver).unwrap_or(PDFObject::Null);
        let (shading_dict, shading_data) = match &shading_obj {
            PDFObject::Dictionary(d) => (d, None),
            PDFObject::Stream { dict, data } => (dict, Some(data.as_slice())),
            _ => return None,
        };

        let shading_type = shading_dict
            .get("ShadingType")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;

        if !matches!(shading_type, 1..=4 | 6..=7) {
            return None;
        }

        let mut shading = parse_shading(shading_dict, shading_data, &mut self.resolver)?;
        shading.use_root_ctm = true;

        let pattern_matrix = match pattern_dict.get("Matrix") {
            Some(PDFObject::Array(arr)) if arr.len() >= 6 => {
                let vals: Vec<f64> = arr.iter().filter_map(|v| v.as_number()).collect();
                if vals.len() >= 6 {
                    [vals[0], vals[1], vals[2], vals[3], vals[4], vals[5]]
                } else {
                    [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
                }
            }
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        };
        shading.matrix = post_concat_matrix(pattern_matrix, shading.matrix);
        Some(shading)
    }
    pub(super) fn set_color_by_component_count(
        &mut self,
        args: &[PDFObject],
        fill: bool,
    ) -> PDFResult<()> {
        let color = match args.len() {
            1 => Color::Gray(extract_number(args, 0)?),
            3 => Color::RGB(
                extract_number(args, 0)?,
                extract_number(args, 1)?,
                extract_number(args, 2)?,
            ),
            4 => Color::CMYK(
                extract_number(args, 0)?,
                extract_number(args, 1)?,
                extract_number(args, 2)?,
                extract_number(args, 3)?,
            ),
            _ => return Ok(()),
        };
        if fill {
            self.current_state_mut().fill_color = color;
        } else {
            self.current_state_mut().stroke_color = color;
        }
        Ok(())
    }
    pub(super) fn resolve_color_space_arg(
        &mut self,
        args: &[PDFObject],
    ) -> PDFResult<Option<PDFObject>> {
        let Some(PDFObject::Name(name)) = args.first() else {
            return Ok(None);
        };
        Ok(self
            .lookup_color_space_resource(name)
            .or_else(|| Some(PDFObject::Name(name.clone()))))
    }
    pub(super) fn lookup_color_space_resource(&mut self, name: &str) -> Option<PDFObject> {
        let resources = self.resources.clone()?;
        let resources = resolve_object_clone(resources, &mut self.resolver)?;
        let PDFObject::Dictionary(resources_dict) = resources else {
            return None;
        };
        let color_spaces = resources_dict.get("ColorSpace")?.clone();
        let color_spaces = resolve_object_clone(color_spaces, &mut self.resolver)?;
        let PDFObject::Dictionary(color_spaces_dict) = color_spaces else {
            return None;
        };
        let color_space = color_spaces_dict.get(name)?.clone();
        resolve_object_clone(color_space, &mut self.resolver)
    }
    pub(super) fn color_from_current_space(
        &mut self,
        args: &[PDFObject],
        color_space: Option<PDFObject>,
    ) -> PDFResult<Option<Color>> {
        let Some(color_space) = color_space else {
            return Ok(None);
        };
        let color_space = match resolve_object_clone(color_space, &mut self.resolver) {
            Some(cs) => cs,
            None => return Ok(None),
        };
        Ok(color_from_color_space(
            &color_space,
            args,
            &mut self.resolver,
        ))
    }

    // === Line Property Operators ===
}
