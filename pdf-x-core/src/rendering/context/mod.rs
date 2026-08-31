//! Rendering context for processing PDF content streams.
//!
//! This module provides the RenderingContext, which coordinates:
//! - Graphics state stack (save/restore)
//! - Current path being constructed
//! - Device for rendering operations
//! - Processing of content stream operators

use super::device::Device;
use super::graphics_state::{Color, FillRule, GraphicsState};
use super::path::{Path, PathElement};
use super::{Paint, PathDrawMode};
use crate::core::cmap::CMap;
use crate::core::content_stream::{ContentStreamEvaluator, OpCode, Operation};
use crate::core::encoding::Encoding;
use crate::core::error::{PDFError, PDFResult};
use crate::core::page::PageResolver;
use crate::core::parser::{PDFObject, Parser};
use crate::core::{BaseStream, Lexer, Stream};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod color;
mod font;
mod graphics_state;
mod imaging;
mod interpreter;
mod mesh;
mod path;
mod pattern;
#[cfg(test)]
mod tests;
mod text;
mod type3;
mod xobject;

use color::*;
use font::*;
use graphics_state::*;
use imaging::*;
use interpreter::*;
use mesh::*;
use path::*;
use pattern::*;
use text::*;
use type3::*;
use xobject::*;

use pattern::TilingPatternPaint;
use type3::Type3Font;

pub(super) const MAX_PATTERN_TILES: usize = 4096;

pub(super) const MAX_PATTERN_DEPTH: usize = 8;

/// Runtime toggle for verbose text spacing traces (Tw/Tc/TJ/Tj).
///
/// This is intentionally global so frontends (like Tauri) can enable tracing
/// around a render call without rebuilding with special features.
static TEXT_TRACE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable or disable runtime text tracing.
///
/// Returns the previous value so callers can restore it after a scoped render.
pub fn set_text_trace_enabled(enabled: bool) -> bool {
    TEXT_TRACE_ENABLED.swap(enabled, Ordering::Relaxed)
}

/// Returns whether runtime text tracing is currently enabled.
pub fn is_text_trace_enabled() -> bool {
    TEXT_TRACE_ENABLED.load(Ordering::Relaxed)
}

/// Rendering context for processing PDF content streams.
///
/// The context maintains the graphics state stack, current path, and device
/// for rendering. It processes content stream operations and forwards them
/// to the device.
///
/// This follows the same pattern as PDF.js's CanvasGraphics and hayro's Context.
pub struct RenderingContext<'a, D: Device> {
    /// The device for rendering
    device: &'a mut D,

    /// Graphics state stack
    state_stack: Vec<GraphicsState>,

    /// Root transform stack used to anchor patterns at page/form boundaries.
    root_ctm_stack: Vec<[f64; 6]>,

    /// Current path being constructed
    current_path: Path,

    /// Pending clipping rule set by W/W*. PDF applies it when the current path
    /// is ended or painted, not when the clipping operator is encountered.
    pending_clip: Option<FillRule>,

    /// Clip path stack (for nesting)
    clip_stack: Vec<FillRule>,

    /// Whether we're in a text object (BT...ET)
    in_text_object: bool,

    /// Object resolver for fetching resources (delta-aware when a `DocStore` is supplied).
    resolver: Option<&'a mut dyn PageResolver>,

    /// Active resources dictionary (for looking up XObjects, fonts, etc.).
    ///
    /// Stored as an owned object so Form XObjects can temporarily override it
    /// with their own `/Resources` dictionaries.
    resources: Option<PDFObject>,

    /// Operation counter for debug logging
    operation_count: usize,

    /// Optional /ToUnicode CMaps keyed by active PDF font resource name.
    font_to_unicode: HashMap<String, CMap>,

    /// Whether a font should be remapped through `/ToUnicode` before drawing.
    ///
    /// We only do this for composite/CID-style fonts where raw shown bytes are
    /// not directly drawable by the simple Type1/TrueType code paths.
    font_requires_cmap_remap: HashMap<String, bool>,

    /// Type3 font programs keyed by active PDF font resource name.
    type3_fonts: HashMap<String, Type3Font>,

    /// Paint override used while interpreting uncolored Type3 shape glyphs.
    type3_shape_color: Option<Color>,

    /// Current nonstroking color space selected by `cs`.
    fill_color_space: Option<PDFObject>,

    /// Current stroking color space selected by `CS`.
    stroke_color_space: Option<PDFObject>,

    /// Color-space graphics state stack parallel to `state_stack`.
    color_space_stack: Vec<(Option<PDFObject>, Option<PDFObject>)>,

    /// Pending fill shading (set by `scn` with a Pattern color space).
    /// Sticky until the fill colour is replaced; also saved/restored by `q`/`Q`
    /// because pattern colour is part of the graphics state (PDF §8.4.1).
    pending_fill_shading: Option<crate::rendering::shading::Shading>,

    /// Pending fill tiling pattern (set by `scn` with a Pattern color space).
    /// Same stickiness as [`Self::pending_fill_shading`].
    pending_fill_tiling: Option<TilingPatternPaint>,

    /// Pending stroke shading (set by `SCN` with a Pattern color space).
    pending_stroke_shading: Option<crate::rendering::shading::Shading>,

    /// Pattern-paint stack parallel to `state_stack` (`q`/`Q`).
    pattern_paint_stack: Vec<(
        Option<crate::rendering::shading::Shading>,
        Option<TilingPatternPaint>,
        Option<crate::rendering::shading::Shading>,
    )>,

    /// Soft mask group reference stack (prevents only true self-recursion).
    soft_mask_group_stack: Vec<(u32, u32)>,

    /// Soft mask rendering depth; Form transparency groups inside mask rendering
    /// are rendered as mask content, not composited as page groups.
    soft_mask_render_depth: usize,

    /// Pattern stream recursion depth guard.
    pattern_depth: usize,

    /// Form XObject nesting depth (C.3 render budget; self- or mutually
    /// recursive forms must not overflow the stack). Mirrors
    /// `Page::FORM_XOBJECT_MAX_DEPTH` on the legacy `Page::render` path.
    form_render_depth: usize,

    /// Replacement paint for uncolored tiling pattern streams.
    stencil_pattern_color: Option<Color>,
}

/// Helper function to extract a number from a PDFObject.
pub(super) fn extract_number(
    args: &[crate::core::parser::PDFObject],
    index: usize,
) -> PDFResult<f64> {
    if index >= args.len() {
        return Err(PDFError::content_stream_error(
            "Not enough arguments".to_string(),
        ));
    }

    match &args[index] {
        crate::core::parser::PDFObject::Number(n) => Ok(*n),
        _ => Err(PDFError::content_stream_error(
            "Expected number argument".to_string(),
        )),
    }
}

