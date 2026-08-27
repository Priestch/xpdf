//! ExtGState (gs), dash/line properties, soft masks, and shading fills (sh).

use super::*;

pub(super) fn bool_pair_array(object: Option<&PDFObject>) -> Option<[bool; 2]> {
    let PDFObject::Array(arr) = object? else {
        return None;
    };

    let mut out = [true; 2];
    for (index, slot) in out.iter_mut().enumerate() {
        let Some(item) = arr.get(index) else {
            break;
        };
        let PDFObject::Boolean(value) = item.as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

/// Parse a shading function from a PDFObject (Type 2 exponential or Type 4 PostScript).
pub(super) fn parse_four_number_array(obj: Option<&PDFObject>) -> Option<[f64; 4]> {
    let PDFObject::Array(values) = obj? else {
        return None;
    };
    if values.len() < 4 {
        return None;
    }

    let mut out = [0.0f64; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let PDFObject::Number(value) = values[i].as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

pub(super) fn parse_three_number_array(obj: Option<&PDFObject>) -> Option<[f64; 3]> {
    let PDFObject::Array(values) = obj? else {
        return None;
    };
    if values.len() < 3 {
        return None;
    }

    let mut out = [0.0f64; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        let PDFObject::Number(value) = values[i].as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

pub(super) fn parse_nine_number_array(obj: Option<&PDFObject>) -> Option<[f64; 9]> {
    let PDFObject::Array(values) = obj? else {
        return None;
    };
    if values.len() < 9 {
        return None;
    }

    let mut out = [0.0f64; 9];
    for (i, slot) in out.iter_mut().enumerate() {
        let PDFObject::Number(value) = values[i].as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

pub(super) fn parse_six_number_array(obj: Option<&PDFObject>) -> Option<[f64; 6]> {
    let PDFObject::Array(values) = obj? else {
        return None;
    };
    if values.len() < 6 {
        return None;
    }

    let mut out = [0.0f64; 6];
    for (i, slot) in out.iter_mut().enumerate() {
        let PDFObject::Number(value) = values[i].as_ref() else {
            return None;
        };
        *slot = *value;
    }
    Some(out)
}

pub(super) fn soft_mask_backdrop_rgb(obj: Option<&PDFObject>) -> Option<[u8; 3]> {
    let components = number_array(obj)?;
    let color = match components.as_slice() {
        [g] => Color::Gray(*g),
        [r, g, b] => Color::RGB(*r, *g, *b),
        [c, m, y, k] => Color::CMYK(*c, *m, *y, *k),
        _ => return None,
    };
    let (r, g, b, _) = color.rgba();
    Some([r, g, b])
}

pub(super) fn form_has_transparency_group(
    form_dict: &HashMap<String, PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> bool {
    let Some(group_obj) = form_dict.get("Group").cloned() else {
        return false;
    };
    let Some(PDFObject::Dictionary(group_dict)) = resolve_object_clone(group_obj, resolver) else {
        return false;
    };
    matches!(group_dict.get("S"), Some(PDFObject::Name(name)) if name == "Transparency")
}

impl<'a, D: Device> RenderingContext<'a, D> {
    pub(super) fn set_line_width(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "w operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().stroke_props.line_width = extract_number(args, 0)?;
        Ok(())
    }
    pub(super) fn set_line_cap(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "J operator requires 1 argument".to_string(),
            ));
        }

        let cap = extract_number(args, 0)? as i32;
        self.current_state_mut().stroke_props.line_cap = match cap {
            0 => crate::rendering::graphics_state::LineCap::Butt,
            1 => crate::rendering::graphics_state::LineCap::Round,
            2 => crate::rendering::graphics_state::LineCap::ProjectingSquare,
            _ => {
                return Err(PDFError::content_stream_error(format!(
                    "Invalid line cap: {}",
                    cap
                )));
            }
        };
        Ok(())
    }
    pub(super) fn set_line_join(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "j operator requires 1 argument".to_string(),
            ));
        }

        let join = extract_number(args, 0)? as i32;
        self.current_state_mut().stroke_props.line_join = match join {
            0 => crate::rendering::graphics_state::LineJoin::Miter,
            1 => crate::rendering::graphics_state::LineJoin::Round,
            2 => crate::rendering::graphics_state::LineJoin::Bevel,
            _ => {
                return Err(PDFError::content_stream_error(format!(
                    "Invalid line join: {}",
                    join
                )));
            }
        };
        Ok(())
    }
    pub(super) fn set_miter_limit(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        if args.len() < 1 {
            return Err(PDFError::content_stream_error(
                "M operator requires 1 argument".to_string(),
            ));
        }

        self.current_state_mut().stroke_props.miter_limit = extract_number(args, 0)?;
        Ok(())
    }
    pub(super) fn set_dash(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        // d operator: dash_array dash_offset
        if args.len() < 2 {
            return Err(PDFError::content_stream_error(
                "d operator requires 2 arguments".to_string(),
            ));
        }

        let mut dash_array = Vec::new();
        if let crate::core::parser::PDFObject::Array(arr) = &args[0] {
            for obj in arr {
                match &**obj {
                    crate::core::parser::PDFObject::Number(n) => dash_array.push(*n),
                    _ => {
                        return Err(PDFError::content_stream_error(
                            "Dash array must contain only numbers".to_string(),
                        ));
                    }
                }
            }
        }

        let dash_offset = extract_number(args, 1)?;

        self.current_state_mut().stroke_props.dash_array = dash_array;
        self.current_state_mut().stroke_props.dash_offset = dash_offset;
        Ok(())
    }
    pub(super) fn set_graphics_state(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        let Some(PDFObject::Name(gs_name)) = args.first() else {
            return Ok(());
        };

        let resources = match self.resources.as_ref() {
            Some(r) => r,
            None => return Ok(()),
        };

        let resources_dict = match resources {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(()),
        };

        let ext_gstate_obj = match resources_dict.get("ExtGState") {
            Some(obj) => obj,
            None => return Ok(()),
        };

        let xref_opt = self.resolver.take();
        let resolver = match xref_opt {
            Some(resolver) => resolver,
            None => return Ok(()),
        };

        let ext_gstate_dict_obj = match resolver.resolve_if_ref(ext_gstate_obj) {
            Ok(obj) => obj,
            Err(_) => {
                self.resolver = Some(resolver);
                return Ok(());
            }
        };

        let ext_gstate_dict = match &*ext_gstate_dict_obj {
            PDFObject::Dictionary(dict) => dict,
            _ => {
                self.resolver = Some(resolver);
                return Ok(());
            }
        };

        let Some(gs_obj_ref) = ext_gstate_dict.get(gs_name) else {
            self.resolver = Some(resolver);
            return Ok(());
        };

        let gs_obj = match resolver.resolve_if_ref(gs_obj_ref) {
            Ok(obj) => obj,
            Err(_) => {
                self.resolver = Some(resolver);
                return Ok(());
            }
        };

        // Put resolver back so SMask rendering can use it
        self.resolver = Some(resolver);

        if let PDFObject::Dictionary(gs_dict) = &*gs_obj {
            if let Some(mode) = gs_dict.get("BM").and_then(parse_blend_mode) {
                self.current_state_mut().blend_mode = mode;
                self.device.set_blend_mode(mode);
            }

            // Handle fill/stroke alpha
            if let Some(ca) = gs_dict.get("ca").and_then(|v| v.as_number()) {
                self.current_state_mut().fill_alpha = ca;
                self.device.set_opacity(ca);
            }
            if let Some(ca) = gs_dict.get("CA").and_then(|v| v.as_number()) {
                self.current_state_mut().stroke_alpha = ca;
            }

            // Handle SMask (soft mask)
            let smask_val = gs_dict.get("SMask").cloned();
            let smask_resolved = match &smask_val {
                Some(PDFObject::Ref(_)) => {
                    let mut resolver = self.resolver.take();
                    let r = resolve_object_clone(smask_val.clone().unwrap(), &mut resolver);
                    self.resolver = resolver;
                    r
                }
                Some(_) => smask_val.clone(),
                None => None,
            };
            match smask_resolved.as_ref() {
                Some(PDFObject::Name(n)) if n == "None" => {
                    let _ = self.device.set_soft_mask(None);
                }
                Some(PDFObject::Dictionary(smask_dict)) => {
                    self.render_and_set_soft_mask(smask_dict.clone());
                }
                _ => {}
            }
        }

        Ok(())
    }
    pub(super) fn render_and_set_soft_mask(&mut self, smask_dict: HashMap<String, PDFObject>) {
        let luminosity = match smask_dict.get("S") {
            Some(PDFObject::Name(n)) => n == "Luminosity",
            _ => false,
        };
        let backdrop = if luminosity {
            soft_mask_backdrop_rgb(smask_dict.get("BC")).or(Some([0, 0, 0]))
        } else {
            None
        };

        let group_ref = match smask_dict.get("G") {
            Some(g) => g.clone(),
            None => return,
        };

        let group_key = match &group_ref {
            PDFObject::Ref(r) => Some((r.num, r.generation)),
            _ => None,
        };
        if group_key.is_some_and(|key| self.soft_mask_group_stack.contains(&key)) {
            return;
        }

        let group_obj = match &mut self.resolver {
            Some(r) => r.resolve_if_ref(&group_ref).ok(),
            None => None,
        };

        let (form_dict, form_data) = match group_obj.as_deref() {
            Some(PDFObject::Stream { dict, data }) => (dict.clone(), data.clone()),
            _ => return,
        };

        let decoded_data = if let Some(filter) = form_dict.get("Filter") {
            crate::core::decode::apply_filters(&form_data, filter).unwrap_or(form_data)
        } else {
            form_data
        };

        let matrix = parse_six_number_array(form_dict.get("Matrix"))
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let bbox = parse_four_number_array(form_dict.get("BBox"));

        // Render the mask content offscreen.
        // Reset opacity to 1.0 — the mask group is independent of the current ca.
        let saved_opacity = self.current_state().fill_alpha;
        self.device.set_opacity(1.0);
        if let Some(key) = group_key {
            self.soft_mask_group_stack.push(key);
        }
        let _ = self.device.begin_offscreen();

        // Save current resources and set form resources if available
        let prev_resources = self.resources.clone();
        let mut resolver = self.resolver.take();
        if let Some(form_res) = form_dict.get("Resources").cloned() {
            let resolved = resolve_object_clone(form_res, &mut resolver);
            if let Some(res) = resolved {
                self.resources = Some(res);
            }
        }
        self.resolver = resolver;

        // Process the mask content stream
        self.soft_mask_render_depth += 1;
        if self.save().is_ok() {
            self.current_state_mut().concat_matrix(&matrix);
            self.device.concat_matrix(&matrix);
            self.push_root_ctm();

            if let Some([x0, y0, x1, y1]) = bbox {
                self.device.begin_path();
                self.device.move_to(x0, y0);
                self.device.line_to(x1, y0);
                self.device.line_to(x1, y1);
                self.device.line_to(x0, y1);
                self.device.close_path();
                let _ = self.device.clip_path(FillRule::NonZero);
            }

            let _ = self.process_content_stream_bytes(decoded_data);
            self.pop_root_ctm();
            let _ = self.restore();
        }
        self.soft_mask_render_depth -= 1;

        // Restore resources
        self.resources = prev_resources;

        // Extract mask and set it
        if let Some((mask_data, mw, mh)) = self.device.end_offscreen_as_mask(luminosity, backdrop) {
            let _ = self.device.set_soft_mask(Some((&mask_data, mw, mh)));
        }
        // Restore the original opacity
        self.device.set_opacity(saved_opacity);
        if group_key.is_some() {
            self.soft_mask_group_stack.pop();
        }
    }

    // === Shading Operator ===
    pub(super) fn handle_shading_fill(
        &mut self,
        args: &[crate::core::parser::PDFObject],
    ) -> PDFResult<()> {
        let shading_name = match args.first() {
            Some(PDFObject::Name(n)) => n.as_str(),
            _ => return Ok(()),
        };

        let resources = match self.resources.as_ref() {
            Some(PDFObject::Dictionary(d)) => d.clone(),
            _ => return Ok(()),
        };

        let shading_resources = match resources.get("Shading").cloned() {
            Some(s) => s,
            None => return Ok(()),
        };

        let shading_ref = match resolve_object_clone(shading_resources, &mut self.resolver) {
            Some(PDFObject::Dictionary(d)) => d.get(shading_name).cloned(),
            Some(other) => Some(other),
            None => None,
        };

        let shading_ref = match shading_ref {
            Some(s) => s,
            None => return Ok(()),
        };

        let shading_obj =
            resolve_object_clone(shading_ref, &mut self.resolver).unwrap_or(PDFObject::Null);
        let (shading_dict, shading_data) = match &shading_obj {
            PDFObject::Dictionary(d) => (d, None),
            PDFObject::Stream { dict, data } => (dict, Some(data.as_slice())),
            _ => return Ok(()),
        };

        let shading_type = shading_dict
            .get("ShadingType")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;

        if !matches!(shading_type, 1..=4 | 6..=7) {
            return Ok(());
        }

        let shading = match parse_shading(shading_dict, shading_data, &mut self.resolver) {
            Some(shading) => shading,
            None => return Ok(()),
        };

        let ctm = post_concat_matrix(self.current_state().ctm, shading.matrix);
        self.device.draw_shading(&shading, &ctm)
    }

    // === XObject Operator ===
}
