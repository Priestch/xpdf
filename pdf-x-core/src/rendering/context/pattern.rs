//! Tiling-pattern fills (Pattern color spaces, scn with pattern dicts, cell replay).

use super::*;

#[derive(Debug, Clone)]
pub(super) struct TilingPatternPaint {
    pub(super) bbox: [f64; 4],
    pub(super) x_step: f64,
    pub(super) y_step: f64,
    pub(super) matrix: [f64; 6],
    pub(super) data: Vec<u8>,
    pub(super) resources: Option<PDFObject>,
    pub(super) paint_type: i32,
    pub(super) stencil_color: Option<Color>,
}

pub(super) fn merge_pattern_resources(
    pattern_resources: Option<PDFObject>,
    parent_resources: Option<PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<PDFObject> {
    let pattern_resources = pattern_resources.and_then(|r| resolve_object_clone(r, resolver));
    let parent_resources = parent_resources.and_then(|r| resolve_object_clone(r, resolver));

    match (pattern_resources, parent_resources) {
        (Some(PDFObject::Dictionary(mut pattern)), Some(PDFObject::Dictionary(parent))) => {
            for (key, value) in parent {
                pattern.entry(key).or_insert(value);
            }
            Some(PDFObject::Dictionary(pattern))
        }
        (Some(resources), _) => Some(resources),
        (None, Some(resources)) => Some(resources),
        (None, None) => None,
    }
}

pub(super) fn pattern_tile_bounds(
    pattern: &TilingPatternPaint,
    fill_bbox: Option<(f64, f64, f64, f64)>,
    page_bounds: (f64, f64),
    ctm: [f64; 6],
    base: [f64; 6],
) -> (i32, i32, i32, i32) {
    let device_bbox = fill_bbox.map(|bbox| transform_bbox(bbox, ctm)).unwrap_or((
        0.0,
        0.0,
        page_bounds.0,
        page_bounds.1,
    ));

    let expanded = (
        device_bbox.0.floor().max(0.0),
        device_bbox.1.floor().max(0.0),
        device_bbox.2.ceil().min(page_bounds.0),
        device_bbox.3.ceil().min(page_bounds.1),
    );

    if let Some(inv) = invert_matrix(base) {
        let pattern_bbox = transform_bbox(expanded, inv);
        tile_range_for_axis(
            pattern_bbox.0,
            pattern_bbox.2,
            pattern.bbox[0],
            pattern.bbox[2],
            pattern.x_step,
        )
        .zip(tile_range_for_axis(
            pattern_bbox.1,
            pattern_bbox.3,
            pattern.bbox[1],
            pattern.bbox[3],
            pattern.y_step,
        ))
        .map(|((ix0, ix1), (iy0, iy1))| (ix0, ix1, iy0, iy1))
        .unwrap_or((0, 0, 0, 0))
    } else {
        (0, 0, 0, 0)
    }
}

#[allow(unused_variables)]
pub(super) fn trace_tiling_pattern(
    stage: &str,
    pattern: &TilingPatternPaint,
    fill_bbox: Option<(f64, f64, f64, f64)>,
    page_bounds: (f64, f64),
    ctm: [f64; 6],
    base: [f64; 6],
    bounds: (i32, i32, i32, i32),
) {
    #[cfg(feature = "debug-logging")]
    {
        let device_bbox = fill_bbox.map(|bbox| transform_bbox(bbox, ctm)).unwrap_or((
            0.0,
            0.0,
            page_bounds.0,
            page_bounds.1,
        ));
        let clip = (
            device_bbox.0.floor().max(0.0),
            device_bbox.1.floor().max(0.0),
            device_bbox.2.ceil().min(page_bounds.0),
            device_bbox.3.ceil().min(page_bounds.1),
        );
        let cell_bbox = transform_bbox(
            (
                pattern.bbox[0],
                pattern.bbox[1],
                pattern.bbox[2],
                pattern.bbox[3],
            ),
            base,
        );
        let bitmap_w = cell_bbox_width(cell_bbox).ceil().max(1.0) as i32;
        let bitmap_h = cell_bbox_height(cell_bbox).ceil().max(1.0) as i32;
        let pdfium_bounds = pdfium_like_tile_bounds(pattern, clip, base);

        eprintln!(
            "[tiling-trace:{stage}] bbox={:?} xstep={:.6} ystep={:.6} matrix={:?}",
            pattern.bbox, pattern.x_step, pattern.y_step, pattern.matrix
        );
        eprintln!(
            "[tiling-trace:{stage}] ctm={:?} root_pattern_base={:?} fill_bbox={:?} clip={:?}",
            ctm, base, fill_bbox, clip
        );
        eprintln!(
            "[tiling-trace:{stage}] cell_bbox_device={:?} bitmap={}x{} ours_bounds={:?} pdfium_bounds={:?}",
            cell_bbox, bitmap_w, bitmap_h, bounds, pdfium_bounds
        );
        if let Some((min_col, max_col, min_row, max_row)) = pdfium_bounds {
            let left_offset = cell_bbox.0 - base[4];
            let top_offset = cell_bbox.1 - base[5];
            eprintln!(
                "[tiling-trace:{stage}] offsets left={:.6} top={:.6} first_start=({:.6},{:.6}) last_start=({:.6},{:.6})",
                left_offset,
                top_offset,
                base[0] * (min_col as f64 * pattern.x_step)
                    + base[2] * (min_row as f64 * pattern.y_step)
                    + base[4]
                    + left_offset,
                base[1] * (min_col as f64 * pattern.x_step)
                    + base[3] * (min_row as f64 * pattern.y_step)
                    + base[5]
                    + top_offset,
                base[0] * (max_col as f64 * pattern.x_step)
                    + base[2] * (max_row as f64 * pattern.y_step)
                    + base[4]
                    + left_offset,
                base[1] * (max_col as f64 * pattern.x_step)
                    + base[3] * (max_row as f64 * pattern.y_step)
                    + base[5]
                    + top_offset,
            );
        }
    }
}

#[allow(unused_variables)]
pub(super) fn trace_tiling_cell(
    pattern: &TilingPatternPaint,
    base: [f64; 6],
    cell_transform: [f64; 6],
    image_to_device: [f64; 6],
    pix_w: u32,
    pix_h: u32,
    render_w: u32,
    render_h: u32,
) {
    #[cfg(feature = "debug-logging")]
    {
        let bbox_w = (pattern.bbox[2] - pattern.bbox[0]).abs();
        let bbox_h = (pattern.bbox[3] - pattern.bbox[1]).abs();
        eprintln!(
            "[tiling-cell-core] bbox_size=({:.6},{:.6}) step=({:.6},{:.6}) pix={}x{} render={}x{} overlap=({}, {})",
            bbox_w,
            bbox_h,
            pattern.x_step,
            pattern.y_step,
            pix_w,
            pix_h,
            render_w,
            render_h,
            bbox_w > pattern.x_step.abs(),
            bbox_h > pattern.y_step.abs(),
        );
        eprintln!(
            "[tiling-cell-core] base={:?} cell_transform={:?} image_to_device={:?}",
            base, cell_transform, image_to_device
        );
    }
}

#[cfg(feature = "debug-logging")]
pub(super) fn cell_bbox_width(bbox: (f64, f64, f64, f64)) -> f64 {
    (bbox.2 - bbox.0).abs()
}

#[cfg(feature = "debug-logging")]
pub(super) fn cell_bbox_height(bbox: (f64, f64, f64, f64)) -> f64 {
    (bbox.3 - bbox.1).abs()
}

pub(super) fn pattern_has_overlapping_tiles(pattern: &TilingPatternPaint) -> bool {
    let width = (pattern.bbox[2] - pattern.bbox[0]).abs();
    let height = (pattern.bbox[3] - pattern.bbox[1]).abs();
    pattern.x_step.abs() < width || pattern.y_step.abs() < height
}

impl<'a, D: Device> RenderingContext<'a, D> {
    pub(super) fn set_tiling_pattern_fill(
        &mut self,
        pattern_dict: &HashMap<String, PDFObject>,
        pattern_obj: &PDFObject,
        args: &[PDFObject],
    ) -> PDFResult<()> {
        let PDFObject::Stream { data, .. } = pattern_obj else {
            return Ok(());
        };

        let bbox = match parse_four_number_array(pattern_dict.get("BBox")) {
            Some([x0, y0, x1, y1])
                if (x1 - x0).abs() > f64::EPSILON && (y1 - y0).abs() > f64::EPSILON =>
            {
                [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
            }
            _ => return Ok(()),
        };

        let x_step = pattern_dict
            .get("XStep")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0)
            .abs();
        let y_step = pattern_dict
            .get("YStep")
            .and_then(|v| v.as_number())
            .unwrap_or(0.0)
            .abs();
        if x_step.abs() <= f64::EPSILON || y_step.abs() <= f64::EPSILON {
            return Ok(());
        }

        let paint_type = pattern_dict
            .get("PaintType")
            .and_then(|v| v.as_number())
            .unwrap_or(1.0) as i32;

        let stencil_color = if paint_type == 2 {
            let color_args = &args[..args.len().saturating_sub(1)];
            self.pattern_base_fill_color(color_args)?
                .or_else(|| Some(self.current_state().fill_color))
        } else {
            None
        };

        let decoded = if let Some(filter) = pattern_dict.get("Filter") {
            crate::core::decode::apply_filters(data, filter).unwrap_or_else(|_| data.clone())
        } else {
            data.clone()
        };

        let matrix = parse_six_number_array(pattern_dict.get("Matrix"))
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let parent_resources = self.resources.clone();
        let resources = merge_pattern_resources(
            pattern_dict.get("Resources").cloned(),
            parent_resources,
            &mut self.resolver,
        );

        self.pending_fill_tiling = Some(TilingPatternPaint {
            bbox,
            x_step,
            y_step,
            matrix,
            data: decoded,
            resources,
            paint_type,
            stencil_color,
        });

        Ok(())
    }
    pub(super) fn pattern_base_fill_color(
        &mut self,
        args: &[PDFObject],
    ) -> PDFResult<Option<Color>> {
        let Some(fill_cs) = self.fill_color_space.clone() else {
            return Ok(None);
        };
        let PDFObject::Array(arr) = fill_cs else {
            return Ok(None);
        };
        if arr.len() < 2 || !matches!(&*arr[0], PDFObject::Name(name) if name == "Pattern") {
            return Ok(None);
        }

        let base = resolve_object_clone(arr[1].as_ref().clone(), &mut self.resolver);
        self.color_from_current_space(args, base)
    }
    pub(super) fn fill_with_tiling_pattern(
        &mut self,
        rule: FillRule,
        pattern: TilingPatternPaint,
    ) -> PDFResult<()> {
        if self.pattern_depth >= MAX_PATTERN_DEPTH {
            self.current_path.begin();
            self.device.begin_path();
            return Ok(());
        }

        let fill_bbox = self.current_path.bounding_box();
        if self.device.supports_pattern_cell_fill()
            && self.fill_with_tiling_pattern_cell(rule, &pattern, fill_bbox)?
        {
            self.current_path.begin();
            self.device.begin_path();
            return Ok(());
        }

        self.device.save_state();
        self.device.clip_path(rule)?;

        let result = (|| -> PDFResult<()> {
            let ctm = self.current_state().ctm;
            let base = post_concat_matrix(self.root_ctm(), pattern.matrix);
            let bounds =
                pattern_tile_bounds(&pattern, fill_bbox, self.device.page_bounds(), ctm, base);
            trace_tiling_pattern(
                "vector",
                &pattern,
                fill_bbox,
                self.device.page_bounds(),
                ctm,
                base,
                bounds,
            );

            let previous_resources = self.resources.clone();
            let previous_stencil = self.stencil_pattern_color;
            if let Some(resources) = pattern.resources.clone() {
                self.resources = Some(resources);
            }

            if pattern.paint_type == 2 {
                if let Some(color) = pattern.stencil_color {
                    self.current_state_mut().fill_color = color;
                    self.current_state_mut().stroke_color = color;
                    self.stencil_pattern_color = Some(color);
                }
            }

            self.pattern_depth += 1;
            let mut tile_count = 0usize;
            let mut render_result = Ok(());

            'tiles: for ix in bounds.0..=bounds.1 {
                for iy in bounds.2..=bounds.3 {
                    if tile_count >= MAX_PATTERN_TILES {
                        break 'tiles;
                    }
                    tile_count += 1;

                    self.save()?;
                    let mut tile_matrix = post_concat_translate(
                        base,
                        ix as f64 * pattern.x_step,
                        iy as f64 * pattern.y_step,
                    );
                    if pattern_has_overlapping_tiles(&pattern) {
                        // PDFium renders overlapping tiling cells into a cell bitmap and
                        // composites that bitmap at rounded device positions. Replaying
                        // every cell as vector content at fractional positions otherwise
                        // shifts the overlap blend by a pixel on small-step patterns.
                        tile_matrix[4] = tile_matrix[4].round();
                        tile_matrix[5] = tile_matrix[5].round();
                    }
                    self.current_state_mut().set_matrix(&tile_matrix);
                    self.device.set_matrix(&tile_matrix);

                    self.device.begin_path();
                    self.device.move_to(pattern.bbox[0], pattern.bbox[1]);
                    self.device.line_to(pattern.bbox[2], pattern.bbox[1]);
                    self.device.line_to(pattern.bbox[2], pattern.bbox[3]);
                    self.device.line_to(pattern.bbox[0], pattern.bbox[3]);
                    self.device.close_path();
                    self.device.clip_path(FillRule::NonZero)?;

                    render_result = self.process_content_stream_bytes(pattern.data.clone());
                    let restore_result = self.restore();
                    if render_result.is_err() {
                        break 'tiles;
                    }
                    restore_result?;
                }
            }

            self.pattern_depth -= 1;
            self.stencil_pattern_color = previous_stencil;
            self.resources = previous_resources;
            render_result
        })();

        self.device.restore_state();
        self.current_path.begin();
        self.device.begin_path();
        result
    }
    pub(super) fn fill_with_tiling_pattern_cell(
        &mut self,
        rule: FillRule,
        pattern: &TilingPatternPaint,
        fill_bbox: Option<(f64, f64, f64, f64)>,
    ) -> PDFResult<bool> {
        if !pattern_has_overlapping_tiles(pattern) {
            return Ok(false);
        }

        let bbox_w = (pattern.bbox[2] - pattern.bbox[0]).abs();
        let bbox_h = (pattern.bbox[3] - pattern.bbox[1]).abs();
        if bbox_w <= f64::EPSILON || bbox_h <= f64::EPSILON {
            return Ok(false);
        }

        let base = post_concat_matrix(self.root_ctm(), pattern.matrix);
        let x_scale = (base[0] * base[0] + base[1] * base[1]).sqrt();
        let y_scale = (base[2] * base[2] + base[3] * base[3]).sqrt();
        if !x_scale.is_finite() || !y_scale.is_finite() || x_scale <= 0.0 || y_scale <= 0.0 {
            return Ok(false);
        }

        let step_w = pattern.x_step.abs();
        let step_h = pattern.y_step.abs();
        if step_w <= f64::EPSILON || step_h <= f64::EPSILON {
            return Ok(false);
        }

        let pix_w = (x_scale * bbox_w).ceil().clamp(1.0, 3000.0) as u32;
        let pix_h = (y_scale * bbox_h).ceil().clamp(1.0, 3000.0) as u32;
        if pix_w == 0 || pix_h == 0 {
            return Ok(false);
        }
        let render_w = pix_w;
        let render_h = pix_h;

        let saved_path = self.current_path.clone();
        let cell_scale_x = pix_w as f64 / bbox_w;
        let cell_scale_y = pix_h as f64 / bbox_h;
        let scale = [cell_scale_x, 0.0, 0.0, cell_scale_y, 0.0, 0.0];
        let translate = [1.0, 0.0, 0.0, 1.0, -pattern.bbox[0], -pattern.bbox[1]];
        let cell_transform = post_concat_matrix(scale, translate);
        let Some(cell_to_pattern) = invert_matrix(cell_transform) else {
            return Ok(false);
        };
        let image_to_device = post_concat_matrix(base, cell_to_pattern);
        let tile_bounds = pattern_tile_bounds(
            pattern,
            fill_bbox,
            self.device.page_bounds(),
            self.current_state().ctm,
            base,
        );
        trace_tiling_cell(
            pattern,
            base,
            cell_transform,
            image_to_device,
            pix_w,
            pix_h,
            render_w,
            render_h,
        );

        self.device.begin_offscreen()?;
        let previous_resources = self.resources.clone();
        let previous_stencil = self.stencil_pattern_color;
        if let Some(resources) = pattern.resources.clone() {
            self.resources = Some(resources);
        }

        self.pattern_depth += 1;
        let render_result = (|| -> PDFResult<()> {
            self.save()?;
            self.current_state_mut().set_matrix(&cell_transform);
            self.device.set_matrix(&cell_transform);
            let _ = self.device.set_soft_mask(None);
            self.device.set_opacity(1.0);

            if pattern.paint_type == 2 {
                if let Some(color) = pattern.stencil_color {
                    self.current_state_mut().fill_color = color;
                    self.current_state_mut().stroke_color = color;
                    self.stencil_pattern_color = Some(color);
                }
            }

            self.device.begin_path();
            self.device.move_to(pattern.bbox[0], pattern.bbox[1]);
            self.device.line_to(pattern.bbox[2], pattern.bbox[1]);
            self.device.line_to(pattern.bbox[2], pattern.bbox[3]);
            self.device.line_to(pattern.bbox[0], pattern.bbox[3]);
            self.device.close_path();
            self.device.clip_path(FillRule::NonZero)?;

            let result = self.process_content_stream_bytes(pattern.data.clone());
            let restore_result = self.restore();
            result?;
            restore_result
        })();
        self.pattern_depth -= 1;
        self.stencil_pattern_color = previous_stencil;
        self.resources = previous_resources;

        let Some(rendered_cell) = self.device.end_offscreen_as_rgba_region(render_w, render_h)
        else {
            return Ok(false);
        };
        render_result?;

        self.current_path = saved_path.clone();
        self.replay_path_to_device(&saved_path);
        self.device.fill_path_with_pattern_cell(
            rule,
            &rendered_cell,
            pix_w,
            pix_h,
            &image_to_device,
            &base,
            &pattern.bbox,
            pattern.x_step,
            pattern.y_step,
            tile_bounds,
        )
    }
}