pub(super) fn parse_shading(
    dict: &HashMap<String, PDFObject>,
    stream_data: Option<&[u8]>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<crate::rendering::shading::Shading> {
    use super::shading::ShadingKind;

    let shading_type = dict
        .get("ShadingType")
        .and_then(|v| v.as_number())
        .unwrap_or(0.0) as i32;

    let color_space = dict
        .get("ColorSpace")
        .and_then(|cs| parse_shading_color_space(cs, resolver))
        .unwrap_or(crate::rendering::shading::ShadingColorSpace::DeviceRGB);
    let n_components = color_space.n_components();

    let function = dict
        .get("Function")
        .map(|f| parse_shading_function(f, resolver));

    let bbox = parse_four_number_array(dict.get("BBox"));
    let matrix =
        parse_six_number_array(dict.get("Matrix")).unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    let kind = match shading_type {
        1 => {
            let function = function?;
            let domain =
                parse_four_number_array(dict.get("Domain")).unwrap_or([0.0, 1.0, 0.0, 1.0]);
            ShadingKind::FunctionBased { domain, function }
        }
        2 | 3 => {
            let function = function?;
            let coords = number_array(dict.get("Coords"))?;
            let domain_arr = number_array(dict.get("Domain")).unwrap_or_else(|| vec![0.0, 1.0]);
            let domain = [
                domain_arr.first().copied().unwrap_or(0.0),
                domain_arr.get(1).copied().unwrap_or(1.0),
            ];
            let extend = bool_pair_array(dict.get("Extend")).unwrap_or([false, false]);

            if shading_type == 2 {
                if coords.len() < 4 {
                    return None;
                }
                ShadingKind::Axial {
                    x0: coords[0],
                    y0: coords[1],
                    x1: coords[2],
                    y1: coords[3],
                    t0: domain[0],
                    t1: domain[1],
                    function,
                    extend_start: extend[0],
                    extend_end: extend[1],
                }
            } else {
                if coords.len() < 6 {
                    return None;
                }
                ShadingKind::Radial {
                    x0: coords[0],
                    y0: coords[1],
                    r0: coords[2],
                    x1: coords[3],
                    y1: coords[4],
                    r1: coords[5],
                    t0: domain[0],
                    t1: domain[1],
                    function,
                    extend_start: extend[0],
                    extend_end: extend[1],
                }
            }
        }
        4 => {
            let data = stream_data?;
            let triangles = parse_type4_triangle_mesh(dict, data, function.as_ref().is_some())?;
            ShadingKind::TriangleMesh {
                triangles,
                function,
            }
        }
        6 | 7 => {
            let data = stream_data?;
            let triangles =
                parse_patch_triangle_mesh(dict, data, shading_type, function.as_ref().is_some())?;
            ShadingKind::TriangleMesh {
                triangles,
                function,
            }
        }
        _ => return None,
    };

    Some(crate::rendering::shading::Shading {
        kind,
        n_components,
        color_space,
        bbox,
        matrix,
        background: number_array(dict.get("Background")),
        use_root_ctm: false,
    })
}

pub(super) fn parse_shading_color_space(
    obj: &PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<crate::rendering::shading::ShadingColorSpace> {
    use super::shading::ShadingColorSpace;

    let resolved = resolve_object_clone(obj.clone(), resolver).unwrap_or_else(|| obj.clone());
    match resolved {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => Some(ShadingColorSpace::DeviceGray),
            "DeviceRGB" | "RGB" => Some(ShadingColorSpace::DeviceRGB),
            "DeviceCMYK" | "CMYK" => Some(ShadingColorSpace::DeviceCMYK),
            _ => None,
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "CalRGB" && arr.len() >= 2 => {
                let dict = match resolve_object_clone(arr[1].as_ref().clone(), resolver)
                    .unwrap_or_else(|| arr[1].as_ref().clone())
                {
                    PDFObject::Dictionary(dict) => dict,
                    _ => return None,
                };
                Some(ShadingColorSpace::CalRGB {
                    white_point: parse_three_number_array(dict.get("WhitePoint"))
                        .unwrap_or([1.0, 1.0, 1.0]),
                    black_point: parse_three_number_array(dict.get("BlackPoint"))
                        .unwrap_or([0.0, 0.0, 0.0]),
                    gamma: parse_three_number_array(dict.get("Gamma")).unwrap_or([1.0, 1.0, 1.0]),
                    matrix: parse_nine_number_array(dict.get("Matrix"))
                        .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
                })
            }
            Some(PDFObject::Name(name)) if name == "Separation" && arr.len() >= 4 => {
                let base = parse_shading_color_space(arr[2].as_ref(), resolver)?;
                let tint_function = parse_shading_function(arr[3].as_ref(), resolver);
                Some(ShadingColorSpace::Alternate {
                    n_components: 1,
                    base: Box::new(base),
                    tint_function,
                })
            }
            Some(PDFObject::Name(name)) if name == "DeviceN" && arr.len() >= 4 => {
                let n_components = match resolve_object_clone(arr[1].as_ref().clone(), resolver)
                    .unwrap_or_else(|| arr[1].as_ref().clone())
                {
                    PDFObject::Array(names) => names.len(),
                    _ => return None,
                };
                if n_components == 0 {
                    return None;
                }
                let base = parse_shading_color_space(arr[2].as_ref(), resolver)?;
                let tint_function = parse_shading_function(arr[3].as_ref(), resolver);
                Some(ShadingColorSpace::Alternate {
                    n_components,
                    base: Box::new(base),
                    tint_function,
                })
            }
            // ICCBased shadings: map by component count. Full ICC transform is
            // not required for the common Gray/RGB/CMYK profile cases used by
            // Type3 soft-mask + pattern fixtures (hayro/mutool agree on that path).
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                let profile = resolve_object_clone(arr[1].as_ref().clone(), resolver)
                    .unwrap_or_else(|| arr[1].as_ref().clone());
                let n = match profile {
                    PDFObject::Stream { dict, .. } => dict
                        .get("N")
                        .and_then(|v| v.as_number())
                        .map(|n| n as i32)
                        .unwrap_or(3),
                    PDFObject::Dictionary(dict) => dict
                        .get("N")
                        .and_then(|v| v.as_number())
                        .map(|n| n as i32)
                        .unwrap_or(3),
                    _ => 3,
                };
                match n {
                    1 => Some(ShadingColorSpace::DeviceGray),
                    4 => Some(ShadingColorSpace::DeviceCMYK),
                    _ => Some(ShadingColorSpace::DeviceRGB),
                }
            }
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn parse_type4_triangle_mesh(
    dict: &HashMap<String, PDFObject>,
    data: &[u8],
    has_function: bool,
) -> Option<Vec<crate::rendering::shading::ShadingTriangle>> {
    use super::shading::{ShadingTriangle, ShadingVertex};

    let bits_per_coordinate = dict.get("BitsPerCoordinate").and_then(|v| v.as_number())? as usize;
    let bits_per_component = dict.get("BitsPerComponent").and_then(|v| v.as_number())? as usize;
    let bits_per_flag = dict.get("BitsPerFlag").and_then(|v| v.as_number())? as usize;
    if bits_per_coordinate == 0
        || bits_per_coordinate > 32
        || bits_per_component == 0
        || bits_per_component > 32
        || bits_per_flag == 0
        || bits_per_flag > 8
    {
        return None;
    }

    let decode = number_array(dict.get("Decode"))?;
    if decode.len() < 6 {
        return None;
    }
    let x_min = decode[0];
    let x_max = decode[1];
    let y_min = decode[2];
    let y_max = decode[3];
    let component_decode = &decode[4..];
    let component_count = if has_function {
        1
    } else {
        component_decode.len() / 2
    };
    if component_count == 0 || component_decode.len() < component_count * 2 {
        return None;
    }

    let decoded = dict
        .get("Filter")
        .and_then(|filter| crate::core::decode::apply_filters(data, filter).ok())
        .unwrap_or_else(|| data.to_vec());
    let mut reader = MeshBitReader::new(&decoded);
    let coord_max = sample_max(bits_per_coordinate)?;
    let component_max = sample_max(bits_per_component)?;

    let mut a: Option<ShadingVertex> = None;
    let mut b: Option<ShadingVertex> = None;
    let mut c: Option<ShadingVertex> = None;
    let mut triangles = Vec::new();

    while reader.remaining_bits() >= bits_per_flag + bits_per_coordinate * 2 {
        let flag = reader.read(bits_per_flag)?;
        if flag > 2 {
            return None;
        }
        let x = interpolate_number(
            reader.read(bits_per_coordinate)? as f64,
            0.0,
            coord_max,
            x_min,
            x_max,
        );
        let y = interpolate_number(
            reader.read(bits_per_coordinate)? as f64,
            0.0,
            coord_max,
            y_min,
            y_max,
        );
        let mut components = Vec::with_capacity(component_count);
        for i in 0..component_count {
            let d0 = component_decode[i * 2];
            let d1 = component_decode[i * 2 + 1];
            components.push(interpolate_number(
                reader.read(bits_per_component)? as f64,
                0.0,
                component_max,
                d0,
                d1,
            ));
        }
        reader.align_byte();

        let vertex = ShadingVertex { x, y, components };
        match flag {
            0 => {
                let second = read_type4_vertex(
                    &mut reader,
                    bits_per_flag,
                    bits_per_coordinate,
                    bits_per_component,
                    component_count,
                    coord_max,
                    component_max,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                    component_decode,
                )?;
                let third = read_type4_vertex(
                    &mut reader,
                    bits_per_flag,
                    bits_per_coordinate,
                    bits_per_component,
                    component_count,
                    coord_max,
                    component_max,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                    component_decode,
                )?;
                a = Some(vertex);
                b = Some(second);
                c = Some(third);
            }
            1 => {
                a = b.clone();
                b = c.clone();
                c = Some(vertex);
            }
            2 => {
                b = c.clone();
                c = Some(vertex);
            }
            _ => unreachable!(),
        }

        let (p0, p1, p2) = (a.clone()?, b.clone()?, c.clone()?);
        if points_nearly_same(&p0, &p1) || points_nearly_same(&p1, &p2) {
            continue;
        }
        triangles.push(ShadingTriangle { p0, p1, p2 });
    }

    Some(triangles)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn read_type4_vertex(
    reader: &mut MeshBitReader<'_>,
    bits_per_flag: usize,
    bits_per_coordinate: usize,
    bits_per_component: usize,
    component_count: usize,
    coord_max: f64,
    component_max: f64,
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
    component_decode: &[f64],
) -> Option<crate::rendering::shading::ShadingVertex> {
    let _flag = reader.read(bits_per_flag)?;
    let x = interpolate_number(
        reader.read(bits_per_coordinate)? as f64,
        0.0,
        coord_max,
        x_min,
        x_max,
    );
    let y = interpolate_number(
        reader.read(bits_per_coordinate)? as f64,
        0.0,
        coord_max,
        y_min,
        y_max,
    );
    let mut components = Vec::with_capacity(component_count);
    for i in 0..component_count {
        components.push(interpolate_number(
            reader.read(bits_per_component)? as f64,
            0.0,
            component_max,
            component_decode[i * 2],
            component_decode[i * 2 + 1],
        ));
    }
    reader.align_byte();
    Some(crate::rendering::shading::ShadingVertex { x, y, components })
}

pub(super) fn points_nearly_same(
    a: &crate::rendering::shading::ShadingVertex,
    b: &crate::rendering::shading::ShadingVertex,
) -> bool {
    (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
}

pub(super) fn parse_patch_triangle_mesh(
    dict: &HashMap<String, PDFObject>,
    data: &[u8],
    shading_type: i32,
    has_function: bool,
) -> Option<Vec<crate::rendering::shading::ShadingTriangle>> {
    let bits_per_coordinate = dict.get("BitsPerCoordinate").and_then(|v| v.as_number())? as usize;
    let bits_per_component = dict.get("BitsPerComponent").and_then(|v| v.as_number())? as usize;
    let bits_per_flag = dict.get("BitsPerFlag").and_then(|v| v.as_number())? as usize;
    if bits_per_coordinate == 0
        || bits_per_coordinate > 32
        || bits_per_component == 0
        || bits_per_component > 32
        || bits_per_flag == 0
        || bits_per_flag > 8
    {
        return None;
    }

    let decode = number_array(dict.get("Decode"))?;
    if decode.len() < 6 {
        return None;
    }
    let x_min = decode[0];
    let x_max = decode[1];
    let y_min = decode[2];
    let y_max = decode[3];
    let component_decode = &decode[4..];
    let component_count = if has_function {
        1
    } else {
        component_decode.len() / 2
    };
    if component_count == 0 || component_decode.len() < component_count * 2 {
        return None;
    }

    let decoded = dict
        .get("Filter")
        .and_then(|filter| crate::core::decode::apply_filters(data, filter).ok())
        .unwrap_or_else(|| data.to_vec());
    let mut reader = MeshBitReader::new(&decoded);
    let coord_max = sample_max(bits_per_coordinate)?;
    let component_max = sample_max(bits_per_component)?;
    let control_points_count = if shading_type == 7 { 16 } else { 12 };
    let mut prev_points: Option<Vec<PatchPoint>> = None;
    let mut prev_colors: Option<[Vec<f64>; 4]> = None;
    let mut triangles = Vec::new();

    while reader.remaining_bits() >= bits_per_flag {
        let flag = reader.read(bits_per_flag)?;
        if flag > 3 {
            break;
        }

        let mut control_points = vec![PatchPoint::default(); 16];
        let mut colors: [Vec<f64>; 4] = Default::default();

        if flag == 0 {
            for point in control_points.iter_mut().take(control_points_count) {
                *point = read_patch_point(
                    &mut reader,
                    bits_per_coordinate,
                    coord_max,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                )?;
            }
            for color in &mut colors {
                *color = read_patch_color(
                    &mut reader,
                    bits_per_component,
                    component_count,
                    component_max,
                    component_decode,
                )?;
            }
        } else {
            let prev_points_ref = prev_points.as_ref()?;
            let prev_colors_ref = prev_colors.as_ref()?;
            copy_patch_control_points(flag, prev_points_ref, &mut control_points);
            match flag {
                1 => {
                    colors[0] = prev_colors_ref[1].clone();
                    colors[1] = prev_colors_ref[2].clone();
                }
                2 => {
                    colors[0] = prev_colors_ref[2].clone();
                    colors[1] = prev_colors_ref[3].clone();
                }
                3 => {
                    colors[0] = prev_colors_ref[3].clone();
                    colors[1] = prev_colors_ref[0].clone();
                }
                _ => unreachable!(),
            }
            for point in control_points.iter_mut().take(control_points_count).skip(4) {
                *point = read_patch_point(
                    &mut reader,
                    bits_per_coordinate,
                    coord_max,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                )?;
            }
            colors[2] = read_patch_color(
                &mut reader,
                bits_per_component,
                component_count,
                component_max,
                component_decode,
            )?;
            colors[3] = read_patch_color(
                &mut reader,
                bits_per_component,
                component_count,
                component_max,
                component_decode,
            )?;
        }

        prev_points = Some(control_points.clone());
        prev_colors = Some(colors.clone());
        if shading_type == 7 {
            append_tensor_patch_triangles(&control_points, &colors, &mut triangles);
        } else {
            append_coons_patch_triangles(&control_points, &colors, &mut triangles);
        }
    }

    Some(triangles)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn read_patch_point(
    reader: &mut MeshBitReader<'_>,
    bits_per_coordinate: usize,
    coord_max: f64,
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
) -> Option<PatchPoint> {
    Some(PatchPoint {
        x: interpolate_number(
            reader.read(bits_per_coordinate)? as f64,
            0.0,
            coord_max,
            x_min,
            x_max,
        ),
        y: interpolate_number(
            reader.read(bits_per_coordinate)? as f64,
            0.0,
            coord_max,
            y_min,
            y_max,
        ),
    })
}

pub(super) fn read_patch_color(
    reader: &mut MeshBitReader<'_>,
    bits_per_component: usize,
    component_count: usize,
    component_max: f64,
    component_decode: &[f64],
) -> Option<Vec<f64>> {
    let mut color = Vec::with_capacity(component_count);
    for i in 0..component_count {
        color.push(interpolate_number(
            reader.read(bits_per_component)? as f64,
            0.0,
            component_max,
            component_decode[i * 2],
            component_decode[i * 2 + 1],
        ));
    }
    Some(color)
}

pub(super) fn append_coons_patch_triangles(
    control_points: &[PatchPoint],
    colors: &[Vec<f64>; 4],
    triangles: &mut Vec<crate::rendering::shading::ShadingTriangle>,
) {
    generate_patch_triangles(
        |u, v| map_coons_patch(control_points, u, v),
        |u, v| interpolate_patch_color(colors, u, v),
        triangles,
    );
}

pub(super) fn append_tensor_patch_triangles(
    control_points: &[PatchPoint],
    colors: &[Vec<f64>; 4],
    triangles: &mut Vec<crate::rendering::shading::ShadingTriangle>,
) {
    generate_patch_triangles(
        |u, v| map_tensor_patch(control_points, u, v),
        |u, v| interpolate_patch_color(colors, u, v),
        triangles,
    );
}

pub(super) fn generate_patch_triangles<M, C>(
    map_coordinate: M,
    interpolate_color: C,
    triangles: &mut Vec<crate::rendering::shading::ShadingTriangle>,
) where
    M: Fn(f64, f64) -> PatchPoint,
    C: Fn(f64, f64) -> Vec<f64>,
{
    use super::shading::{ShadingTriangle, ShadingVertex};
    const GRID_SIZE: usize = 20;

    for i in 0..(GRID_SIZE - 1) {
        for j in 0..(GRID_SIZE - 1) {
            let u0 = i as f64 / (GRID_SIZE - 1) as f64;
            let u1 = (i + 1) as f64 / (GRID_SIZE - 1) as f64;
            let v0 = j as f64 / (GRID_SIZE - 1) as f64;
            let v1 = (j + 1) as f64 / (GRID_SIZE - 1) as f64;

            let p00 = map_coordinate(u0, v0);
            let p10 = map_coordinate(u1, v0);
            let p01 = map_coordinate(u0, v1);
            let p11 = map_coordinate(u1, v1);

            let v00 = ShadingVertex {
                x: p00.x,
                y: p00.y,
                components: interpolate_color(u0, v0),
            };
            let v10 = ShadingVertex {
                x: p10.x,
                y: p10.y,
                components: interpolate_color(u1, v0),
            };
            let v01 = ShadingVertex {
                x: p01.x,
                y: p01.y,
                components: interpolate_color(u0, v1),
            };
            let v11 = ShadingVertex {
                x: p11.x,
                y: p11.y,
                components: interpolate_color(u1, v1),
            };

            triangles.push(ShadingTriangle {
                p0: v00.clone(),
                p1: v10.clone(),
                p2: v01.clone(),
            });
            triangles.push(ShadingTriangle {
                p0: v10,
                p1: v11,
                p2: v01,
            });
        }
    }
}

#[cfg(feature = "debug-logging")]
pub(super) fn pdfium_like_tile_bounds(
    pattern: &TilingPatternPaint,
    clip: (f64, f64, f64, f64),
    base: [f64; 6],
) -> Option<(i32, i32, i32, i32)> {
    let inv = invert_matrix(base)?;
    let clip_p = transform_bbox(clip, inv);
    if pattern.x_step.abs() <= f64::EPSILON || pattern.y_step.abs() <= f64::EPSILON {
        return None;
    }

    let min_col = ((clip_p.0 - pattern.bbox[2]) / pattern.x_step).ceil() as i32;
    let max_col = ((clip_p.2 - pattern.bbox[0]) / pattern.x_step).floor() as i32;
    let min_row = ((clip_p.1 - pattern.bbox[3]) / pattern.y_step).ceil() as i32;
    let max_row = ((clip_p.3 - pattern.bbox[1]) / pattern.y_step).floor() as i32;
    Some((min_col, max_col, min_row, max_row))
}

pub(super) fn tile_range_for_axis(
    area_min: f64,
    area_max: f64,
    tile_min: f64,
    tile_max: f64,
    step: f64,
) -> Option<(i32, i32)> {
    if step.abs() <= f64::EPSILON {
        return None;
    }
    let s = step.abs();
    let lo = ((area_min - tile_max) / s).ceil() as i32;
    let hi = ((area_max - tile_min) / s).floor() as i32;
    Some((lo, hi))
}

pub(super) fn transform_bbox(bbox: (f64, f64, f64, f64), matrix: [f64; 6]) -> (f64, f64, f64, f64) {
    let corners = [
        transform_point(matrix, bbox.0, bbox.1),
        transform_point(matrix, bbox.2, bbox.1),
        transform_point(matrix, bbox.2, bbox.3),
        transform_point(matrix, bbox.0, bbox.3),
    ];

    corners.iter().fold(
        (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
        |(min_x, min_y, max_x, max_y), (x, y)| {
            (min_x.min(*x), min_y.min(*y), max_x.max(*x), max_y.max(*y))
        },
    )
}

pub(super) fn transform_point(matrix: [f64; 6], x: f64, y: f64) -> (f64, f64) {
    (
        matrix[0] * x + matrix[2] * y + matrix[4],
        matrix[1] * x + matrix[3] * y + matrix[5],
    )
}

pub(super) fn invert_matrix(matrix: [f64; 6]) -> Option<[f64; 6]> {
    let [a, b, c, d, e, f] = matrix;
    let det = a * d - b * c;
    if det.abs() <= 1e-12 {
        return None;
    }

    let inv_det = 1.0 / det;
    Some([
        d * inv_det,
        -b * inv_det,
        -c * inv_det,
        a * inv_det,
        (c * f - d * e) * inv_det,
        (b * e - a * f) * inv_det,
    ])
}

pub(super) fn parse_blend_mode(
    obj: &PDFObject,
) -> Option<crate::rendering::graphics_state::BlendMode> {
    use super::graphics_state::BlendMode;

    let name = match obj {
        PDFObject::Name(name) => Some(name.as_str()),
        PDFObject::Array(items) => items.iter().find_map(|item| match item.as_ref() {
            PDFObject::Name(name) => Some(name.as_str()),
            _ => None,
        }),
        _ => None,
    }?;

    match name {
        "Normal" | "Compatible" => Some(BlendMode::Normal),
        "Multiply" => Some(BlendMode::Multiply),
        "Screen" => Some(BlendMode::Screen),
        "Overlay" => Some(BlendMode::Overlay),
        "Darken" => Some(BlendMode::Darken),
        "Lighten" => Some(BlendMode::Lighten),
        "ColorDodge" => Some(BlendMode::ColorDodge),
        "ColorBurn" => Some(BlendMode::ColorBurn),
        "HardLight" => Some(BlendMode::HardLight),
        "SoftLight" => Some(BlendMode::SoftLight),
        "Difference" => Some(BlendMode::Difference),
        "Exclusion" => Some(BlendMode::Exclusion),
        "Hue" => Some(BlendMode::Hue),
        "Saturation" => Some(BlendMode::Saturation),
        "Color" => Some(BlendMode::Color),
        "Luminosity" => Some(BlendMode::Luminosity),
        _ => None,
    }
}

pub(super) fn font_needs_to_unicode_remap(font_dict_info: &crate::core::font::FontDict) -> bool {
    use crate::core::font::FontType;

    if matches!(
        font_dict_info.font_type,
        FontType::CIDFontType0 | FontType::CIDFontType2
    ) {
        return true;
    }

    if font_dict_info.descendant_fonts.is_some() {
        return true;
    }

    matches!(
        font_dict_info.encoding.as_ref(),
        Some(PDFObject::Name(name)) if matches!(name.as_str(), "Identity-H" | "Identity-V")
    )
}

pub(super) fn font_encoding_is_identity_v(font_dict_info: &crate::core::font::FontDict) -> bool {
    matches!(
        font_dict_info.encoding.as_ref(),
        Some(PDFObject::Name(name)) if name == "Identity-V"
    )
}

pub(super) fn font_uses_identity_cid_to_gid(
    font_obj: &PDFObject,
    font_dict_info: &crate::core::font::FontDict,
    resolver: &mut dyn PageResolver,
) -> bool {
    use crate::core::font::FontType;

    let has_identity_encoding = matches!(
        font_dict_info.encoding.as_ref(),
        Some(PDFObject::Name(name)) if matches!(name.as_str(), "Identity-H" | "Identity-V")
    );
    if !has_identity_encoding {
        return false;
    }

    let Some(font_dict) = font_obj.as_dictionary() else {
        return false;
    };

    if matches!(font_dict_info.font_type, FontType::CIDFontType2) {
        // PDF 32000-1:2008 §9.7.4.2 Table 117: CIDToGIDMap default is Identity
        // when the entry is absent. Only a stream map is non-identity.
        return match font_dict.get("CIDToGIDMap") {
            None => true,
            Some(PDFObject::Name(name)) if name == "Identity" => true,
            _ => false,
        };
    }

    if !matches!(font_dict_info.font_type, FontType::Type0) {
        return false;
    }

    // `/DescendantFonts` is frequently an indirect array ref (`6 0 R` →
    // `[7 0 R]`), as on tile_clamp_bug.pdf Type0 fonts. Resolve before Array.
    let Some(descendant_fonts) = font_dict.get("DescendantFonts") else {
        return false;
    };
    let mut resolver_opt = Some(resolver);
    let Some(PDFObject::Array(descendants)) =
        resolve_object_clone(descendant_fonts.clone(), &mut resolver_opt)
    else {
        return false;
    };
    let Some(first_descendant) = descendants.first() else {
        return false;
    };
    let Some(PDFObject::Dictionary(descendant)) =
        resolve_object_clone(first_descendant.as_ref().clone(), &mut resolver_opt)
    else {
        return false;
    };

    let subtype = match descendant.get("Subtype") {
        Some(PDFObject::Name(s)) => s.as_str(),
        _ => return false,
    };

    // Identity-H/V Type0 always carries multi-byte CIDs in the content stream
    // (PDF §9.7.6.2). Enable raw-CID decoding for both CFF (CIDFontType0) and
    // TrueType (CIDFontType2) descendants — otherwise UTF-16BE CIDs are misread
    // as single bytes (nulls + ASCII), mangling headlines like BentonSansComp
    // Black on tile_clamp_bug.pdf.
    //
    // CIDToGIDMap applies only to CIDFontType2 (PDF §9.7.4.2 Table 117); when
    // omitted the default is Identity. Type0 CFF maps CIDs via the font program
    // charset (`glyph_index_by_cid` / Identity gid).
    match subtype {
        "CIDFontType0" => true,
        "CIDFontType2" => match descendant.get("CIDToGIDMap") {
            None => true,
            Some(PDFObject::Name(name)) if name == "Identity" => true,
            // Stream map is non-identity but text is still multi-byte CIDs.
            Some(PDFObject::Stream { .. }) | Some(PDFObject::Ref(_)) => true,
            _ => false,
        },
        _ => false,
    }
}

/// Returns raw text bytes for PDF text string objects.
///
/// PDF content streams represent text with both literal strings `( ... )`
/// and hexadecimal strings `< ... >`. Both should be rendered as byte sequences.
pub(super) fn text_bytes(obj: &PDFObject) -> Option<&[u8]> {
    match obj {
        PDFObject::String(bytes) | PDFObject::HexString(bytes) => Some(bytes.as_slice()),
        _ => None,
    }
}

pub(super) fn build_type3_font(
    font_obj: &Arc<PDFObject>,
    font_dict_info: &crate::core::font::FontDict,
    resolver: &mut dyn PageResolver,
) -> Option<Type3Font> {
    let dict = match &**font_obj {
        PDFObject::Dictionary(d) => d,
        _ => return None,
    };

    let char_procs_obj = resolver.resolve_if_ref(dict.get("CharProcs")?).ok()?;
    let char_procs_dict = match &*char_procs_obj {
        PDFObject::Dictionary(d) => d,
        _ => return None,
    };

    let mut char_procs = HashMap::new();
    for (name, proc_ref) in char_procs_dict {
        let proc_obj = resolver.resolve_if_ref(proc_ref).ok()?;
        if let PDFObject::Stream { dict, data } = &*proc_obj {
            let decoded = if let Some(filter) = dict.get("Filter") {
                crate::core::decode::apply_filters(data, filter).unwrap_or_else(|_| data.clone())
            } else {
                data.clone()
            };
            let is_shape_glyph = type3_is_shape_glyph(&decoded);
            char_procs.insert(
                name.clone(),
                Type3Glyph {
                    data: decoded,
                    is_shape_glyph,
                    resources: dict.get("Resources").cloned(),
                },
            );
        }
    }

    if char_procs.is_empty() {
        return None;
    }

    Some(Type3Font {
        encoding: build_type3_encoding(font_dict_info.encoding.as_ref(), resolver),
        char_procs,
        resources: dict.get("Resources").cloned(),
        font_matrix: parse_six_number_array(dict.get("FontMatrix"))
            .unwrap_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]),
        first_char: font_dict_info.first_char.unwrap_or(0),
        widths: font_dict_info.widths.clone().unwrap_or_default(),
        default_width: font_dict_info.default_width,
    })
}

/// Classify a Type3 CharProc as an uncolored *shape* glyph (`d0`) vs a colored
/// glyph (`d1`). Shape glyphs paint with the current text colour; coloured
/// glyphs supply their own paints (often ImageMasks with `/Decode`).
///
/// PDF 1.7 §9.6.5: `d0` = wx wy d0 (shape), `d1` = wx wy llx lly urx ury d1
/// (coloured). Matches hayro `Type3::render_glyph` ShapeGlyph vs ColorGlyph.
///
/// Source: hayro/hayro-interpret/src/font/type3.rs
pub(super) fn type3_is_shape_glyph(data: &[u8]) -> bool {
    let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
    let Ok(lexer) = Lexer::new(stream) else {
        return false;
    };
    let Ok(parser) = Parser::new(lexer) else {
        return false;
    };
    let mut evaluator = ContentStreamEvaluator::new(parser);

    while let Ok(Some(op)) = evaluator.read_operation() {
        match op.op {
            // d0 — uncolored shape glyph
            OpCode::SetCharWidth => return true,
            // d1 — coloured glyph (own colour / image mask)
            OpCode::SetCharWidthAndBounds => return false,
            _ => {}
        }
    }

    false
}

pub(super) fn build_type3_encoding(
    encoding_obj: Option<&PDFObject>,
    resolver: &mut dyn PageResolver,
) -> Type3Encoding {
    let Some(encoding_obj) = encoding_obj else {
        return Type3Encoding::Names(type3_base_encoding_names("StandardEncoding"));
    };

    let resolved = resolver
        .resolve_if_ref(encoding_obj)
        .unwrap_or_else(|_| Arc::new(encoding_obj.clone()));

    if let Some(names) = type3_difference_names(&resolved) {
        Type3Encoding::Names(names)
    } else {
        Type3Encoding::Text(Encoding::from_pdf_object(&resolved).unwrap_or(Encoding::Standard))
    }
}

pub(super) fn type3_difference_names(encoding_obj: &PDFObject) -> Option<Vec<Option<String>>> {
    let PDFObject::Dictionary(dict) = encoding_obj else {
        return None;
    };

    let base_name = match dict.get("BaseEncoding") {
        Some(PDFObject::Name(name)) => name.as_str(),
        _ => "StandardEncoding",
    };
    let mut names = type3_base_encoding_names(base_name);

    let PDFObject::Array(differences) = dict.get("Differences")? else {
        return None;
    };

    let mut current = 0usize;
    for item in differences {
        match item.as_ref() {
            PDFObject::Number(n) => current = (*n as usize).min(255),
            PDFObject::Name(name) => {
                if current < names.len() {
                    names[current] = Some(name.clone());
                }
                current = (current + 1).min(256);
            }
            _ => {}
        }
    }

    Some(names)
}

pub(super) fn type3_base_encoding_names(base_name: &str) -> Vec<Option<String>> {
    let mut names = vec![None; 256];
    for code in 0x20u8..=0x7e {
        names[code as usize] = type3_ascii_glyph_name(code).map(str::to_string);
    }

    if matches!(base_name, "WinAnsiEncoding" | "MacRomanEncoding") {
        const WIN_ANSI_NAMES: &[(u8, &str)] = &[
            (0x80, "Euro"),
            (0x82, "quotesinglbase"),
            (0x83, "florin"),
            (0x84, "quotedblbase"),
            (0x85, "ellipsis"),
            (0x86, "dagger"),
            (0x87, "daggerdbl"),
            (0x88, "circumflex"),
            (0x89, "perthousand"),
            (0x8a, "Scaron"),
            (0x8b, "guilsinglleft"),
            (0x8c, "OE"),
            (0x8e, "Zcaron"),
            (0x91, "quoteleft"),
            (0x92, "quoteright"),
            (0x93, "quotedblleft"),
            (0x94, "quotedblright"),
            (0x95, "bullet"),
            (0x96, "endash"),
            (0x97, "emdash"),
            (0x98, "tilde"),
            (0x99, "trademark"),
            (0x9a, "scaron"),
            (0x9b, "guilsinglright"),
            (0x9c, "oe"),
            (0x9e, "zcaron"),
            (0x9f, "Ydieresis"),
        ];
        for &(code, name) in WIN_ANSI_NAMES {
            names[code as usize] = Some(name.to_string());
        }
    }

    names
}

pub(super) fn type3_ascii_glyph_name(code: u8) -> Option<&'static str> {
    Some(match code {
        b' ' => "space",
        b'!' => "exclam",
        b'"' => "quotedbl",
        b'#' => "numbersign",
        b'$' => "dollar",
        b'%' => "percent",
        b'&' => "ampersand",
        b'\'' => "quotesingle",
        b'(' => "parenleft",
        b')' => "parenright",
        b'*' => "asterisk",
        b'+' => "plus",
        b',' => "comma",
        b'-' => "hyphen",
        b'.' => "period",
        b'/' => "slash",
        b'0' => "zero",
        b'1' => "one",
        b'2' => "two",
        b'3' => "three",
        b'4' => "four",
        b'5' => "five",
        b'6' => "six",
        b'7' => "seven",
        b'8' => "eight",
        b'9' => "nine",
        b':' => "colon",
        b';' => "semicolon",
        b'<' => "less",
        b'=' => "equal",
        b'>' => "greater",
        b'?' => "question",
        b'@' => "at",
        b'A' => "A",
        b'B' => "B",
        b'C' => "C",
        b'D' => "D",
        b'E' => "E",
        b'F' => "F",
        b'G' => "G",
        b'H' => "H",
        b'I' => "I",
        b'J' => "J",
        b'K' => "K",
        b'L' => "L",
        b'M' => "M",
        b'N' => "N",
        b'O' => "O",
        b'P' => "P",
        b'Q' => "Q",
        b'R' => "R",
        b'S' => "S",
        b'T' => "T",
        b'U' => "U",
        b'V' => "V",
        b'W' => "W",
        b'X' => "X",
        b'Y' => "Y",
        b'Z' => "Z",
        b'a' => "a",
        b'b' => "b",
        b'c' => "c",
        b'd' => "d",
        b'e' => "e",
        b'f' => "f",
        b'g' => "g",
        b'h' => "h",
        b'i' => "i",
        b'j' => "j",
        b'k' => "k",
        b'l' => "l",
        b'm' => "m",
        b'n' => "n",
        b'o' => "o",
        b'p' => "p",
        b'q' => "q",
        b'r' => "r",
        b's' => "s",
        b't' => "t",
        b'u' => "u",
        b'v' => "v",
        b'w' => "w",
        b'x' => "x",
        b'y' => "y",
        b'z' => "z",
        b'[' => "bracketleft",
        b'\\' => "backslash",
        b']' => "bracketright",
        b'^' => "asciicircum",
        b'_' => "underscore",
        b'`' => "grave",
        b'{' => "braceleft",
        b'|' => "bar",
        b'}' => "braceright",
        b'~' => "asciitilde",
        _ => return None,
    })
}

pub(super) fn compose_type3_text_transform(
    text_matrix: [f64; 6],
    font_size: f64,
    horizontal_scaling: f64,
    text_rise: f64,
) -> [f64; 6] {
    let [a, b, c, d, e, f] = text_matrix;
    let h_scale = horizontal_scaling / 100.0;

    [
        a * font_size * h_scale,
        b * font_size * h_scale,
        c * font_size,
        d * font_size,
        c * text_rise + e,
        d * text_rise + f,
    ]
}

pub(super) fn text_paint_for_rendering_mode(
    mode: crate::rendering::graphics_state::TextRenderingMode,
    fill_color: Color,
) -> Paint {
    use super::graphics_state::TextRenderingMode;

    match mode {
        // Invisible/clip-only text should not be painted but still advance the text matrix.
        // Stroke-only modes use the stroke paint instead of fill.
        TextRenderingMode::Invisible
        | TextRenderingMode::Clip
        | TextRenderingMode::Stroke
        | TextRenderingMode::StrokeClip => Paint::Transparent,
        _ => Paint::from_color(fill_color),
    }
}

/// Stroke paint for text rendering modes that stroke glyph outlines (1, 2, 5, 6).
pub(super) fn text_stroke_paint_for_rendering_mode(
    mode: crate::rendering::graphics_state::TextRenderingMode,
    stroke_color: Color,
) -> Paint {
    use super::graphics_state::TextRenderingMode;

    match mode {
        TextRenderingMode::Stroke
        | TextRenderingMode::FillStroke
        | TextRenderingMode::StrokeClip
        | TextRenderingMode::FillStrokeClip => Paint::from_color(stroke_color),
        // Invisible/clip-only/fill-only: no stroke contribution.
        _ => Paint::Transparent,
    }
}

/// Whether the rendering mode requires adding glyph outlines to the clip path.
pub(super) fn mode_clips(mode: crate::rendering::graphics_state::TextRenderingMode) -> bool {
    matches!(
        mode,
        crate::rendering::graphics_state::TextRenderingMode::FillClip
            | crate::rendering::graphics_state::TextRenderingMode::StrokeClip
            | crate::rendering::graphics_state::TextRenderingMode::FillStrokeClip
            | crate::rendering::graphics_state::TextRenderingMode::Clip
    )
}

pub(super) fn mode_fills(mode: crate::rendering::graphics_state::TextRenderingMode) -> bool {
    use super::graphics_state::TextRenderingMode;
    matches!(
        mode,
        TextRenderingMode::Fill
            | TextRenderingMode::FillStroke
            | TextRenderingMode::FillClip
            | TextRenderingMode::FillStrokeClip
    )
}

pub(super) fn mode_strokes(mode: crate::rendering::graphics_state::TextRenderingMode) -> bool {
    use super::graphics_state::TextRenderingMode;
    matches!(
        mode,
        TextRenderingMode::Stroke
            | TextRenderingMode::FillStroke
            | TextRenderingMode::StrokeClip
            | TextRenderingMode::FillStrokeClip
    )
}

pub(super) fn resolve_object_clone(
    object: PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<PDFObject> {
    match object {
        PDFObject::Ref(r) => resolver
            .as_mut()?
            .resolve_ref(r.num, r.generation)
            .ok()
            .map(|o| (*o).clone()),
        other => Some(other),
    }
}

pub(super) fn color_from_color_space(
    color_space: &PDFObject,
    args: &[PDFObject],
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Color> {
    match color_space {
        PDFObject::Name(name) => color_from_named_space(name, args),
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" && arr.len() >= 2 => {
                let (xw, yw, zw, amin, amax, bmin, bmax) = parse_lab_params(arr);
                let l = extract_number(args, 0).ok()?;
                let a = extract_number(args, 1).ok()?;
                let b = extract_number(args, 2).ok()?;
                Some(lab_components_to_color(
                    l, a, b, xw, yw, zw, amin, amax, bmin, bmax,
                ))
            }
            Some(PDFObject::Name(name)) if name == "Separation" && arr.len() >= 4 => {
                let tint = extract_number(args, 0).ok()?;
                let alternate = resolve_object_clone(arr[2].as_ref().clone(), resolver)?;
                let tint_fn = resolve_object_clone(arr[3].as_ref().clone(), resolver)?;
                let alternate_components = components_for_color_space(&alternate, resolver).max(1);
                let components = eval_tint_function(&tint_fn, &[tint], alternate_components)?;
                color_from_components(&alternate, &components, resolver)
            }
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                color_from_icc_based(arr[1].as_ref(), args, resolver)
            }
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn color_from_named_space(name: &str, args: &[PDFObject]) -> Option<Color> {
    match name {
        "DeviceGray" | "G" => Some(Color::Gray(extract_number(args, 0).ok()?)),
        "DeviceRGB" | "RGB" => Some(Color::RGB(
            extract_number(args, 0).ok()?,
            extract_number(args, 1).ok()?,
            extract_number(args, 2).ok()?,
        )),
        "DeviceCMYK" | "CMYK" => Some(Color::CMYK(
            extract_number(args, 0).ok()?,
            extract_number(args, 1).ok()?,
            extract_number(args, 2).ok()?,
            extract_number(args, 3).ok()?,
        )),
        _ => None,
    }
}

pub(super) fn initial_color_for_color_space(
    color_space: Option<PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Color> {
    let color_space = resolve_object_clone(color_space?, resolver)?;
    match color_space {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => Some(Color::Gray(0.0)),
            "DeviceRGB" | "RGB" => Some(Color::RGB(0.0, 0.0, 0.0)),
            "DeviceCMYK" | "CMYK" => Some(Color::CMYK(0.0, 0.0, 0.0, 1.0)),
            _ => None,
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" => Some(Color::RGB(0.0, 0.0, 0.0)),
            Some(PDFObject::Name(name)) if name == "Separation" => color_from_color_space(
                &PDFObject::Array(arr.clone()),
                &[PDFObject::Number(1.0)],
                resolver,
            ),
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                let profile = resolve_object_clone(arr[1].as_ref().clone(), resolver)
                    .unwrap_or_else(|| arr[1].as_ref().clone());
                match icc_based_component_count(&profile) {
                    1 => Some(Color::Gray(0.0)),
                    3 => Some(Color::RGB(0.0, 0.0, 0.0)),
                    4 => Some(Color::CMYK(0.0, 0.0, 0.0, 1.0)),
                    _ => None,
                }
            }
            _ => None,
        },
        PDFObject::Ref(_) => initial_color_for_color_space(Some(color_space), resolver),
        _ => None,
    }
}

pub(super) fn color_from_components(
    color_space: &PDFObject,
    components: &[f64],
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Color> {
    match color_space {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => Some(Color::Gray(*components.first()?)),
            "DeviceRGB" | "RGB" => Some(Color::RGB(
                *components.first()?,
                *components.get(1)?,
                *components.get(2)?,
            )),
            "DeviceCMYK" | "CMYK" => Some(Color::CMYK(
                *components.first()?,
                *components.get(1)?,
                *components.get(2)?,
                *components.get(3)?,
            )),
            _ => None,
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" && arr.len() >= 2 => {
                let (xw, yw, zw, amin, amax, bmin, bmax) = parse_lab_params(arr);
                Some(lab_components_to_color(
                    *components.first()?,
                    *components.get(1)?,
                    *components.get(2)?,
                    xw,
                    yw,
                    zw,
                    amin,
                    amax,
                    bmin,
                    bmax,
                ))
            }
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                let profile = resolve_object_clone(arr[1].as_ref().clone(), resolver)?;
                color_from_icc_components(&profile, components)
            }
            _ => None,
        },
        PDFObject::Ref(_) => {
            let resolved = resolve_object_clone(color_space.clone(), resolver)?;
            color_from_components(&resolved, components, resolver)
        }
        _ => None,
    }
}

pub(super) fn components_for_color_space(
    color_space: &PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> usize {
    match color_space {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => 1,
            "DeviceRGB" | "RGB" => 3,
            "DeviceCMYK" | "CMYK" => 4,
            _ => 0,
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" => 3,
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                let profile = resolve_object_clone(arr[1].as_ref().clone(), resolver);
                profile
                    .as_ref()
                    .map(icc_based_component_count)
                    .unwrap_or_else(|| icc_based_component_count(arr[1].as_ref()))
            }
            _ => 0,
        },
        PDFObject::Ref(_) => {
            let resolved = resolve_object_clone(color_space.clone(), resolver);
            resolved
                .as_ref()
                .map(|resolved| components_for_color_space(resolved, resolver))
                .unwrap_or(0)
        }
        _ => 0,
    }
}

pub(super) fn color_from_icc_based(
    profile_obj: &PDFObject,
    args: &[PDFObject],
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Color> {
    let profile = resolve_object_clone(profile_obj.clone(), resolver)?;
    let components: Vec<f64> = args
        .iter()
        .filter_map(|arg| match arg {
            PDFObject::Number(value) => Some(*value),
            _ => None,
        })
        .collect();

    #[cfg(feature = "rendering")]
    if let Some(color) = color_from_icc_components_with_profile(&profile, &components) {
        return Some(color);
    }

    if let PDFObject::Stream { dict, .. } = &profile {
        if let Some(alternate) = dict
            .get("Alternate")
            .and_then(|alt| resolve_object_clone(alt.clone(), resolver))
        {
            if let Some(color) = color_from_color_space(&alternate, args, resolver) {
                return Some(color);
            }
        }
    }

    color_from_icc_components(&profile, &components)
}

pub(super) fn color_from_icc_components(
    profile_obj: &PDFObject,
    components: &[f64],
) -> Option<Color> {
    #[cfg(feature = "rendering")]
    if let Some(color) = color_from_icc_components_with_profile(profile_obj, components) {
        return Some(color);
    }

    match icc_based_component_count(profile_obj) {
        1 => Some(Color::Gray(*components.first()?)),
        3 => Some(Color::RGB(
            *components.first()?,
            *components.get(1)?,
            *components.get(2)?,
        )),
        4 => Some(Color::CMYK(
            *components.first()?,
            *components.get(1)?,
            *components.get(2)?,
            *components.get(3)?,
        )),
        _ => None,
    }
}

#[cfg(feature = "rendering")]
pub(super) fn color_from_icc_components_with_profile(
    profile_obj: &PDFObject,
    components: &[f64],
) -> Option<Color> {
    use moxcms::{ColorProfile, Layout, TransformOptions};

    let PDFObject::Stream { dict, data } = profile_obj else {
        return None;
    };
    let n = dict
        .get("N")
        .and_then(|value| value.as_number())
        .map(|value| value as usize)?;
    if components.len() < n {
        return None;
    }

    let profile_data = dict
        .get("Filter")
        .and_then(|filter| crate::core::decode::apply_filters(data, filter).ok())
        .unwrap_or_else(|| data.clone());

    if n == 3 && is_srgb_icc_profile(&profile_data) {
        return Some(Color::RGB(
            components[0].clamp(0.0, 1.0),
            components[1].clamp(0.0, 1.0),
            components[2].clamp(0.0, 1.0),
        ));
    }

    let src = ColorProfile::new_from_slice(&profile_data).ok()?;
    let src_layout = match n {
        1 => Layout::Gray,
        3 => Layout::Rgb,
        4 => Layout::Rgba,
        _ => return None,
    };
    let dst = ColorProfile::new_srgb();
    let transform = src
        .create_transform_f32(src_layout, &dst, Layout::Rgb, TransformOptions::default())
        .ok()?;
    let input: Vec<f32> = components.iter().take(n).map(|v| *v as f32).collect();
    let mut output = [0.0f32; 3];
    transform.transform(&input, &mut output).ok()?;
    Some(Color::RGB(
        output[0].clamp(0.0, 1.0) as f64,
        output[1].clamp(0.0, 1.0) as f64,
        output[2].clamp(0.0, 1.0) as f64,
    ))
}

#[cfg(feature = "rendering")]
pub(super) fn is_srgb_icc_profile(profile: &[u8]) -> bool {
    profile.len() >= 128
        && profile.get(36..40) == Some(b"acsp")
        && profile.get(16..20) == Some(b"RGB ")
        && (profile
            .windows(b"sRGB IEC61966-2.1".len())
            .any(|window| window == b"sRGB IEC61966-2.1")
            || profile
                .windows(b"IEC 61966-2.1".len())
                .any(|window| window == b"IEC 61966-2.1"))
}

pub(super) fn icc_based_component_count(profile_obj: &PDFObject) -> usize {
    match profile_obj {
        PDFObject::Stream { dict, .. } => dict
            .get("N")
            .and_then(|value| value.as_number())
            .map(|value| value as usize)
            .unwrap_or(0),
        _ => 0,
    }
}

pub(super) fn eval_tint_function(
    function: &PDFObject,
    input: &[f64],
    output_count: usize,
) -> Option<Vec<f64>> {
    let dict = match function {
        PDFObject::Dictionary(dict) => dict,
        PDFObject::Stream { dict, .. } => dict,
        _ => return None,
    };
    match dict.get("FunctionType") {
        Some(PDFObject::Number(n)) if (*n as i32) == 0 => {
            let PDFObject::Stream { data, .. } = function else {
                return None;
            };
            let sampled = parse_sampled_function(dict, data);
            let mut output = sampled.eval_inputs(input);
            output.truncate(output_count);
            Some(output)
        }
        Some(PDFObject::Number(n)) if (*n as i32) == 2 => eval_type2_function(dict, input),
        Some(PDFObject::Number(n)) if (*n as i32) == 4 => {
            let PDFObject::Stream { data, .. } = function else {
                return None;
            };
            eval_type4_function(dict, data, input, output_count)
        }
        _ => None,
    }
}

pub(super) fn eval_type2_function(
    dict: &HashMap<String, PDFObject>,
    input: &[f64],
) -> Option<Vec<f64>> {
    // Ported from pdf.js `PDFFunctionFactory.constructInterpolated`
    // (`src/core/function.js`): y_j = C0_j + x^N * (C1_j - C0_j).
    let x = input.first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
    let c0 = number_array(dict.get("C0"))?;
    let c1 = number_array(dict.get("C1"))?;
    let n = match dict.get("N") {
        Some(PDFObject::Number(n)) => *n,
        _ => 1.0,
    };
    let scale = x.powf(n);
    Some(
        c0.iter()
            .zip(c1.iter())
            .map(|(start, end)| start + scale * (end - start))
            .collect(),
    )
}

pub(super) fn eval_type4_function(
    _dict: &HashMap<String, PDFObject>,
    data: &[u8],
    input: &[f64],
    output_count: usize,
) -> Option<Vec<f64>> {
    // Minimal PostScript calculator evaluator for tint transforms, following
    // pdf.js `PostScriptEvaluator` behavior for stack arithmetic operators.
    let program = std::str::from_utf8(data).ok()?;
    let mut stack = input.to_vec();
    for token in program
        .replace(['{', '}'], " ")
        .split_whitespace()
        .filter(|token| !token.is_empty())
    {
        if let Ok(value) = token.parse::<f64>() {
            stack.push(value);
            continue;
        }
        match token {
            "dup" => stack.push(*stack.last()?),
            "exch" => {
                let len = stack.len();
                if len < 2 {
                    return None;
                }
                stack.swap(len - 1, len - 2);
            }
            "pop" => {
                stack.pop()?;
            }
            "add" => binary_stack_op(&mut stack, |a, b| a + b)?,
            "sub" => binary_stack_op(&mut stack, |a, b| a - b)?,
            "mul" => binary_stack_op(&mut stack, |a, b| a * b)?,
            "div" => binary_stack_op(&mut stack, |a, b| a / b)?,
            _ => return None,
        }
    }
    if stack.len() < output_count {
        return None;
    }
    Some(stack[stack.len() - output_count..].to_vec())
}

pub(super) fn binary_stack_op(
    stack: &mut Vec<f64>,
    op: impl FnOnce(f64, f64) -> f64,
) -> Option<()> {
    let b = stack.pop()?;
    let a = stack.pop()?;
    stack.push(op(a, b));
    Some(())
}

pub(super) fn number_array(object: Option<&PDFObject>) -> Option<Vec<f64>> {
    let PDFObject::Array(arr) = object? else {
        return None;
    };
    arr.iter()
        .map(|item| match &**item {
            PDFObject::Number(n) => Some(*n),
            _ => None,
        })
        .collect()
}

/// Resolve and expand an Indexed color space lookup table.
pub(super) fn expand_indexed_cs(
    data: &[u8],
    cs_obj: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
    bits_per_component: u8,
    width: u32,
    height: u32,
) -> (Vec<u8>, bool) {
    // Try to resolve the ColorSpace and lookup table.
    let cs_obj = match cs_obj {
        Some(o) => o,
        None => return (data.to_vec(), false),
    };

    // Resolve indirect ref for the ColorSpace array
    let resolved_cs;
    let cs = match cs_obj {
        PDFObject::Ref(r) => {
            let res = match resolver.as_mut() {
                Some(r) => r,
                None => return (data.to_vec(), false),
            };
            match res.resolve_ref(r.num, r.generation) {
                Ok(o) => {
                    resolved_cs = o;
                    &*resolved_cs
                }
                Err(_) => return (data.to_vec(), false),
            }
        }
        other => other,
    };

    let cs_arr = match cs {
        PDFObject::Array(a) if a.len() >= 4 => a,
        _ => return (data.to_vec(), false),
    };

    if !matches!(&*cs_arr[0], PDFObject::Name(n) if n == "Indexed") {
        return (data.to_vec(), false);
    }

    let base_channels = match &*cs_arr[1] {
        PDFObject::Name(n) => match n.as_str() {
            "DeviceRGB" | "CalRGB" => 3,
            "DeviceGray" | "CalGray" => 1,
            "DeviceCMYK" => 4,
            _ => 3,
        },
        _ => 3,
    };

    // Get lookup table (inline String/HexString/Stream or Ref).
    let lut = match &*cs_arr[3] {
        PDFObject::String(s) | PDFObject::HexString(s) => s.clone(),
        PDFObject::Stream {
            dict,
            data: stream_data,
        } => {
            if let Some(filter) = dict.get("Filter") {
                crate::core::decode::apply_filters(stream_data, filter)
                    .unwrap_or_else(|_| stream_data.clone())
            } else {
                stream_data.clone()
            }
        }
        PDFObject::Ref(r) => {
            // Resolve the lookup table reference.
            let res = match resolver.as_mut() {
                Some(r) => r,
                None => return (data.to_vec(), false),
            };
            match res.resolve_ref(r.num, r.generation) {
                Ok(fetched) => match &*fetched {
                    PDFObject::String(s) | PDFObject::HexString(s) => s.clone(),
                    PDFObject::Stream {
                        dict,
                        data: stream_data,
                    } => {
                        if let Some(filter) = dict.get("Filter") {
                            crate::core::decode::apply_filters(stream_data, filter)
                                .unwrap_or_else(|_| stream_data.clone())
                        } else {
                            stream_data.clone()
                        }
                    }
                    _ => return (data.to_vec(), false),
                },
                Err(_) => return (data.to_vec(), false),
            }
        }
        _ => return (data.to_vec(), false),
    };

    let hival = cs_arr
        .get(2)
        .and_then(|item| item.as_number())
        .map(|value| value as u32)
        .unwrap_or(255);

    (
        expand_indexed(
            data,
            &lut,
            base_channels,
            bits_per_component,
            width,
            height,
            hival,
        ),
        true,
    )
}

pub(super) fn is_indexed_color_space(
    cs_obj: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> bool {
    let Some(cs_obj) = cs_obj else {
        return false;
    };

    let resolved_cs;
    let cs = match cs_obj {
        PDFObject::Ref(r) => {
            let Some(resolver) = resolver.as_mut() else {
                return false;
            };
            match resolver.resolve_ref(r.num, r.generation) {
                Ok(object) => {
                    resolved_cs = object;
                    &*resolved_cs
                }
                Err(_) => return false,
            }
        }
        other => other,
    };

    matches!(
        cs,
        PDFObject::Array(arr)
            if matches!(arr.first().map(|item| item.as_ref()), Some(PDFObject::Name(name)) if name == "Indexed")
    )
}

pub(super) fn image_color_space_info(
    dict: &HashMap<String, PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> ImageColorSpaceInfo {
    let color_space = dict.get("ColorSpace").or_else(|| dict.get("CS"));
    image_color_space_obj_info(color_space, resolver)
}

pub(super) fn image_color_space_obj_info(
    obj: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> ImageColorSpaceInfo {
    let Some(obj) = obj else {
        return ImageColorSpaceInfo {
            name: Some("DeviceRGB"),
            components: Some(3),
        };
    };

    match obj {
        PDFObject::Name(name) => match name.as_str() {
            "DeviceGray" | "G" => ImageColorSpaceInfo {
                name: Some("DeviceGray"),
                components: Some(1),
            },
            "DeviceRGB" | "RGB" => ImageColorSpaceInfo {
                name: Some("DeviceRGB"),
                components: Some(3),
            },
            "DeviceCMYK" | "CMYK" => ImageColorSpaceInfo {
                name: Some("DeviceCMYK"),
                components: Some(4),
            },
            _ => ImageColorSpaceInfo {
                name: Some("DeviceRGB"),
                components: Some(3),
            },
        },
        PDFObject::Array(arr) => match arr.first().map(|item| item.as_ref()) {
            Some(PDFObject::Name(name)) if name == "Lab" => ImageColorSpaceInfo {
                name: Some("Lab"),
                components: Some(3),
            },
            Some(PDFObject::Name(name)) if name == "ICCBased" && arr.len() >= 2 => {
                icc_based_image_color_space_info(arr[1].as_ref(), resolver)
            }
            Some(PDFObject::Name(name)) if name == "Indexed" => ImageColorSpaceInfo {
                name: Some("Indexed"),
                components: Some(1),
            },
            Some(PDFObject::Name(name)) if name == "DeviceN" && arr.len() >= 2 => {
                let components = match arr[1].as_ref() {
                    PDFObject::Array(names) => u8::try_from(names.len()).ok(),
                    _ => None,
                };
                ImageColorSpaceInfo {
                    name: Some("DeviceN"),
                    components,
                }
            }
            _ => ImageColorSpaceInfo {
                name: Some("DeviceRGB"),
                components: Some(3),
            },
        },
        PDFObject::Ref(_) => {
            let resolved = resolver
                .as_mut()
                .and_then(|resolver| resolver.resolve_if_ref(obj).ok());
            image_color_space_obj_info(resolved.as_deref(), resolver)
        }
        _ => ImageColorSpaceInfo {
            name: Some("DeviceRGB"),
            components: Some(3),
        },
    }
}

pub(super) fn icc_based_image_color_space_info(
    profile_obj: &PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> ImageColorSpaceInfo {
    let resolved = match profile_obj {
        PDFObject::Ref(_) => resolver
            .as_mut()
            .and_then(|resolver| resolver.resolve_if_ref(profile_obj).ok()),
        _ => None,
    };
    let profile = resolved.as_deref().unwrap_or(profile_obj);
    let components = match profile {
        PDFObject::Stream { dict, .. } | PDFObject::Dictionary(dict) => dict
            .get("N")
            .and_then(PDFObject::as_number)
            .and_then(|n| u8::try_from(n as usize).ok()),
        _ => None,
    };
    let name = match components {
        Some(1) => Some("DeviceGray"),
        Some(3) => Some("DeviceRGB"),
        Some(4) => Some("DeviceCMYK"),
        _ => Some("ICCBased"),
    };
    ImageColorSpaceInfo { name, components }
}

pub(super) fn jpeg_decode_options(
    dict: &HashMap<String, PDFObject>,
    components: Option<u8>,
    color_space_name: Option<&'static str>,
) -> crate::core::image::JpegDecodeOptions {
    let _ = color_space_name;
    let color_transform = dct_color_transform(dict);
    crate::core::image::JpegDecodeOptions {
        components,
        color_transform,
    }
}

pub(super) fn dct_color_transform(dict: &HashMap<String, PDFObject>) -> Option<i32> {
    decode_parms_for_filter(dict, "DCTDecode")
        .or_else(|| decode_parms_for_filter(dict, "DCT"))
        .and_then(|parms| match parms {
            PDFObject::Dictionary(d) => d.get("ColorTransform"),
            _ => None,
        })
        .and_then(PDFObject::as_number)
        .map(|n| n as i32)
}

pub(super) fn decode_parms_for_filter<'a>(
    dict: &'a HashMap<String, PDFObject>,
    filter_name: &str,
) -> Option<&'a PDFObject> {
    let decode_parms = dict.get("DecodeParms").or_else(|| dict.get("DP"))?;
    match decode_parms {
        PDFObject::Dictionary(_) => Some(decode_parms),
        PDFObject::Array(parms) => {
            let index = filter_index(dict.get("Filter").or_else(|| dict.get("F")), filter_name)?;
            parms.get(index).map(|item| item.as_ref())
        }
        _ => None,
    }
}

pub(super) fn filter_index(filter: Option<&PDFObject>, filter_name: &str) -> Option<usize> {
    match filter? {
        PDFObject::Name(name) => {
            if name == filter_name {
                Some(0)
            } else {
                None
            }
        }
        PDFObject::Array(filters) => filters.iter().position(
            |filter| matches!(filter.as_ref(), PDFObject::Name(name) if name == filter_name),
        ),
        _ => None,
    }
}

pub(super) fn filter_contains_any(filter: Option<&PDFObject>, names: &[&str]) -> bool {
    match filter {
        Some(PDFObject::Name(name)) => names.contains(&name.as_str()),
        Some(PDFObject::Array(filters)) => filters.iter().any(|filter| {
            matches!(filter.as_ref(), PDFObject::Name(name) if names.contains(&name.as_str()))
        }),
        _ => false,
    }
}

/// Apply PNG predictor from DecodeParms (handles both Dict and Array forms).
pub(super) fn apply_predictor_from_parms(
    data: Vec<u8>,
    dict: &HashMap<String, PDFObject>,
    default_columns: u32,
) -> (Vec<u8>, Option<u8>) {
    let parms = match dict.get("DecodeParms").or_else(|| dict.get("DP")) {
        Some(p) => p,
        None => return (data, None),
    };
    let parms_dict = match parms {
        PDFObject::Dictionary(d) => Some(d),
        PDFObject::Array(arr) => arr.iter().find_map(|item| match &**item {
            PDFObject::Dictionary(d) => Some(d),
            _ => None,
        }),
        _ => None,
    };
    let parms_dict = match parms_dict {
        Some(d) => d,
        None => return (data, None),
    };
    let pred = match parms_dict.get("Predictor") {
        Some(PDFObject::Number(n)) => *n as i32,
        _ => return (data, None),
    };
    if pred <= 1 {
        return (data, None);
    }
    let colors = match parms_dict.get("Colors") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => 1,
    };
    let bpc = match parms_dict.get("BitsPerComponent") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => match dict.get("BitsPerComponent") {
            Some(PDFObject::Number(n)) => *n as usize,
            _ => 8,
        },
    };
    let image_bpc = match dict.get("BitsPerComponent") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => bpc,
    };
    let predictor_bpc = if bpc == image_bpc {
        u8::try_from(bpc).ok()
    } else {
        None
    };
    let cols = match parms_dict.get("Columns") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => default_columns as usize,
    };
    // Compute row sizes
    let row_bytes = (cols * colors * bpc + 7) / 8;
    let image_row_bytes = (default_columns as usize * colors * image_bpc + 7) / 8;
    let expected_image_bytes = match dict.get("Height") {
        Some(PDFObject::Number(h)) if bpc != image_bpc => {
            Some((*h as usize).saturating_mul(image_row_bytes))
        }
        _ => None,
    };
    if pred == 2 {
        let height = match dict.get("Height") {
            Some(PDFObject::Number(h)) => *h as usize,
            _ => 0,
        };
        let target = if let Some(expected) = expected_image_bytes {
            expected.div_ceil(row_bytes).saturating_mul(row_bytes)
        } else if height > 0 {
            height * row_bytes
        } else {
            (data.len() / row_bytes) * row_bytes
        };
        let mut aligned = data;
        if aligned.len() < target {
            aligned.resize(target, 0);
        } else {
            aligned.truncate(target);
        }
        let decoded = crate::core::decode::decode_tiff_predictor(&aligned, colors, bpc, cols)
            .unwrap_or(aligned);
        if let Some(expected) = expected_image_bytes {
            return (decoded.into_iter().take(expected).collect(), predictor_bpc);
        }
        return (decoded, predictor_bpc);
    }
    if pred < 10 {
        return (data, None);
    }

    let pred_stride = 1 + row_bytes; // stride with predictor byte per row
    let height = match dict.get("Height") {
        Some(PDFObject::Number(h)) => *h as usize,
        _ => 0,
    };

    let data_len = data.len();
    let pred_target = if let Some(expected) = expected_image_bytes {
        expected.div_ceil(row_bytes).saturating_mul(pred_stride)
    } else if height > 0 {
        (height * pred_stride).min(data_len)
    } else {
        (data_len / pred_stride) * pred_stride
    };
    let mut aligned = data;
    if aligned.len() < pred_target {
        aligned.resize(pred_target, 0);
    } else {
        aligned.truncate(pred_target);
    }

    // Try PNG predictor
    let decoded = crate::core::decode::decode_png_predictor(&aligned, colors, bpc, cols);
    match decoded {
        Ok(out) => {
            if let Some(expected) = expected_image_bytes {
                (out.into_iter().take(expected).collect(), predictor_bpc)
            } else {
                (out, predictor_bpc)
            }
        }
        Err(_) => {
            // Predictor failed — data likely has no PNG prediction.
            // Fall back to the exact raw pixel count; malformed streams can
            // declare Predictor=15 even when the decoded bytes are plain pixels.
            let raw_target = if height > 0 {
                height * row_bytes
            } else {
                (data_len / row_bytes) * row_bytes
            };
            aligned.truncate(raw_target.min(aligned.len()));
            (aligned, predictor_bpc)
        }
    }
}

/// Parse Lab color space parameters from the CS array.
pub(super) fn parse_lab_params(cs_arr: &[Box<PDFObject>]) -> (f64, f64, f64, f64, f64, f64, f64) {
    let mut xw = 1.0;
    let mut yw = 1.0;
    let mut zw = 1.0;
    let mut amin = -100.0;
    let mut amax = 100.0;
    let mut bmin = -100.0;
    let mut bmax = 100.0;

    if cs_arr.len() >= 2 {
        if let PDFObject::Dictionary(d) = &*cs_arr[1] {
            if let Some(PDFObject::Array(wp)) = d.get("WhitePoint") {
                if wp.len() >= 3 {
                    if let (PDFObject::Number(x), PDFObject::Number(y), PDFObject::Number(z)) =
                        (&*wp[0], &*wp[1], &*wp[2])
                    {
                        xw = *x;
                        yw = *y;
                        zw = *z;
                    }
                }
            }
            if let Some(PDFObject::Array(range)) = d.get("Range") {
                if range.len() >= 4 {
                    if let (
                        PDFObject::Number(a0),
                        PDFObject::Number(a1),
                        PDFObject::Number(b0),
                        PDFObject::Number(b1),
                    ) = (&*range[0], &*range[1], &*range[2], &*range[3])
                    {
                        amin = *a0;
                        amax = *a1;
                        bmin = *b0;
                        bmax = *b1;
                    }
                }
            }
        }
    }
    (xw, yw, zw, amin, amax, bmin, bmax)
}

/// Convert Lab pixel data to RGB. Reference: pdf.js/src/core/colorspace.js — LabCS.#toRgb
pub(super) fn lab_to_rgb(
    data: &[u8],
    xw: f64,
    yw: f64,
    zw: f64,
    amin: f64,
    amax: f64,
    bmin: f64,
    bmax: f64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(3) {
        if chunk.len() < 3 {
            out.extend_from_slice(chunk);
            continue;
        }
        // Decode from 0-255 to Lab ranges
        let l_star = chunk[0] as f64 * 100.0 / 255.0;
        let a_star = amin + chunk[1] as f64 * (amax - amin) / 255.0;
        let b_star = bmin + chunk[2] as f64 * (bmax - bmin) / 255.0;

        let (r, g, b) =
            lab_components_to_rgb_u8(l_star, a_star, b_star, xw, yw, zw, amin, amax, bmin, bmax);
        out.push(r);
        out.push(g);
        out.push(b);
    }
    out
}

pub(super) fn lab_components_to_color(
    l_star: f64,
    a_star: f64,
    b_star: f64,
    xw: f64,
    yw: f64,
    zw: f64,
    amin: f64,
    amax: f64,
    bmin: f64,
    bmax: f64,
) -> Color {
    let (r, g, b) =
        lab_components_to_rgb_u8(l_star, a_star, b_star, xw, yw, zw, amin, amax, bmin, bmax);
    Color::RGB(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

pub(super) fn lab_components_to_rgb_u8(
    l_star: f64,
    a_star: f64,
    b_star: f64,
    xw: f64,
    yw: f64,
    zw: f64,
    amin: f64,
    amax: f64,
    bmin: f64,
    bmax: f64,
) -> (u8, u8, u8) {
    let fn_g = |x: f64| -> f64 {
        if x >= 6.0 / 29.0 {
            x * x * x
        } else {
            (108.0 / 841.0) * (x - 4.0 / 29.0)
        }
    };

    let a_star = a_star.clamp(amin, amax);
    let b_star = b_star.clamp(bmin, bmax);

    #[cfg(feature = "rendering")]
    if let Some(rgb) =
        crate::rendering::lab_icc::lab_pixel_to_rgb(l_star, a_star, b_star, xw, yw, zw)
    {
        return rgb;
    }

    let m = (l_star + 16.0) / 116.0;
    let l = m + a_star / 500.0;
    let n = m - b_star / 200.0;

    let x = xw * fn_g(l);
    let y = yw * fn_g(m);
    let z = zw * fn_g(n);

    // Ported from pdf.js LabCS.#toRgb.
    let (r, g, b) = if zw < 1.0 {
        (
            x * 3.1339 + y * -1.617 + z * -0.4906,
            x * -0.9785 + y * 1.916 + z * 0.0333,
            x * 0.072 + y * -0.229 + z * 1.4057,
        )
    } else {
        (
            x * 3.2406 + y * -1.5372 + z * -0.4986,
            x * -0.9689 + y * 1.8758 + z * 0.0415,
            x * 0.0557 + y * -0.204 + z * 1.057,
        )
    };

    (
        (r.max(0.0).sqrt() * 255.0).clamp(0.0, 255.0) as u8,
        (g.max(0.0).sqrt() * 255.0).clamp(0.0, 255.0) as u8,
        (b.max(0.0).sqrt() * 255.0).clamp(0.0, 255.0) as u8,
    )
}

/// Each sample in `data` is an index into `lut`, which contains
/// `base_channels` bytes per entry. Indexed images may pack indices at
/// 1/2/4 bpc; PDF.js/hayro unpack those samples before lookup.
pub(super) fn expand_indexed(
    data: &[u8],
    lut: &[u8],
    base_channels: usize,
    bits_per_component: u8,
    width: u32,
    height: u32,
    hival: u32,
) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let row_bytes = (width.saturating_mul(bits_per_component as usize) + 7) / 8;
    let sample_count = width.saturating_mul(height);
    let mut out = Vec::with_capacity(sample_count.saturating_mul(base_channels));
    for y in 0..height {
        let row_start = y.saturating_mul(row_bytes);
        let row = data
            .get(row_start..row_start.saturating_add(row_bytes))
            .unwrap_or(&[]);
        for x in 0..width {
            let idx = read_packed_index_sample(row, x, bits_per_component).min(hival);
            let offset = idx as usize * base_channels;
            for ch in 0..base_channels {
                out.push(lut.get(offset + ch).copied().unwrap_or(0));
            }
        }
    }
    out
}

pub(super) fn read_packed_index_sample(
    data: &[u8],
    sample_index: usize,
    bits_per_component: u8,
) -> u32 {
    match bits_per_component {
        1 | 2 | 4 => {
            let bit_offset = sample_index.saturating_mul(bits_per_component as usize);
            let byte = data.get(bit_offset / 8).copied().unwrap_or(0) as u32;
            let bit_in_byte = bit_offset % 8;
            let shift = 8usize.saturating_sub(bit_in_byte + bits_per_component as usize);
            (byte >> shift) & ((1u32 << bits_per_component) - 1)
        }
        _ => data.get(sample_index).copied().unwrap_or(0) as u32,
    }
}

pub(super) fn image_smask_alpha(
    color_data: &[u8],
    color_space_name: Option<&str>,
    width: u32,
    height: u32,
    smask_obj: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Vec<u8>> {
    let channels = match color_space_name {
        Some("DeviceGray") | Some("CalGray") => 1,
        Some("DeviceCMYK") => 4,
        _ => 3,
    };
    let pixel_count = width as usize * height as usize;
    if color_data.len() < pixel_count.saturating_mul(channels) {
        return None;
    }

    let alpha = decode_smask_image(smask_obj?, width, height, resolver)?;
    if alpha.len() < pixel_count {
        return None;
    }

    let mut rgba = Vec::with_capacity(pixel_count * 4);
    match channels {
        1 => {
            for (gray, alpha) in color_data.iter().take(pixel_count).zip(alpha.iter()) {
                rgba.extend_from_slice(&[*gray, *gray, *gray, *alpha]);
            }
        }
        4 => {
            for (pixel, alpha) in color_data
                .chunks_exact(4)
                .take(pixel_count)
                .zip(alpha.iter())
            {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], *alpha]);
            }
        }
        _ => {
            for (pixel, alpha) in color_data
                .chunks_exact(3)
                .take(pixel_count)
                .zip(alpha.iter())
            {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], *alpha]);
            }
        }
    }
    Some(rgba)
}

pub(super) fn decode_smask_image(
    smask_obj: &PDFObject,
    fallback_width: u32,
    fallback_height: u32,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Vec<u8>> {
    let resolved;
    let smask = match smask_obj {
        PDFObject::Ref(r) => {
            let res = resolver.as_mut()?;
            resolved = res.resolve_ref(r.num, r.generation).ok()?;
            &*resolved
        }
        other => other,
    };

    let PDFObject::Stream { dict, data } = smask else {
        return None;
    };

    if !matches!(dict.get("Subtype"), Some(PDFObject::Name(name)) if name == "Image") {
        return None;
    }

    let width = match dict.get("Width") {
        Some(PDFObject::Number(w)) => *w as u32,
        _ => fallback_width,
    };
    let height = match dict.get("Height") {
        Some(PDFObject::Number(h)) => *h as u32,
        _ => fallback_height,
    };
    if width == 0 || height == 0 || fallback_width == 0 || fallback_height == 0 {
        return None;
    }

    let bpc = match dict.get("BitsPerComponent") {
        Some(PDFObject::Number(b)) => *b as u8,
        _ => 8,
    };

    let filter = dict.get("Filter").or_else(|| dict.get("F"));
    let mut decoded = if let Some(filter) = filter {
        crate::core::decode::apply_filters(data, filter).unwrap_or_else(|_| data.clone())
    } else {
        data.clone()
    };
    let mut bpc = bpc;
    let mut decoded_width = width;
    let mut decoded_height = height;

    if filter_contains_any(filter, &["DCTDecode", "DCT"]) {
        #[cfg(feature = "jpeg-decoding")]
        {
            let color_info = image_color_space_info(dict, &mut None);
            if let Ok(image) = crate::core::image::ImageDecoder::decode_pdf_jpeg(
                &decoded,
                jpeg_decode_options(dict, color_info.components, color_info.name),
            ) {
                decoded = image.data;
                decoded_width = image.metadata.width;
                decoded_height = image.metadata.height;
                bpc = 8;
            }
        }
    } else if filter_contains_any(filter, &["CCITTFaxDecode", "CCF"]) {
        #[cfg(feature = "hayro-ccitt")]
        {
            decoded = decode_ccitt(&decoded, dict, width, height)?;
            bpc = 1;
        }
    } else if filter_contains_any(filter, &["JPXDecode"]) {
        #[cfg(feature = "advanced-image-formats")]
        {
            let color_info = {
                let mut resolver = None;
                image_color_space_info(dict, &mut resolver)
            };
            let (data, jpx_width, jpx_height, jpx_bpc, _) =
                decode_jpx(&decoded, jpx_decode_options(dict, color_info.components))?;
            decoded = data;
            decoded_width = jpx_width;
            decoded_height = jpx_height;
            bpc = jpx_bpc;
        }
    } else if filter_contains_any(filter, &["JBIG2Decode"]) {
        #[cfg(feature = "advanced-image-formats")]
        {
            // Inline images rarely carry JBIG2Globals; pass None.
            decoded = decode_jbig2(&decoded, None)?;
            bpc = 1;
        }
    }

    let (decoded, predictor_bpc) = apply_predictor_from_parms(decoded, dict, decoded_width);
    let bpc = predictor_bpc.unwrap_or(bpc);

    // Soft masks are treated as single-channel coverage. Multi-channel sources
    // (e.g. accidental RGB after JPEG) keep the first channel as gray.
    let gray = match bpc {
        1 => expand_1bpc_image_to_8bpc(&decoded, decoded_width, decoded_height, 255),
        8 => {
            let need = decoded_width as usize * decoded_height as usize;
            if decoded.len() >= need * 3 && decoded.len() != need {
                // RGB/gray-padded: take R (or gray) channel only.
                let stride = if decoded.len() >= need * 4 { 4 } else { 3 };
                decoded
                    .chunks_exact(stride)
                    .take(need)
                    .map(|c| c[0])
                    .collect()
            } else {
                decoded
            }
        }
        16 => downscale_16_to_8(&decoded),
        _ => return None,
    };

    let mut alpha = if let Some(PDFObject::Array(decode_arr)) = dict.get("Decode") {
        apply_decode_inversion(&gray, decode_arr, 8)
    } else {
        gray
    };

    // PDF 1.7 §11.6.5.2 / §8.9.6.3: when soft-mask dimensions differ from the
    // parent image, scale mask samples to the parent size. InDesign/Acrobat
    // often ship 2× SMask bitmaps (tile_clamp_bug photos).
    // Source: PDF 32000-1:2008 §11.6.5.2; hayro soft-mask image path.
    if decoded_width != fallback_width || decoded_height != fallback_height {
        alpha = scale_gray_nearest(
            &alpha,
            decoded_width,
            decoded_height,
            fallback_width,
            fallback_height,
        )?;
    }

    let need = fallback_width as usize * fallback_height as usize;
    if alpha.len() < need {
        return None;
    }
    alpha.truncate(need);
    Some(alpha)
}

/// Nearest-neighbor scale of an 8-bit grayscale buffer (soft-mask coverage).
pub(super) fn scale_gray_nearest(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Option<Vec<u8>> {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return None;
    }
    let sw = src_w as usize;
    let sh = src_h as usize;
    if src.len() < sw.saturating_mul(sh) {
        return None;
    }
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    let mut out = vec![0u8; dw * dh];
    for y in 0..dh {
        let sy = (y as u64 * sh as u64 / dh as u64) as usize;
        let src_row = sy * sw;
        let dst_row = y * dw;
        for x in 0..dw {
            let sx = (x as u64 * sw as u64 / dw as u64) as usize;
            out[dst_row + x] = src[src_row + sx];
        }
    }
    Some(out)
}

pub(super) fn decode_inline_image_data(
    data: &[u8],
    dict: &HashMap<String, PDFObject>,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let filter = dict.get("Filter")?;
    let bits_per_component = dict
        .get("BitsPerComponent")
        .and_then(PDFObject::as_number)
        .unwrap_or(8.0) as u8;

    match filter {
        PDFObject::Name(name) => match name.as_str() {
            "FlateDecode" | "Fl" => {
                let decoded = crate::core::decode::decode_flate(data).ok()?;
                Some(apply_predictor_from_parms(decoded, dict, width).0)
            }
            "LZWDecode" | "LZW" => {
                let decoded = crate::core::decode::decode_lzw(data).ok()?;
                Some(apply_predictor_from_parms(decoded, dict, width).0)
            }
            // Type3 glyph ImageMasks from Ghostscript/dvips often use bare
            // ASCIIHex (`/F/AHx`) rather than a filter array. Without decoding,
            // hex digits are painted as raw 1-bpp samples and scramble the
            // glyph (font_type3_widths_with_matrix).
            // Source: PDF 1.7 §7.4.2; hayro stream decoded_image path.
            "ASCIIHexDecode" | "AHx" => crate::core::decode::decode_ascii_hex(data).ok(),
            "ASCII85Decode" | "A85" => crate::core::decode::decode_ascii85(data).ok(),
            "RunLengthDecode" | "RL" => crate::core::decode::decode_run_length(data).ok(),
            "CCITTFaxDecode" | "CCF" => {
                #[cfg(feature = "hayro-ccitt")]
                {
                    decode_ccitt(data, dict, width, height)
                }
                #[cfg(not(feature = "hayro-ccitt"))]
                {
                    None
                }
            }
            "DCTDecode" | "DCT" => {
                #[cfg(feature = "jpeg-decoding")]
                {
                    let color_info = image_color_space_info(dict, &mut None);
                    crate::core::image::ImageDecoder::decode_pdf_jpeg(
                        data,
                        jpeg_decode_options(dict, color_info.components, color_info.name),
                    )
                    .ok()
                    .map(|image| image.data)
                }
                #[cfg(not(feature = "jpeg-decoding"))]
                {
                    None
                }
            }
            _ => None,
        },
        PDFObject::Array(filters) => {
            let decoded = crate::core::decode::apply_filters(data, filter).ok()?;
            let filter_names: Vec<&str> = filters
                .iter()
                .filter_map(|filter| match filter.as_ref() {
                    PDFObject::Name(name) => Some(name.as_str()),
                    _ => None,
                })
                .collect();

            if filter_names.contains(&"CCITTFaxDecode") || filter_names.contains(&"CCF") {
                #[cfg(feature = "hayro-ccitt")]
                {
                    decode_ccitt(&decoded, dict, width, height)
                }
                #[cfg(not(feature = "hayro-ccitt"))]
                {
                    None
                }
            } else if filter_names.contains(&"DCTDecode") || filter_names.contains(&"DCT") {
                #[cfg(feature = "jpeg-decoding")]
                {
                    let color_info = image_color_space_info(dict, &mut None);
                    crate::core::image::ImageDecoder::decode_pdf_jpeg(
                        &decoded,
                        jpeg_decode_options(dict, color_info.components, color_info.name),
                    )
                    .ok()
                    .map(|image| image.data)
                }
                #[cfg(not(feature = "jpeg-decoding"))]
                {
                    None
                }
            } else if bits_per_component > 0 {
                Some(apply_predictor_from_parms(decoded, dict, width).0)
            } else {
                Some(decoded)
            }
        }
        _ => None,
    }
}

/// Downscale 16-bit pixel data to 8-bit (take high byte of each 16-bit sample).
pub(super) fn downscale_16_to_8(data: &[u8]) -> Vec<u8> {
    data.chunks_exact(2).map(|pair| pair[0]).collect()
}

pub(super) fn should_expand_1bpc_sampled_image(filter: Option<&PDFObject>) -> bool {
    match filter {
        None => true,
        Some(PDFObject::Name(name)) => {
            matches!(
                name.as_str(),
                "FlateDecode" | "Fl" | "LZWDecode" | "LZW" | "CCITTFaxDecode" | "CCF"
            )
        }
        Some(PDFObject::Array(filters)) => filters.iter().all(|filter| {
            matches!(
                filter.as_ref(),
                PDFObject::Name(name)
                    if !matches!(
                        name.as_str(),
                        "DCTDecode"
                            | "DCT"
                            | "JPXDecode"
                            | "JBIG2Decode"
                    )
            )
        }),
        _ => false,
    }
}

/// Expand a 1-bit sampled image to one byte per pixel.
///
/// DeviceGray-style images use `one_value = 255`; Indexed images use `one_value = 1`
/// so palette expansion still receives valid lookup indices.
pub(super) fn expand_1bpc_image_to_8bpc(
    data: &[u8],
    width: u32,
    height: u32,
    one_value: u8,
) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let row_bytes = (width + 7) / 8;
    let mut out = Vec::with_capacity(width.saturating_mul(height));

    for y in 0..height {
        let row_start = y.saturating_mul(row_bytes);
        let row = data
            .get(row_start..row_start.saturating_add(row_bytes))
            .unwrap_or(&[]);
        for x in 0..width {
            let byte = row.get(x / 8).copied().unwrap_or(0);
            let bit = 7 - (x % 8);
            let sample = (byte >> bit) & 1;
            out.push(if sample == 0 { 0 } else { one_value });
        }
    }

    out
}

pub(super) fn image_mask_paint_one(dict: &HashMap<String, PDFObject>) -> bool {
    if let Some(PDFObject::Array(decode)) = dict.get("Decode") {
        return match (decode.first(), decode.get(1)) {
            (Some(a), Some(b)) => match (a.as_ref(), b.as_ref()) {
                (PDFObject::Number(min), PDFObject::Number(max)) => *min > *max,
                _ => false,
            },
            _ => false,
        };
    }

    false
}

/// Apply /Decode array inversion. Each pair [min max] in the array
/// controls one component. When [1 0], the component is inverted (255 - v).
/// When [0 1] (default), no change. Only 8-bpc is handled.
pub(super) fn apply_decode_inversion(data: &[u8], decode: &[Box<PDFObject>], bpc: u8) -> Vec<u8> {
    if bpc != 8 {
        return data.to_vec();
    }

    // Determine which channels are inverted ([1 0] = inverted).
    let channels = decode.len() / 2;
    let mut inverted = Vec::with_capacity(channels);
    for i in 0..channels {
        let is_inverted = match (decode.get(i * 2), decode.get(i * 2 + 1)) {
            (Some(b), Some(c)) => match (&**b, &**c) {
                (PDFObject::Number(min), PDFObject::Number(max)) => *min == 1.0 && *max == 0.0,
                _ => false,
            },
            _ => false,
        };
        inverted.push(is_inverted);
    }

    if !inverted.iter().any(|&b| b) {
        return data.to_vec(); // No inversions needed.
    }

    let mut result = data.to_vec();
    for (i, byte) in result.iter_mut().enumerate() {
        let ch = i % channels;
        if ch < inverted.len() && inverted[ch] {
            *byte = 255 - *byte;
        }
    }
    result
}

/// Post-concatenate a translation matrix.
///
/// Equivalent to `matrix * translate(tx, ty)` in PDF 2D affine form.
pub(super) fn post_concat_translate(matrix: [f64; 6], tx: f64, ty: f64) -> [f64; 6] {
    post_concat_matrix(matrix, [1.0, 0.0, 0.0, 1.0, tx, ty])
}

/// Multiply two PDF affine transforms in 6-element form.
///
/// Returns `left * right`.
pub(super) fn post_concat_matrix(left: [f64; 6], right: [f64; 6]) -> [f64; 6] {
    let [a1, b1, c1, d1, e1, f1] = left;
    let [a2, b2, c2, d2, e2, f2] = right;

    [
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * e2 + c1 * f2 + e1,
        b1 * e2 + d1 * f2 + f1,
    ]
}

pub(super) fn remap_text_bytes_with_cmap(bytes: &[u8], cmap: &CMap) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        return None;
    }

    let one_byte_codes: Vec<u16> = bytes.iter().map(|&b| b as u16).collect();
    let one_byte_hits = one_byte_codes
        .iter()
        .filter(|&&code| cmap.to_unicode(code).is_some())
        .count();

    let mut best_codes = one_byte_codes;
    let mut best_hits = one_byte_hits;
    let mut best_is_two_byte = false;

    if bytes.len() % 2 == 0 {
        let two_byte_codes: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        let two_byte_hits = two_byte_codes
            .iter()
            .filter(|&&code| cmap.to_unicode(code).is_some())
            .count();

        if two_byte_hits > best_hits {
            best_codes = two_byte_codes;
            best_hits = two_byte_hits;
            best_is_two_byte = true;
        }
    }

    if best_hits == 0 {
        return None;
    }

    let mapped_chars: Vec<char> = best_codes
        .iter()
        .map(|&code| {
            cmap.to_unicode(code)
                .or_else(|| char::from_u32(code as u32))
                .unwrap_or('\u{FFFD}')
        })
        .collect();

    if mapped_chars.iter().all(|&ch| (ch as u32) <= 0xFF) {
        let mapped_single_byte: Vec<u8> = mapped_chars.iter().map(|&ch| ch as u8).collect();
        if best_is_two_byte || mapped_single_byte != bytes {
            return Some(mapped_single_byte);
        }
        return None;
    }

    let mut mapped_utf16be = Vec::new();
    for unicode_char in mapped_chars {
        let mut utf16_buf = [0u16; 2];
        let units = unicode_char.encode_utf16(&mut utf16_buf);
        for unit in units.iter() {
            mapped_utf16be.extend_from_slice(&unit.to_be_bytes());
        }
    }

    if best_is_two_byte || mapped_utf16be != bytes {
        Some(mapped_utf16be)
    } else {
        None
    }
}

// Image decoder helpers

#[cfg(feature = "hayro-ccitt")]
pub(super) fn decode_ccitt(
    data: &[u8],
    dict: &HashMap<String, PDFObject>,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    use hayro_ccitt::{self, DecodeSettings, EncodingMode};
    let parms = ccitt_decode_parms(dict);
    let k = num_in(parms, "K").unwrap_or(0);
    let columns = num_in(parms, "Columns").unwrap_or(width as i64) as u32;
    let end_of_block = bool_in(parms, "EndOfBlock").unwrap_or(true);
    let mut rows = num_in(parms, "Rows").unwrap_or(height as i64) as u32;
    if end_of_block {
        rows = u32::MAX;
    }
    let settings = DecodeSettings {
        columns,
        rows,
        end_of_block,
        end_of_line: bool_in(parms, "EndOfLine").unwrap_or(false),
        rows_are_byte_aligned: bool_in(parms, "EncodedByteAlign").unwrap_or(false),
        encoding: if k < 0 {
            EncodingMode::Group4
        } else if k > 0 {
            EncodingMode::Group3_2D { k: k as u32 }
        } else {
            EncodingMode::Group3_1D
        },
        invert_black: bool_in(parms, "BlackIs1").unwrap_or(false),
    };
    let mut sink = BitmapSink::new();
    let _ = hayro_ccitt::decode(data, &mut sink, &settings);
    Some(sink.finish())
}

#[cfg(feature = "advanced-image-formats")]
pub(super) fn jpx_decode_options(
    dict: &HashMap<String, PDFObject>,
    pdf_components: Option<u8>,
) -> JpxDecodeOptions {
    let pdf_components = if dict.contains_key("ColorSpace") || dict.contains_key("CS") {
        pdf_components
    } else {
        None
    };
    let smask_in_data = dict
        .get("SMaskInData")
        .and_then(PDFObject::as_number)
        .is_some_and(|n| n as i64 == 1);
    JpxDecodeOptions {
        pdf_components,
        smask_in_data,
    }
}

#[cfg(feature = "advanced-image-formats")]

/// Decode a PDF-embedded JBIG2 image stream.
///
/// Uses hayro-jbig2's PDF.js-style decoder (bare segments + optional globals,
/// including SymbolDictionary / TextRegion). Falls back to the standalone
/// file decoder when the input carries a JBIG2 file header.
///
/// Source: hayro-jbig2 `pdf_stream.rs` ← PDF.js `core/jbig2.js`.
#[cfg(feature = "advanced-image-formats")]
pub(super) fn decode_jbig2(data: &[u8], globals: Option<&[u8]>) -> Option<Vec<u8>> {
    // Prefer the PDF-aware decoder (handles bare streams + JBIG2Globals).
    if let Some(bitmap) = crate::core::jbig2_pdf::decode_with_globals(data, globals) {
        return Some(bitmap);
    }
    // Fallback: standalone JBIG2 file (with file header).
    // Polarity: hayro-jbig2 true=black; DeviceGray wants bit0=black.
    let image = hayro_jbig2::decode(data).ok()?;
    let width = image.width;
    let height = image.height;
    let row_bytes = ((width + 7) / 8) as usize;
    let mut bitmap = vec![0u8; row_bytes * height as usize];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) as usize;
            // true = black → leave bit 0; false = white → set bit 1
            if i < image.data.len() && !image.data[i] {
                let byte_i = y as usize * row_bytes + (x as usize / 8);
                bitmap[byte_i] |= 1 << (7 - (x % 8));
            }
        }
    }
    Some(bitmap)
}

/// Resolve `/DecodeParms` → `/JBIG2Globals` stream data (already filter-decoded).
///
/// DecodeParms may be a dict or an array parallel to the Filter array
/// (ISO 32000-1 §7.4.7, Table 12).
#[cfg(feature = "advanced-image-formats")]

/// Resolve `/DecodeParms` → `/JBIG2Globals` stream data (already filter-decoded).
///
/// DecodeParms may be a dict or an array parallel to the Filter array
/// (ISO 32000-1 §7.4.7, Table 12).
#[cfg(feature = "advanced-image-formats")]
pub(super) fn jbig2_globals_data(
    dict: &HashMap<String, PDFObject>,
    filter: Option<&PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Vec<u8>> {
    let parms = dict.get("DecodeParms").or_else(|| dict.get("DP"))?;
    let parms_dict = match parms {
        PDFObject::Dictionary(d) => d,
        PDFObject::Array(arr) => {
            let idx = jbig2_filter_index(filter)?;
            match arr.get(idx).map(|o| o.as_ref()) {
                Some(PDFObject::Dictionary(d)) => d,
                Some(PDFObject::Null) | None => return None,
                Some(PDFObject::Ref(r)) => {
                    let res = resolver.as_mut()?;
                    let fetched = res.resolve_ref(r.num, r.generation).ok()?;
                    match &*fetched {
                        PDFObject::Dictionary(d) => {
                            // Need owned data from the stream below; resolve globals from this dict.
                            return resolve_jbig2_globals_from_dict(d, resolver);
                        }
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        PDFObject::Ref(r) => {
            let res = resolver.as_mut()?;
            let fetched = res.resolve_ref(r.num, r.generation).ok()?;
            match &*fetched {
                PDFObject::Dictionary(d) => {
                    return resolve_jbig2_globals_from_dict(d, resolver);
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    resolve_jbig2_globals_from_dict(parms_dict, resolver)
}

#[cfg(feature = "advanced-image-formats")]
pub(super) fn jbig2_filter_index(filter: Option<&PDFObject>) -> Option<usize> {
    match filter? {
        PDFObject::Name(name) if name == "JBIG2Decode" => Some(0),
        PDFObject::Array(filters) => filters
            .iter()
            .position(|f| matches!(f.as_ref(), PDFObject::Name(n) if n == "JBIG2Decode")),
        _ => None,
    }
}

#[cfg(feature = "advanced-image-formats")]
pub(super) fn resolve_jbig2_globals_from_dict(
    parms: &HashMap<String, PDFObject>,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> Option<Vec<u8>> {
    let globals_obj = parms.get("JBIG2Globals")?;
    match globals_obj {
        PDFObject::Stream { dict, data } => {
            if let Some(filter) = dict.get("Filter") {
                crate::core::decode::apply_filters(data, filter).ok()
            } else {
                Some(data.clone())
            }
        }
        PDFObject::Ref(r) => {
            let res = resolver.as_mut()?;
            let resolved = res.resolve_ref(r.num, r.generation).ok()?;
            match &*resolved {
                PDFObject::Stream { dict, data } => {
                    if let Some(filter) = dict.get("Filter") {
                        crate::core::decode::apply_filters(data, filter).ok()
                    } else {
                        Some(data.clone())
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(feature = "hayro-ccitt")]
pub(super) fn ccitt_decode_parms(dict: &HashMap<String, PDFObject>) -> Option<&PDFObject> {
    let decode_parms = dict.get("DecodeParms").or_else(|| dict.get("DP"))?;
    match decode_parms {
        PDFObject::Dictionary(_) => Some(decode_parms),
        PDFObject::Array(parms) => {
            let index = ccitt_filter_index(dict.get("Filter").or_else(|| dict.get("F")))?;
            parms.get(index).and_then(|item| match item.as_ref() {
                PDFObject::Dictionary(_) => Some(item.as_ref()),
                _ => None,
            })
        }
        _ => None,
    }
}

#[cfg(feature = "hayro-ccitt")]
pub(super) fn ccitt_filter_index(filter: Option<&PDFObject>) -> Option<usize> {
    match filter? {
        PDFObject::Name(name) => {
            if matches!(name.as_str(), "CCITTFaxDecode" | "CCF") {
                Some(0)
            } else {
                None
            }
        }
        PDFObject::Array(filters) => filters.iter().position(|filter| {
            matches!(
                filter.as_ref(),
                PDFObject::Name(name) if matches!(name.as_str(), "CCITTFaxDecode" | "CCF")
            )
        }),
        _ => None,
    }
}

#[cfg(feature = "hayro-ccitt")]
pub(super) fn num_in(p: Option<&PDFObject>, key: &str) -> Option<i64> {
    p.and_then(|p| match p {
        PDFObject::Dictionary(d) => Some(d),
        _ => None,
    })?
    .get(key)
    .and_then(|v| match v {
        PDFObject::Number(n) => Some(*n as i64),
        _ => None,
    })
}

#[cfg(feature = "hayro-ccitt")]
pub(super) fn bool_in(p: Option<&PDFObject>, key: &str) -> Option<bool> {
    p.and_then(|p| match p {
        PDFObject::Dictionary(d) => Some(d),
        _ => None,
    })?
    .get(key)
    .and_then(|v| match v {
        PDFObject::Boolean(b) => Some(*b),
        _ => None,
    })
}

impl<'a, D: Device> RenderingContext<'a, D> {
    /// Create a new rendering context.
    ///
    /// # Arguments
    /// * `device` - The rendering device to use
    pub fn new(device: &'a mut D) -> Self {
        RenderingContext {
            device,
            state_stack: vec![GraphicsState::default()],
            root_ctm_stack: vec![GraphicsState::default().ctm],
            current_path: Path::new(),
            pending_clip: None,
            clip_stack: Vec::new(),
            in_text_object: false,
            resolver: None,
            resources: None,
            operation_count: 0,
            font_to_unicode: HashMap::new(),
            font_requires_cmap_remap: HashMap::new(),
            type3_fonts: HashMap::new(),
            type3_shape_color: None,
            fill_color_space: None,
            stroke_color_space: None,
            color_space_stack: Vec::new(),
            pending_fill_shading: None,
            pending_fill_tiling: None,
            pending_stroke_shading: None,
            pattern_paint_stack: Vec::new(),
            soft_mask_group_stack: Vec::new(),
            soft_mask_render_depth: 0,
            pattern_depth: 0,
            form_render_depth: 0,
            stencil_pattern_color: None,
        }
    }

    /// Maximum Form XObject nesting depth (C.3 render budget). Matches the
    /// legacy `Page::render` / text-extraction guard (`page.rs`).
    const FORM_XOBJECT_MAX_DEPTH: usize = 16;

    /// Set the object resolver and page resources for XObject/font/ext-gstate rendering.
    ///
    /// The resolver is delta-aware when a `DocStore` is supplied, so resource fetches honor
    /// delta overrides. Pass `&mut XRef` for the legacy base-only path.
    ///
    /// # Arguments
    /// * `resolver` - The object resolver for fetching resources
    /// * `resources` - The page's resources dictionary
    /// Set the object resolver and page resources for XObject/font/ext-gstate rendering.
    ///
    /// The resolver is delta-aware when a `DocStore` is supplied, so resource fetches honor
    /// delta overrides. Pass `&mut XRef` for the legacy base-only path.
    ///
    /// # Arguments
    /// * `resolver` - The object resolver for fetching resources
    /// * `resources` - The page's resources dictionary
    pub fn set_xobject_resources(
        &mut self,
        resolver: &'a mut dyn PageResolver,
        resources: &'a PDFObject,
    ) {
        self.resolver = Some(resolver);
        self.resources = Some(resources.clone());
    }

    /// Get the current graphics state.
    /// Get the current graphics state.
    pub fn current_state(&self) -> &GraphicsState {
        self.state_stack
            .last()
            .expect("Graphics state stack underflow")
    }

    /// Get mutable reference to the current graphics state.
    /// Get mutable reference to the current graphics state.
    pub fn current_state_mut(&mut self) -> &mut GraphicsState {
        self.state_stack
            .last_mut()
            .expect("Graphics state stack underflow")
    }

    /// Set the root CTM used to anchor patterns independently of content-stream CTM changes.
    /// Set the root CTM used to anchor patterns independently of content-stream CTM changes.
    pub fn set_root_ctm(&mut self, matrix: [f64; 6]) {
        if let Some(root) = self.root_ctm_stack.last_mut() {
            *root = matrix;
        } else {
            self.root_ctm_stack.push(matrix);
        }
    }
    pub(super) fn root_ctm(&self) -> [f64; 6] {
        self.root_ctm_stack
            .last()
            .copied()
            .unwrap_or_else(|| GraphicsState::default().ctm)
    }
    pub(super) fn push_root_ctm(&mut self) {
        self.root_ctm_stack.push(self.current_state().ctm);
    }
    pub(super) fn pop_root_ctm(&mut self) {
        if self.root_ctm_stack.len() > 1 {
            self.root_ctm_stack.pop();
        }
    }

    /// Get the device.
    /// Get the device.
    pub fn device(&mut self) -> &mut D {
        &mut *self.device
    }

    /// Process a content stream operation.
    ///
    /// This is the main entry point for interpreting PDF content streams.
    /// It dispatches to appropriate handler methods based on the operator.
    /// Process a content stream operation.
    ///
    /// This is the main entry point for interpreting PDF content streams.
    /// It dispatches to appropriate handler methods based on the operator.
    pub fn process_operation(&mut self, op: &Operation) -> PDFResult<()> {
        // Debug: Log first 20 operations to understand what's happening
        #[cfg(feature = "debug-logging")]
        {
            if self.operation_count < 20 {
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Operation #{}: {:?} with {} args",
                    self.operation_count,
                    op.op,
                    op.args.len()
                );
                self.operation_count += 1;
            }
        }

        if is_text_trace_enabled() {
            match op.op {
                OpCode::SetCharSpacing
                | OpCode::SetWordSpacing
                | OpCode::ShowText
                | OpCode::ShowSpacedText
                | OpCode::NextLineShowText
                | OpCode::NextLineSetSpacingShowText => {
                    eprintln!("[text-trace] op={:?} args={:?}", op.op, op.args);
                }
                _ => {}
            }
        }

        match op.op {
            // Graphics state operators
            OpCode::Save => self.save()?,
            OpCode::Restore => self.restore()?,
            OpCode::Transform => self.transform(&op.args)?,

            // Path construction operators
            OpCode::MoveTo => self.move_to(&op.args)?,
            OpCode::LineTo => self.line_to(&op.args)?,
            OpCode::CurveTo => self.curve_to(&op.args)?,
            OpCode::CurveTo2 => self.curve_to_2(&op.args)?,
            OpCode::CurveTo3 => self.curve_to_3(&op.args)?,
            OpCode::ClosePath => self.close_path()?,
            OpCode::Rectangle => self.rectangle(&op.args)?,

            // Path painting operators
            OpCode::Stroke => self.stroke()?,
            OpCode::CloseStroke => self.close_and_stroke()?,
            OpCode::Fill => self.fill(FillRule::NonZero)?,
            OpCode::EOFill => self.fill(FillRule::EvenOdd)?,
            OpCode::FillStroke => self.fill_and_stroke(FillRule::NonZero)?,
            OpCode::EOFillStroke => self.fill_and_stroke(FillRule::EvenOdd)?,
            OpCode::CloseFillStroke => self.close_fill_stroke(FillRule::NonZero)?,
            OpCode::CloseEOFillStroke => self.close_fill_stroke(FillRule::EvenOdd)?,
            OpCode::EndPath => self.end_path()?,

            // Clipping operators
            OpCode::Clip => self.clip(FillRule::NonZero)?,
            OpCode::EOClip => self.clip(FillRule::EvenOdd)?,

            // Text object operators
            OpCode::BeginText => self.begin_text()?,
            OpCode::EndText => self.end_text()?,

            // Text showing operators
            OpCode::ShowText => self.show_text(&op.args)?,
            OpCode::ShowSpacedText => self.show_spaced_text(&op.args)?,
            OpCode::NextLineShowText => self.next_line_show_text(&op.args)?,
            OpCode::NextLineSetSpacingShowText => self.next_line_set_spacing_show_text(&op.args)?,

            // Text positioning operators
            OpCode::MoveText => self.move_text(&op.args)?,
            OpCode::SetLeadingMoveText => self.set_leading_move_text(&op.args)?,
            OpCode::SetTextMatrix => self.set_text_matrix(&op.args)?,
            OpCode::NextLine => self.next_line()?,

            // Text state operators
            OpCode::SetFont => self.set_font(&op.args)?,
            OpCode::SetCharSpacing => self.set_char_spacing(&op.args)?,
            OpCode::SetWordSpacing => self.set_word_spacing(&op.args)?,
            OpCode::SetHScale => self.set_horizontal_scaling(&op.args)?,
            OpCode::SetLeading => self.set_leading(&op.args)?,
            OpCode::SetTextRenderingMode => self.set_text_rendering_mode(&op.args)?,
            OpCode::SetTextRise => self.set_text_rise(&op.args)?,

            // Color operators
            OpCode::SetStrokeGray => self.set_stroke_gray(&op.args)?,
            OpCode::SetFillGray => self.set_fill_gray(&op.args)?,
            OpCode::SetStrokeRGBColor => self.set_stroke_rgb(&op.args)?,
            OpCode::SetFillRGBColor => self.set_fill_rgb(&op.args)?,
            OpCode::SetStrokeCMYKColor => self.set_stroke_cmyk(&op.args)?,
            OpCode::SetFillCMYKColor => self.set_fill_cmyk(&op.args)?,
            OpCode::SetStrokeColorSpace => self.set_stroke_color_space(&op.args)?,
            OpCode::SetFillColorSpace => self.set_fill_color_space(&op.args)?,
            OpCode::SetStrokeColor | OpCode::SetStrokeColorN => self.set_stroke_color(&op.args)?,
            OpCode::SetFillColor | OpCode::SetFillColorN => self.set_fill_color(&op.args)?,

            // Line property operators
            OpCode::SetLineWidth => self.set_line_width(&op.args)?,
            OpCode::SetLineCap => self.set_line_cap(&op.args)?,
            OpCode::SetLineJoin => self.set_line_join(&op.args)?,
            OpCode::SetMiterLimit => self.set_miter_limit(&op.args)?,
            OpCode::SetDash => self.set_dash(&op.args)?,
            OpCode::SetGState => self.set_graphics_state(&op.args)?,

            // XObject operator
            OpCode::PaintXObject => self.paint_xobject(&op.args)?,
            OpCode::BeginInlineImage => self.paint_inline_image(&op.args)?,

            // Shading operator
            OpCode::ShadingFill => self.handle_shading_fill(&op.args)?,

            _ => {
                // Other operators not yet implemented
                // Silently skip to avoid spamming logs
                // These are typically non-critical operators like marked-content tags.
            }
        }

        Ok(())
    }

    // === Graphics State Operators ===
    pub(super) fn save(&mut self) -> PDFResult<()> {
        // Save current state
        let saved = self.current_state().save();
        self.state_stack.push(saved);
        self.color_space_stack.push((
            self.fill_color_space.clone(),
            self.stroke_color_space.clone(),
        ));
        // Pattern colour is part of the graphics state (PDF §8.4.1 / §8.7.3).
        self.pattern_paint_stack.push((
            self.pending_fill_shading.clone(),
            self.pending_fill_tiling.clone(),
            self.pending_stroke_shading.clone(),
        ));
        self.device.save_state();
        Ok(())
    }
    pub(super) fn restore(&mut self) -> PDFResult<()> {
        // Restore any clipping paths
        while self.clip_stack.len() >= self.state_stack.len() {
            self.clip_stack.pop();
            // Note: Device doesn't have a pop_clip method in our trait
            // In a full implementation, we'd pop the clip here
        }

        // Restore graphics state
        if self.state_stack.len() > 1 {
            self.state_stack.pop();
            if let Some((fill, stroke)) = self.color_space_stack.pop() {
                self.fill_color_space = fill;
                self.stroke_color_space = stroke;
            }
            if let Some((fill_sh, fill_tile, stroke_sh)) = self.pattern_paint_stack.pop() {
                self.pending_fill_shading = fill_sh;
                self.pending_fill_tiling = fill_tile;
                self.pending_stroke_shading = stroke_sh;
            }
            self.device.restore_state();
        }
        Ok(())
    }
    pub(super) fn transform(&mut self, args: &[crate::core::parser::PDFObject]) -> PDFResult<()> {
        if args.len() < 6 {
            return Err(PDFError::content_stream_error(
                "cm operator requires 6 arguments".to_string(),
            ));
        }

        let mut matrix = [0.0; 6];
        for i in 0..6 {
            if let crate::core::parser::PDFObject::Number(n) = args[i] {
                matrix[i] = n;
            } else {
                return Err(PDFError::content_stream_error(
                    "cm operator arguments must be numbers".to_string(),
                ));
            }
        }

        self.current_state_mut().concat_matrix(&matrix);
        self.device.concat_matrix(&matrix);
        Ok(())
    }

    // === Path Construction Operators ===
}
