//! A tiny-skia based rendering device.

use crate::type1_font::Type1Font;
use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image as FirImage};
use pdf_x_core::core::error::{PDFError, PDFResult};
use pdf_x_core::rendering::device::{Device, ImageData, Paint, PathDrawMode, Type0GlyphRun};
use pdf_x_core::rendering::{
    BlendMode, Color, FillRule, LineCap, LineJoin, StrokeProps, TextRenderingMode,
};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, Target};
use skrifa::{FontRef, GlyphId as SkrifaGlyphId, MetadataProvider};
use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::{
    BlendMode as SkiaBlendMode, FillRule as SkiaFillRule, FilterQuality, LineCap as SkiaLineCap,
    LineJoin as SkiaLineJoin, Mask, MaskType, Paint as SkiaPaint, Path, PathBuilder, Pixmap,
    PixmapMut, PixmapRef, Rect, Stroke, Transform,
};
use ttf_parser::OutlineBuilder;

// --- Conversion helpers ---

fn to_skia_color(color: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(color.r(), color.g(), color.b(), color.a())
}

fn to_skia_blend_mode(mode: BlendMode) -> SkiaBlendMode {
    match mode {
        BlendMode::Normal => SkiaBlendMode::SourceOver,
        BlendMode::Multiply => SkiaBlendMode::Multiply,
        BlendMode::Screen => SkiaBlendMode::Screen,
        BlendMode::Overlay => SkiaBlendMode::Overlay,
        BlendMode::Darken => SkiaBlendMode::Darken,
        BlendMode::Lighten => SkiaBlendMode::Lighten,
        BlendMode::ColorDodge => SkiaBlendMode::ColorDodge,
        BlendMode::ColorBurn => SkiaBlendMode::ColorBurn,
        BlendMode::HardLight => SkiaBlendMode::HardLight,
        BlendMode::SoftLight => SkiaBlendMode::SoftLight,
        BlendMode::Difference => SkiaBlendMode::Difference,
        BlendMode::Exclusion => SkiaBlendMode::Exclusion,
        BlendMode::Hue => SkiaBlendMode::Hue,
        BlendMode::Saturation => SkiaBlendMode::Saturation,
        BlendMode::Color => SkiaBlendMode::Color,
        BlendMode::Luminosity => SkiaBlendMode::Luminosity,
    }
}

fn to_skia_paint(paint: &Paint, blend_mode: BlendMode, opacity: f32) -> SkiaPaint {
    let mut sk_paint = SkiaPaint::default();
    match paint {
        Paint::Solid(color) => {
            let (r, g, b, a) = color.rgba();
            let adj_a = ((a as f32) * opacity) as u8;
            sk_paint.set_color(tiny_skia::Color::from_rgba8(r, g, b, adj_a));
        }
        Paint::Transparent => {
            sk_paint.set_color_rgba8(0, 0, 0, 0);
        }
    }
    sk_paint.blend_mode = to_skia_blend_mode(blend_mode);
    sk_paint.anti_alias = true;
    sk_paint
}

fn to_skia_line_cap(line_cap: LineCap) -> SkiaLineCap {
    match line_cap {
        LineCap::Butt => SkiaLineCap::Butt,
        LineCap::Round => SkiaLineCap::Round,
        LineCap::ProjectingSquare => SkiaLineCap::Square,
    }
}

fn to_skia_line_join(line_join: LineJoin) -> SkiaLineJoin {
    match line_join {
        LineJoin::Miter => SkiaLineJoin::Miter,
        LineJoin::Round => SkiaLineJoin::Round,
        LineJoin::Bevel => SkiaLineJoin::Bevel,
    }
}

fn to_skia_fill_rule(fill_rule: FillRule) -> SkiaFillRule {
    match fill_rule {
        FillRule::NonZero => SkiaFillRule::Winding,
        FillRule::EvenOdd => SkiaFillRule::EvenOdd,
    }
}

#[allow(dead_code)]
fn build_type3_mask_outline_path(image: &ImageData) -> Option<tiny_skia::Path> {
    const POINT_TYPES: [u8; 16] = [0, 2, 4, 0, 1, 0, 5, 4, 8, 10, 0, 8, 0, 2, 1, 0];

    let width = image.width as usize;
    let height = image.height as usize;
    if width == 0 || height == 0 {
        return None;
    }

    let width1 = width + 1;
    let row_bytes = (width + 7) / 8;
    let mut painted = vec![false; width * height];
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
            painted[y * width + x] = set == image.mask_paint_one;
        }
    }

    let mut points = vec![0u8; width1 * (height + 1)];
    let mut count = 0usize;

    if painted[0] {
        points[0] = 1;
        count += 1;
    }
    for j in 1..width {
        if painted[j - 1] != painted[j] {
            points[j] = if painted[j - 1] { 2 } else { 1 };
            count += 1;
        }
    }
    if painted[width - 1] {
        points[width] = 2;
        count += 1;
    }

    for i in 1..height {
        let row = i * width;
        let prev_row = (i - 1) * width;
        let point_row = i * width1;
        if painted[prev_row] != painted[row] {
            points[point_row] = if painted[row] { 1 } else { 8 };
            count += 1;
        }

        let mut sum = (if painted[row] { 4 } else { 0 }) + (if painted[prev_row] { 8 } else { 0 });
        for j in 1..width {
            sum = (sum >> 2)
                + (if painted[row + j] { 4 } else { 0 })
                + (if painted[prev_row + j] { 8 } else { 0 });
            let point_type = POINT_TYPES[sum];
            if point_type != 0 {
                points[point_row + j] = point_type;
                count += 1;
            }
        }

        if painted[prev_row + width - 1] != painted[row + width - 1] {
            points[point_row + width] = if painted[row + width - 1] { 2 } else { 4 };
            count += 1;
        }
    }

    let last_row = (height - 1) * width;
    let point_row = height * width1;
    if painted[last_row] {
        points[point_row] = 8;
        count += 1;
    }
    for j in 1..width {
        if painted[last_row + j - 1] != painted[last_row + j] {
            points[point_row + j] = if painted[last_row + j - 1] { 4 } else { 8 };
            count += 1;
        }
    }
    if painted[last_row + width - 1] {
        points[point_row + width] = 4;
        count += 1;
    }

    let steps = [
        0isize,
        width1 as isize,
        -1,
        0,
        -(width1 as isize),
        0,
        0,
        0,
        1,
    ];
    let mut pb = PathBuilder::new();
    let mut remaining = count;

    for i in 0..=height {
        let mut p = i * width1;
        let end = p + width;
        while p < end {
            while p < end && points[p] == 0 {
                p += 1;
            }
            if p == end {
                break;
            }

            let x = p % width1;
            let y = i;
            pb.move_to(x as f32 / width as f32, 1.0 - y as f32 / height as f32);

            let p0 = p;
            let mut point_type = points[p];
            loop {
                let step = steps[point_type as usize];
                loop {
                    p = p.checked_add_signed(step)?;
                    if p >= points.len() {
                        return None;
                    }
                    if points[p] != 0 {
                        break;
                    }
                }

                let pp = points[p];
                if pp != 5 && pp != 10 {
                    point_type = pp;
                    points[p] = 0;
                } else {
                    point_type = pp & (((0x33u16 * u16::from(point_type)) >> 4) as u8);
                    points[p] &= (point_type >> 2) | (point_type << 2);
                }

                let x = p % width1;
                let y = p / width1;
                pb.line_to(x as f32 / width as f32, 1.0 - y as f32 / height as f32);

                if points[p] == 0 {
                    remaining = remaining.saturating_sub(1);
                }
                if p == p0 {
                    pb.close();
                    break;
                }
            }
        }
        if remaining == 0 {
            break;
        }
    }

    pb.finish()
}

/// Average linear scale factor of a 2D affine transform (similar to MuPDF's
/// `fz_matrix_expansion`).
fn transform_expansion(t: Transform) -> f32 {
    let scale_x = (t.sx * t.sx + t.ky * t.ky).sqrt();
    let scale_y = (t.kx * t.kx + t.sy * t.sy).sqrt();
    ((scale_x + scale_y) * 0.5).max(1e-6)
}

/// Resolve a simple-font byte code to a TrueType glyph id.
///
/// PDF 1.7 §9.6.6.4: with `/Encoding`, map code → character → Unicode cmap;
/// without Encoding (common for symbolic/subset TT, Flags bit 3), use the
/// font's cmap subtables including non-Unicode (Mac 1,0 / Symbol 3,0).
/// `ttf_parser::Face::glyph_index` only walks Unicode subtables, which fails
/// for Arabic/Persian subsets that only ship a Mac cmap (e.g. BTitr, BNazanin
/// in image_mask_missing_bpc.pdf).
fn true_type_glyph_id_for_code(
    face: &ttf_parser::Face<'_>,
    code: u8,
    encoding: Option<&pdf_x_core::core::encoding::Encoding>,
) -> Option<ttf_parser::GlyphId> {
    if let Some(enc) = encoding {
        let ch = enc.char_to_unicode(code);
        if let Some(gid) = face.glyph_index(ch) {
            if gid.0 != 0 {
                return Some(gid);
            }
        }
    } else if let Some(gid) = face.glyph_index(char::from(code)) {
        if gid.0 != 0 {
            return Some(gid);
        }
    }

    // Non-Unicode / symbolic cmaps: map character code directly.
    if let Some(cmap) = face.tables().cmap {
        for subtable in cmap.subtables {
            if let Some(gid) = subtable.glyph_index(u32::from(code)) {
                if gid.0 != 0 {
                    return Some(gid);
                }
            }
        }
    }
    None
}

fn to_skia_stroke(
    stroke_props: &StrokeProps,
    transform: Transform,
    min_device_width: f32,
) -> Stroke {
    let dash = if stroke_props.dash_array.is_empty() {
        None
    } else {
        let mut intervals = Vec::with_capacity(stroke_props.dash_array.len() + 1);
        for (i, &value) in stroke_props.dash_array.iter().enumerate() {
            let mut value = value as f32;
            if i % 2 == 0 && value <= 0.000001 {
                value = 0.1;
            } else if i % 2 == 1 {
                value = value.max(0.0);
            }
            intervals.push(value);
        }
        if intervals.len() % 2 == 1 {
            intervals.push(*intervals.last().unwrap());
        }
        tiny_skia::StrokeDash::new(intervals, stroke_props.dash_offset as f32)
    };
    let mut width = stroke_props.line_width as f32;
    let scale_x = (transform.sx * transform.sx + transform.ky * transform.ky).sqrt();
    let scale_y = (transform.kx * transform.kx + transform.sy * transform.sy).sqrt();
    let scale = ((scale_x + scale_y) * 0.5).max(1e-6);
    let device_width = width * scale;
    if width > 0.0 && device_width < min_device_width {
        width = min_device_width / scale;
    }

    Stroke {
        width,
        miter_limit: stroke_props.miter_limit as f32,
        line_cap: to_skia_line_cap(stroke_props.line_cap),
        line_join: to_skia_line_join(stroke_props.line_join),
        dash,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn system_standard_font_data(base_font: &str) -> Option<Vec<u8>> {
    let clean = base_font
        .rsplit_once('+')
        .map_or(base_font, |(_, name)| name);
    let candidates: &[&str] = match clean {
        "Helvetica" | "ArialMT" | "Arial" | "LiberationSans" | "NimbusSanL-Regu" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Arial.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ],
        "Helvetica-Bold"
        | "Arial-BoldMT"
        | "Arial-Bold"
        | "Arial,Bold"
        | "LiberationSans-Bold"
        | "NimbusSanL-Bold" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Arial_Bold.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        ],
        "Helvetica-Oblique"
        | "Arial-ItalicMT"
        | "Arial-Italic"
        | "Helvetica-Italic"
        | "LiberationSans-Italic"
        | "NimbusSanL-ReguItal" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Arial_Italic.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Italic.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Italic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
        ],
        "Helvetica-BoldOblique"
        | "Arial-BoldItalicMT"
        | "Helvetica-BoldItalic"
        | "LiberationSans-BoldItalic"
        | "NimbusSanL-BoldItal" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Arial_Bold_Italic.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-BoldItalic.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-BoldItalic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf",
        ],
        "Times-Roman" | "Times" | "Times New Roman" | "TimesNewRomanPSMT" | "TimesNewRoman"
        | "TimesNewRomanPS" | "LiberationSerif" | "NimbusRomNo9L-Regu" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Times_New_Roman.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSerif-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
        ],
        "Times-Bold"
        | "TimesNewRomanPS-BoldMT"
        | "TimesNewRomanPS-Bold"
        | "TimesNewRoman-Bold"
        | "LiberationSerif-Bold"
        | "NimbusRomNo9L-Medi" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Times_New_Roman_Bold.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSerif-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
        ],
        "Times-Italic"
        | "TimesNewRomanPS-ItalicMT"
        | "TimesNewRomanPS-Italic"
        | "TimesNewRoman-Italic"
        | "LiberationSerif-Italic"
        | "NimbusRomNo9L-ReguItal" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Times_New_Roman_Italic.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSerif-Italic.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Italic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Italic.ttf",
        ],
        "Times-BoldItalic"
        | "TimesNewRomanPS-BoldItalicMT"
        | "TimesNewRomanPS-BoldItalic"
        | "TimesNewRoman-BoldItalic"
        | "LiberationSerif-BoldItalic"
        | "NimbusRomNo9L-MediItal" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Times_New_Roman_Bold_Italic.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSerif-BoldItalic.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSerif-BoldItalic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif-BoldItalic.ttf",
        ],
        "Courier" | "CourierNew" | "CourierNewPSMT" | "LiberationMono" | "NimbusMonL-Regu" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Courier_New.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        ],
        "Courier-Bold"
        | "CourierNewPS-BoldMT"
        | "CourierNew-Bold"
        | "LiberationMono-Bold"
        | "NimbusMonL-Bold" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Courier_New_Bold.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationMono-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
        ],
        "Courier-Oblique"
        | "CourierNewPS-ItalicMT"
        | "CourierNew-Italic"
        | "LiberationMono-Italic"
        | "NimbusMonL-ReguObli" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Courier_New_Italic.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationMono-Italic.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Italic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf",
        ],
        "Courier-BoldOblique"
        | "CourierNewPS-BoldItalicMT"
        | "CourierNew-BoldItalic"
        | "LiberationMono-BoldItalic"
        | "NimbusMonL-BoldObli" => &[
            "/usr/share/fonts/truetype/msttcorefonts/Courier_New_Bold_Italic.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationMono-BoldItalic.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-BoldItalic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf",
        ],
        _ => return None,
    };

    candidates.iter().find_map(|path| std::fs::read(path).ok())
}

#[cfg(target_arch = "wasm32")]
fn system_standard_font_data(_base_font: &str) -> Option<Vec<u8>> {
    None
}

/// Compose the text-space transform used for glyph drawing.
///
/// This mirrors hayro's text transform model:
/// full = text_matrix * temp(font_size, horizontal_scaling, text_rise) * scale(1 / units_per_em).
fn compose_text_transform(
    text_matrix: &[f64; 6],
    font_size: f64,
    horizontal_scaling: f64,
    text_rise: f64,
    units_per_em: f32,
) -> Transform {
    let tm_a = text_matrix[0] as f32;
    let tm_b = text_matrix[1] as f32;
    let tm_c = text_matrix[2] as f32;
    let tm_d = text_matrix[3] as f32;
    let tm_e = text_matrix[4] as f32;
    let tm_f = text_matrix[5] as f32;

    let upem = units_per_em.max(1.0);
    let font_scale = font_size as f32 / upem;
    let h_scale = horizontal_scaling as f32 / 100.0; // Tz is a percentage.
    let rise = text_rise as f32;

    // Compose text_matrix * temp(font, rise) * scale(1/upem).
    Transform::from_row(
        tm_a * font_scale * h_scale,
        tm_b * font_scale * h_scale,
        tm_c * font_scale,
        tm_d * font_scale,
        tm_c * rise + tm_e,
        tm_d * rise + tm_f,
    )
}

struct DecodedTextRun {
    text: String,
    spacing_codes: Vec<u32>,
    is_multibyte_codes: bool,
}

fn decode_utf16_pairs(pairs: impl Iterator<Item = u16>) -> DecodedTextRun {
    let mut text = String::new();
    let mut spacing_codes = Vec::new();

    for code in pairs {
        spacing_codes.push(code as u32);
        if let Some(ch) = char::from_u32(code as u32) {
            text.push(ch);
        } else {
            text.push('\u{FFFD}');
        }
    }

    DecodedTextRun {
        text,
        spacing_codes,
        is_multibyte_codes: true,
    }
}

fn looks_like_utf16be(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || bytes.len() % 2 != 0 {
        return false;
    }

    let zero_high_bytes = bytes.iter().step_by(2).filter(|&&b| b == 0).count();
    let high_count = bytes.len() / 2;

    // Heuristic for common ASCII-ish UTF-16BE text where most high bytes are zero.
    zero_high_bytes * 2 >= high_count
}

fn decode_text_run(text_bytes: &[u8]) -> DecodedTextRun {
    if text_bytes.len() >= 2 && text_bytes.len() % 2 == 0 {
        if text_bytes.starts_with(&[0xFE, 0xFF]) {
            let pairs = text_bytes[2..]
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
            return decode_utf16_pairs(pairs);
        }

        if text_bytes.starts_with(&[0xFF, 0xFE]) {
            let pairs = text_bytes[2..]
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]));
            return decode_utf16_pairs(pairs);
        }

        if looks_like_utf16be(text_bytes) {
            let pairs = text_bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
            return decode_utf16_pairs(pairs);
        }
    }

    DecodedTextRun {
        text: text_bytes.iter().map(|&b| b as char).collect(),
        spacing_codes: text_bytes.iter().map(|&b| b as u32).collect(),
        is_multibyte_codes: false,
    }
}

fn decode_be_cids(text_bytes: &[u8]) -> Vec<u16> {
    text_bytes
        .chunks(2)
        .map(|chunk| match chunk {
            [hi, lo] => u16::from_be_bytes([*hi, *lo]),
            [byte] => *byte as u16,
            _ => 0,
        })
        .collect()
}

#[derive(Clone)]
struct SkiaGraphicsState {
    transform: Transform,
    clip_mask: Option<Mask>,
    blend_mode: BlendMode,
    soft_mask: Option<Mask>,
    opacity: f32,
}

#[derive(Clone)]
struct TextDrawOverride {
    stroke_paint: Paint,
    stroke_props: StrokeProps,
    rendering_mode: TextRenderingMode,
}

impl Default for SkiaGraphicsState {
    fn default() -> Self {
        SkiaGraphicsState {
            transform: Transform::identity(),
            clip_mask: None,
            blend_mode: BlendMode::Normal,
            soft_mask: None,
            opacity: 1.0,
        }
    }
}

/// Font data storage with Arc for shared ownership
pub struct StoredFont {
    /// The font data (kept alive to satisfy 'static requirement)
    _data: Arc<Vec<u8>>,
    /// Font type (TrueType or Type1)
    font_type: FontType,
    /// Optional PDF width table (character code/CID -> width in 1/1000 text-space units).
    /// See PDF 32000-1:2008 §9.7.4.3 (`/W`, `/DW`).
    pdf_widths: Option<Vec<f32>>,
    /// Default width for CIDs/codes missing from `pdf_widths` (`/DW` or Type1 default).
    /// PDF units (thousandths of text space), not font units-per-em.
    pdf_default_width: f32,
    /// Simple-font encoding (WinAnsi/MacRoman/…) for single-byte → Unicode mapping
    /// when outlining TrueType substitutes. Type1 uses its own glyph-name encoding.
    simple_encoding: Option<pdf_x_core::core::encoding::Encoding>,
    /// Cached glyph outlines for Type1 fonts, keyed by byte code.
    /// Type1 charstring → bezier Path is expensive; caching avoids re-outlining
    /// the same glyph on every draw_text call.
    glyph_cache: HashMap<u8, Option<tiny_skia::Path>>,
    /// Cached glyph outlines for CID-keyed CFF fonts, keyed by CID.
    cid_glyph_cache: HashMap<u16, Option<tiny_skia::Path>>,
    /// Cached glyph outlines keyed by explicit TrueType glyph id.
    gid_glyph_cache: HashMap<u16, Option<tiny_skia::Path>>,
    /// Render composite text bytes as raw big-endian CIDs/GIDs.
    raw_cid_codes: bool,
    /// Whether raw CID text uses vertical writing metrics.
    cid_vertical: bool,
    /// Default vertical metrics from `/DW2`: (v1, w1).
    cid_dw2: (f32, f32),
    /// Per-CID vertical metrics from `/W2`: CID -> [w1, v1, v2].
    cid_widths2: HashMap<u16, [f32; 3]>,
    /// CIDFontType2 `/CIDToGIDMap`, indexed by CID.
    cid_to_gid_map: Option<Vec<u16>>,
    /// Native FreeType charmap target used only for CIDFontType2 charmap lookup.
    #[cfg(feature = "native-freetype")]
    native_charmap_target: freetype::ffi::FT_Encoding,
    /// Whether the embedded font actually has a FreeType charmap to query.
    #[cfg(feature = "native-freetype")]
    native_charmap_available: bool,
}

/// Enum to hold different font types
enum FontType {
    /// TrueType/OpenType font (using ttf_parser and rustybuzz)
    TrueType {
        face: ttf_parser::Face<'static>,
        buzz_face: rustybuzz::Face<'static>,
    },
    /// Type1 font (using hayro-font)
    Type1 { font: Type1Font },
}

impl StoredFont {
    /// Create a new stored font from data.
    /// Uses unsafe to extend lifetime - safe because we own the data via Arc.
    unsafe fn new(data: Vec<u8>) -> Result<Self, String> {
        // Detect font format
        if Type1Font::is_type1(&data) {
            // Try Type1 font - clone data since Type1Font takes ownership
            let font_data = data.clone();
            Type1Font::new(font_data).map(|font| StoredFont {
                _data: Arc::new(data),
                font_type: FontType::Type1 { font },
                pdf_widths: None,
                pdf_default_width: 1000.0,
                simple_encoding: None,
                glyph_cache: HashMap::new(),
                cid_glyph_cache: HashMap::new(),
                gid_glyph_cache: HashMap::new(),
                raw_cid_codes: false,
                cid_vertical: false,
                cid_dw2: (880.0, -1000.0),
                cid_widths2: HashMap::new(),
                cid_to_gid_map: None,
                #[cfg(feature = "native-freetype")]
                native_charmap_target: freetype::ffi::FT_ENCODING_UNICODE,
                #[cfg(feature = "native-freetype")]
                native_charmap_available: false,
            })
        } else {
            // Try TrueType font
            let arc_data = Arc::new(data);

            // Get a slice of the actual data (NOT the Vec struct!)
            let slice: &[u8] = &arc_data;
            let ptr = slice.as_ptr();
            let len = slice.len();

            // Extend lifetime to 'static - safe because we keep arc_data alive
            let static_slice: &'static [u8] = unsafe { std::slice::from_raw_parts(ptr, len) };

            // Try rustybuzz first (more lenient), fall back to ttf_parser
            let buzz_face = rustybuzz::Face::from_slice(static_slice, 0)
                .ok_or("Failed to create rustybuzz face")?;

            let face = ttf_parser::Face::parse(static_slice, 0)
                .map_err(|e| format!("Failed to parse font with ttf_parser: {:?}", e))?;

            Ok(StoredFont {
                _data: arc_data,
                font_type: FontType::TrueType { face, buzz_face },
                pdf_widths: None,
                pdf_default_width: 1000.0,
                simple_encoding: None,
                glyph_cache: HashMap::new(),
                cid_glyph_cache: HashMap::new(),
                gid_glyph_cache: HashMap::new(),
                raw_cid_codes: false,
                cid_vertical: false,
                cid_dw2: (880.0, -1000.0),
                cid_widths2: HashMap::new(),
                cid_to_gid_map: None,
                #[cfg(feature = "native-freetype")]
                native_charmap_target: freetype::ffi::FT_ENCODING_UNICODE,
                #[cfg(feature = "native-freetype")]
                native_charmap_available: false,
            })
        }
    }

    /// Shape text with this font (TrueType only).
    pub fn shape(&self, text: &str) -> Option<rustybuzz::GlyphBuffer> {
        match &self.font_type {
            FontType::TrueType { buzz_face, .. } => {
                let mut buffer = rustybuzz::UnicodeBuffer::new();
                buffer.push_str(text);
                buffer.guess_segment_properties();
                // PDF content streams already provide per-glyph positioning (e.g. TJ),
                // so enabling OpenType kerning/ligature features can double-apply
                // spacing and make characters look overlapped.
                let mut features = Vec::with_capacity(4);
                for feature in ["kern=0", "liga=0", "clig=0", "rlig=0"] {
                    if let Ok(parsed) = feature.parse::<rustybuzz::Feature>() {
                        features.push(parsed);
                    }
                }
                Some(rustybuzz::shape(buzz_face, &features, buffer))
            }
            FontType::Type1 { .. } => None, // Type1 doesn't use rustybuzz shaping
        }
    }

    /// Get the ttf_parser Face (TrueType only).
    pub fn face(&self) -> Option<&ttf_parser::Face> {
        match &self.font_type {
            FontType::TrueType { face, .. } => Some(face),
            FontType::Type1 { .. } => None,
        }
    }

    /// Get the Type1 font (Type1 only).
    pub fn type1_font(&self) -> Option<&Type1Font> {
        match &self.font_type {
            FontType::Type1 { font } => Some(font),
            FontType::TrueType { .. } => None,
        }
    }

    /// Check if this is a Type1 font
    pub fn is_type1(&self) -> bool {
        matches!(self.font_type, FontType::Type1 { .. })
    }

    /// Whether this font should be rendered from raw two-byte CIDs.
    pub fn uses_raw_cid_codes(&self) -> bool {
        match &self.font_type {
            FontType::Type1 { font } => self.raw_cid_codes || font.is_cid_cff(),
            FontType::TrueType { .. } => self.raw_cid_codes,
        }
    }

    /// Enable or disable raw CID/GID rendering for composite fonts.
    pub fn set_raw_cid_codes(&mut self, enabled: bool) {
        self.raw_cid_codes = enabled;
    }

    pub fn set_cid_vertical_metrics(
        &mut self,
        vertical: bool,
        dw2: (f64, f64),
        widths2: &[(u32, f64, f64, f64)],
    ) {
        self.cid_vertical = vertical;
        self.cid_dw2 = (dw2.0 as f32, dw2.1 as f32);
        self.cid_widths2.clear();
        for &(cid, w1, v1, v2) in widths2 {
            if cid <= u16::MAX as u32 {
                self.cid_widths2
                    .insert(cid as u16, [w1 as f32, v1 as f32, v2 as f32]);
            }
        }
    }

    pub fn set_cid_to_gid_map(&mut self, map: &[u16]) {
        self.cid_to_gid_map = Some(map.to_vec());
    }

    #[cfg(feature = "native-freetype")]
    pub fn select_native_charmap(
        &mut self,
        encoding: Option<&pdf_x_core::core::parser::PDFObject>,
    ) {
        self.native_charmap_target = native_charmap_target(encoding);
        self.native_charmap_available =
            native_freetype_has_charmap(&self._data, self.native_charmap_target);
    }

    pub fn cid_vertical(&self) -> bool {
        self.cid_vertical
    }

    fn cid_vertical_advance(&self, cid: u16) -> f32 {
        self.cid_widths2
            .get(&cid)
            .map(|metrics| metrics[0])
            .unwrap_or(self.cid_dw2.1)
    }

    fn cid_vertical_origin(&self, cid: u16, horizontal_width: f32) -> (f32, f32) {
        // PDF / W2 position vector (v1, v2); hayro uses (-v1, -v2).
        // Default without W2: (-w0/2, -DW2.v1).
        if let Some([_, v1, v2]) = self.cid_widths2.get(&cid).copied() {
            (-v1, -v2)
        } else {
            (-horizontal_width / 2.0, -self.cid_dw2.0)
        }
    }

    fn cid_to_gid(&self, cid: u16) -> u16 {
        self.cid_to_gid_map
            .as_ref()
            .and_then(|map| map.get(cid as usize).copied())
            .or_else(|| self.native_cid_to_gid(cid))
            .unwrap_or(cid)
    }

    #[cfg(feature = "native-freetype")]
    fn native_cid_to_gid(&self, cid: u16) -> Option<u16> {
        native_freetype_char_index(&self._data, self.native_charmap_target, cid as usize)
            .and_then(|gid| u16::try_from(gid).ok())
    }

    #[cfg(not(feature = "native-freetype"))]
    fn native_cid_to_gid(&self, _cid: u16) -> Option<u16> {
        None
    }

    /// Get a cached glyph outline for a Type1 font byte code.
    /// Returns a cloned Path (cheap — tiny_skia::Path is internally Arc'd).
    /// On first access, outlines the glyph via hayro-font and caches the result.
    pub fn cached_glyph_path(&mut self, byte: u8) -> Option<tiny_skia::Path> {
        // Check cache first.
        if let Some(cached) = self.glyph_cache.get(&byte) {
            return cached.clone();
        }
        // Outline the glyph and cache the result.
        let path = if let FontType::Type1 { font } = &self.font_type {
            let ch = char::from(byte);
            let mut builder = PathBuilder::new();
            if font.outline_glyph_char(ch, &mut builder).is_some() {
                builder.finish()
            } else {
                None
            }
        } else {
            None
        };
        self.glyph_cache.insert(byte, path.clone());
        path
    }

    /// Get a cached glyph outline for a CID-keyed CFF font.
    pub fn cached_cid_glyph_path(&mut self, cid: u16) -> Option<tiny_skia::Path> {
        if let Some(cached) = self.cid_glyph_cache.get(&cid) {
            return cached.clone();
        }

        let path = match &self.font_type {
            FontType::Type1 { font } => {
                // CID-keyed CFF: map via charset only (`glyph_index_by_cid`).
                // Do NOT fall back to treating the CID as a raw GID — empty
                // glyphs (space, CID 21 on Antenna-Bold) would then paint the
                // unrelated glyph at that index (e.g. GID 21 = "S"), scrambling
                // headlines such as tile_clamp_bug.pdf "24. JULI 2025".
                // Non-CID CFF/Type1C under Identity-H uses CID==GID (PDF default).
                // Source: hayro Type0Font::map_code + PDF 32000-1 §9.7.4.2.
                let mut builder = PathBuilder::new();
                let ok = if font.is_cid_cff() {
                    font.outline_glyph_cid(cid, &mut builder).is_some()
                } else {
                    font.outline_glyph_cid(cid, &mut builder).is_some()
                        || font.outline_glyph_id(cid, &mut builder).is_some()
                };
                if ok { builder.finish() } else { None }
            }
            FontType::TrueType { face, .. } => {
                // Prefer explicit /CIDToGIDMap, then native charmap fallback for
                // non-identity fonts. When raw CID codes are enabled (Identity
                // CID→GID, including the PDF default when the map is omitted),
                // do NOT treat CID as a Unicode codepoint — that mis-maps subset
                // TT CIDFonts such as Gargi under Identity-H.
                // Reference: PDF 32000-1:2008 §9.7.4.2 (CIDToGIDMap default Identity).
                let gid = if self.raw_cid_codes {
                    self.cid_to_gid(cid)
                } else {
                    self.cid_to_gid_map
                        .as_ref()
                        .and_then(|map| map.get(cid as usize).copied())
                        .or_else(|| self.native_cid_to_gid(cid))
                        .or_else(|| {
                            char::from_u32(cid as u32)
                                .and_then(|ch| face.glyph_index(ch))
                                .map(|gid| gid.0)
                        })
                        .unwrap_or(cid)
                };
                let n_glyphs = face.number_of_glyphs();
                if gid == 0 || gid >= n_glyphs {
                    None
                } else {
                    // Prefer skrifa (1000-em space, with TT interpreter when
                    // required); fall back to ttf_parser scaled into the same
                    // 1000-em space so placement can always use upem=1000.
                    let font_bytes = self._data.as_ref();
                    let face_upem = face.units_per_em() as f32;
                    outline_truetype_glyph_skrifa(font_bytes, gid)
                        .or_else(|| outline_truetype_glyph_ttf_parser_1000(face, gid, face_upem))
                }
            }
        };

        self.cid_glyph_cache.insert(cid, path.clone());
        path
    }

    /// Get a cached glyph outline by explicit glyph id.
    pub fn cached_gid_glyph_path(&mut self, glyph_id: u16) -> Option<tiny_skia::Path> {
        if let Some(cached) = self.gid_glyph_cache.get(&glyph_id) {
            return cached.clone();
        }

        let path = match &self.font_type {
            FontType::TrueType { face, .. } => {
                let n_glyphs = face.number_of_glyphs();
                if glyph_id == 0 || glyph_id >= n_glyphs {
                    None
                } else {
                    let font_bytes = self._data.as_ref();
                    let face_upem = face.units_per_em() as f32;
                    outline_truetype_glyph_skrifa(font_bytes, glyph_id).or_else(|| {
                        outline_truetype_glyph_ttf_parser_1000(face, glyph_id, face_upem)
                    })
                }
            }
            FontType::Type1 { font } => {
                let mut builder = PathBuilder::new();
                if font.outline_glyph_id(glyph_id, &mut builder).is_some() {
                    builder.finish()
                } else {
                    None
                }
            }
        };

        self.gid_glyph_cache.insert(glyph_id, path.clone());
        path
    }

    /// Set custom encoding for simple fonts.
    pub fn set_custom_encoding(&mut self, encoding: std::collections::HashMap<u8, String>) {
        if let FontType::Type1 { font } = &mut self.font_type {
            font.set_custom_encoding(encoding);
        }
    }

    /// Set PDF character widths (from /FirstChar + /Widths or CID `/W` + `/DW`).
    ///
    /// Widths are in thousandths of a unit of text space (PDF §9.7.4.3), not the
    /// TrueType units-per-em of the embedded face.
    pub fn set_pdf_widths(&mut self, first_char: u32, widths: &[f64], default_width: f64) {
        self.pdf_default_width = if default_width.is_finite() {
            default_width as f32
        } else {
            1000.0
        };

        if widths.is_empty() {
            return;
        }

        let dw = self.pdf_default_width;
        let mut table = vec![dw; first_char as usize + widths.len()];
        let start = first_char as usize;

        for (i, width) in widths.iter().enumerate() {
            let code = start + i;
            if code >= table.len() {
                table.resize(code + 1, dw);
            }

            if width.is_finite() {
                table[code] = *width as f32;
            }
        }

        self.pdf_widths = Some(table);
    }

    /// PDF glyph width for a character code or CID, in 1/1000 text-space units.
    pub fn pdf_width_for_code(&self, code: u16) -> f32 {
        self.pdf_widths
            .as_ref()
            .and_then(|w| w.get(code as usize).copied())
            .unwrap_or(self.pdf_default_width)
    }

    /// When the PDF omits `/Widths` (common for Base-14 fonts), synthesize a
    /// 256-entry PDF-unit width table from the TrueType face + simple encoding.
    ///
    /// This forces the single-byte advance path (PDF model: no OpenType shaping)
    /// instead of harfbuzz, which can still introduce micro-spacing differences
    /// even with kern/liga disabled. Liberation metrics match Adobe AFM at 1000
    /// units for Helvetica/Courier ASCII, so layout tracks mutool Base-14.
    pub fn ensure_simple_widths_from_face(&mut self) {
        if self.pdf_widths.is_some() || self.raw_cid_codes {
            return;
        }
        let FontType::TrueType { face, .. } = &self.font_type else {
            return;
        };
        let upem = face.units_per_em() as f32;
        if upem <= 0.0 {
            return;
        }
        let scale = 1000.0 / upem;
        let encoding = self.simple_encoding.clone();

        let mut table = vec![0.0f32; 256];
        for code in 0u8..=255 {
            if let Some(gid) = true_type_glyph_id_for_code(face, code, encoding.as_ref()) {
                if let Some(adv) = face.glyph_hor_advance(gid) {
                    table[code as usize] = adv as f32 * scale;
                }
            }
        }

        // Prefer space width as default; otherwise mean of non-zero advances.
        let mut default_w = table[b' ' as usize];
        if default_w <= 0.0 {
            let (sum, n) = table.iter().fold((0.0f32, 0u32), |(s, n), &w| {
                if w > 0.0 { (s + w, n + 1) } else { (s, n) }
            });
            default_w = if n > 0 { sum / n as f32 } else { 500.0 };
        }
        for w in &mut table {
            if *w <= 0.0 {
                *w = default_w;
            }
        }
        self.pdf_default_width = default_w;
        self.pdf_widths = Some(table);
    }
}

#[cfg(feature = "native-freetype")]
fn native_charmap_target(
    encoding: Option<&pdf_x_core::core::parser::PDFObject>,
) -> freetype::ffi::FT_Encoding {
    let Some(pdf_x_core::core::parser::PDFObject::Name(name)) = encoding else {
        return freetype::ffi::FT_ENCODING_UNICODE;
    };

    let name = name.as_str();
    if name.contains("GB") || name.contains("UniGB") {
        freetype::ffi::FT_ENCODING_GB2312
    } else if name.contains("Big5") || name.contains("B5") || name.contains("CNS") {
        freetype::ffi::FT_ENCODING_BIG5
    } else if name.contains("Japan")
        || name.contains("JIS")
        || name.contains("RKSJ")
        || name.contains("90ms")
        || name.contains("90pv")
        || name.contains("83pv")
    {
        freetype::ffi::FT_ENCODING_SJIS
    } else if name.contains("Korea") || name.contains("KSC") || name.contains("UniKS") {
        freetype::ffi::FT_ENCODING_JOHAB
    } else {
        freetype::ffi::FT_ENCODING_UNICODE
    }
}

#[cfg(feature = "native-freetype")]
fn native_freetype_char_index(
    data: &[u8],
    target: freetype::ffi::FT_Encoding,
    charcode: usize,
) -> Option<u32> {
    let library = freetype::Library::init().ok()?;
    let face = library.new_memory_face(data.to_vec(), 0).ok()?;
    select_freetype_charmap(&face, target);
    face.get_char_index(charcode)
}

#[cfg(feature = "native-freetype")]
fn native_freetype_has_charmap(data: &[u8], target: freetype::ffi::FT_Encoding) -> bool {
    let Some(face) = freetype::Library::init()
        .ok()
        .and_then(|library| library.new_memory_face(data.to_vec(), 0).ok())
    else {
        return false;
    };

    selected_freetype_charmap_index(&face, target).is_some()
}

#[cfg(feature = "native-freetype")]
fn select_freetype_charmap(face: &freetype::Face, target: freetype::ffi::FT_Encoding) {
    let Some(index) = selected_freetype_charmap_index(face, target) else {
        return;
    };
    let charmap = face.get_charmap(index as isize);
    let _ = face.set_charmap(&charmap);
}

#[cfg(feature = "native-freetype")]
fn selected_freetype_charmap_index(
    face: &freetype::Face,
    target: freetype::ffi::FT_Encoding,
) -> Option<i32> {
    let first_charmap_index = (face.num_charmaps() > 0).then_some(0);
    let mut unicode_charmap_index = None;

    for i in 0..face.num_charmaps() {
        let charmap = face.get_charmap(i as isize);

        if charmap.encoding() == freetype::ffi::FT_ENCODING_UNICODE {
            unicode_charmap_index = Some(i);
        }

        if charmap.encoding() == target {
            return Some(i);
        }
    }

    unicode_charmap_index.or(first_charmap_index)
}

/// Parse a PDF Encoding dictionary into a byte-to-glyph-name mapping.
///
/// This handles both named encodings (like "WinAnsiEncoding") and custom
/// encodings with a Differences array.
fn parse_encoding_dictionary(
    enc_obj: &pdf_x_core::core::parser::PDFObject,
) -> Option<std::collections::HashMap<u8, String>> {
    use pdf_x_core::core::parser::PDFObject;
    use std::collections::HashMap;

    match enc_obj {
        PDFObject::Name(name) => {
            // Named encoding - use predefined glyph name mappings
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Named encoding '{}', loading predefined mapping",
                name
            );
            Some(get_predefined_encoding(name.as_str()))
        }
        PDFObject::Dictionary(dict) => {
            // PDF 32000-1 §9.6.6.1: start from BaseEncoding (or leave sparse and
            // let the font's built-in encoding fill gaps), then apply Differences.
            // Previously Differences alone replaced the base map, so unmapped codes
            // fell through to the CFF/Type1 built-in encoding instead of BaseEncoding
            // (WinAnsi/MacRoman). Source: pdf.js `getEncoding` + Differences merge.
            let mut encoding = if let Some(PDFObject::Name(base_name)) = dict.get("BaseEncoding") {
                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: BaseEncoding '{}', loading predefined mapping",
                    base_name
                );
                get_predefined_encoding(base_name.as_str())
            } else {
                HashMap::new()
            };

            if let Some(PDFObject::Array(arr)) = dict.get("Differences") {
                #[cfg(feature = "debug-logging")]
                eprintln!("DEBUG: Parsing Differences with {} entries", arr.len());

                let mut current_code: Option<u8> = None;
                for item in arr {
                    match &**item {
                        PDFObject::Number(n) => {
                            if *n >= 0.0 && *n <= 255.0 {
                                current_code = Some(*n as u8);
                            }
                        }
                        PDFObject::Name(name) => {
                            if let Some(code) = current_code {
                                #[cfg(feature = "debug-logging")]
                                eprintln!("DEBUG: Encoding[{}] = '{}'", code, name);
                                encoding.insert(code, name.clone());
                                current_code = code.checked_add(1);
                            }
                        }
                        _ => {}
                    }
                }
            }

            if encoding.is_empty() {
                None
            } else {
                #[cfg(feature = "debug-logging")]
                eprintln!("DEBUG: Encoding map has {} entries", encoding.len());
                Some(encoding)
            }
        }
        _ => None,
    }
}

/// Predefined glyph-name tables for PDF simple encodings.
/// Source: pdf.js `src/core/encodings.js` (MacRomanEncoding / WinAnsiEncoding /
/// StandardEncoding). Used when substituting Base-14 faces for unembedded Type1.

const MAC_ROMAN_GLYPHS: &[(u8, &str)] = &[
    (0x20, "space"),
    (0x21, "exclam"),
    (0x22, "quotedbl"),
    (0x23, "numbersign"),
    (0x24, "dollar"),
    (0x25, "percent"),
    (0x26, "ampersand"),
    (0x27, "quotesingle"),
    (0x28, "parenleft"),
    (0x29, "parenright"),
    (0x2A, "asterisk"),
    (0x2B, "plus"),
    (0x2C, "comma"),
    (0x2D, "hyphen"),
    (0x2E, "period"),
    (0x2F, "slash"),
    (0x30, "zero"),
    (0x31, "one"),
    (0x32, "two"),
    (0x33, "three"),
    (0x34, "four"),
    (0x35, "five"),
    (0x36, "six"),
    (0x37, "seven"),
    (0x38, "eight"),
    (0x39, "nine"),
    (0x3A, "colon"),
    (0x3B, "semicolon"),
    (0x3C, "less"),
    (0x3D, "equal"),
    (0x3E, "greater"),
    (0x3F, "question"),
    (0x40, "at"),
    (0x41, "A"),
    (0x42, "B"),
    (0x43, "C"),
    (0x44, "D"),
    (0x45, "E"),
    (0x46, "F"),
    (0x47, "G"),
    (0x48, "H"),
    (0x49, "I"),
    (0x4A, "J"),
    (0x4B, "K"),
    (0x4C, "L"),
    (0x4D, "M"),
    (0x4E, "N"),
    (0x4F, "O"),
    (0x50, "P"),
    (0x51, "Q"),
    (0x52, "R"),
    (0x53, "S"),
    (0x54, "T"),
    (0x55, "U"),
    (0x56, "V"),
    (0x57, "W"),
    (0x58, "X"),
    (0x59, "Y"),
    (0x5A, "Z"),
    (0x5B, "bracketleft"),
    (0x5C, "backslash"),
    (0x5D, "bracketright"),
    (0x5E, "asciicircum"),
    (0x5F, "underscore"),
    (0x60, "grave"),
    (0x61, "a"),
    (0x62, "b"),
    (0x63, "c"),
    (0x64, "d"),
    (0x65, "e"),
    (0x66, "f"),
    (0x67, "g"),
    (0x68, "h"),
    (0x69, "i"),
    (0x6A, "j"),
    (0x6B, "k"),
    (0x6C, "l"),
    (0x6D, "m"),
    (0x6E, "n"),
    (0x6F, "o"),
    (0x70, "p"),
    (0x71, "q"),
    (0x72, "r"),
    (0x73, "s"),
    (0x74, "t"),
    (0x75, "u"),
    (0x76, "v"),
    (0x77, "w"),
    (0x78, "x"),
    (0x79, "y"),
    (0x7A, "z"),
    (0x7B, "braceleft"),
    (0x7C, "bar"),
    (0x7D, "braceright"),
    (0x7E, "asciitilde"),
    (0x80, "Adieresis"),
    (0x81, "Aring"),
    (0x82, "Ccedilla"),
    (0x83, "Eacute"),
    (0x84, "Ntilde"),
    (0x85, "Odieresis"),
    (0x86, "Udieresis"),
    (0x87, "aacute"),
    (0x88, "agrave"),
    (0x89, "acircumflex"),
    (0x8A, "adieresis"),
    (0x8B, "atilde"),
    (0x8C, "aring"),
    (0x8D, "ccedilla"),
    (0x8E, "eacute"),
    (0x8F, "egrave"),
    (0x90, "ecircumflex"),
    (0x91, "edieresis"),
    (0x92, "iacute"),
    (0x93, "igrave"),
    (0x94, "icircumflex"),
    (0x95, "idieresis"),
    (0x96, "ntilde"),
    (0x97, "oacute"),
    (0x98, "ograve"),
    (0x99, "ocircumflex"),
    (0x9A, "odieresis"),
    (0x9B, "otilde"),
    (0x9C, "uacute"),
    (0x9D, "ugrave"),
    (0x9E, "ucircumflex"),
    (0x9F, "udieresis"),
    (0xA0, "dagger"),
    (0xA1, "degree"),
    (0xA2, "cent"),
    (0xA3, "sterling"),
    (0xA4, "section"),
    (0xA5, "bullet"),
    (0xA6, "paragraph"),
    (0xA7, "germandbls"),
    (0xA8, "registered"),
    (0xA9, "copyright"),
    (0xAA, "trademark"),
    (0xAB, "acute"),
    (0xAC, "dieresis"),
    (0xAD, "notequal"),
    (0xAE, "AE"),
    (0xAF, "Oslash"),
    (0xB0, "infinity"),
    (0xB1, "plusminus"),
    (0xB2, "lessequal"),
    (0xB3, "greaterequal"),
    (0xB4, "yen"),
    (0xB5, "mu"),
    (0xB6, "partialdiff"),
    (0xB7, "summation"),
    (0xB8, "product"),
    (0xB9, "pi"),
    (0xBA, "integral"),
    (0xBB, "ordfeminine"),
    (0xBC, "ordmasculine"),
    (0xBD, "Omega"),
    (0xBE, "ae"),
    (0xBF, "oslash"),
    (0xC0, "questiondown"),
    (0xC1, "exclamdown"),
    (0xC2, "logicalnot"),
    (0xC3, "radical"),
    (0xC4, "florin"),
    (0xC5, "approxequal"),
    (0xC6, "Delta"),
    (0xC7, "guillemotleft"),
    (0xC8, "guillemotright"),
    (0xC9, "ellipsis"),
    (0xCA, "space"),
    (0xCB, "Agrave"),
    (0xCC, "Atilde"),
    (0xCD, "Otilde"),
    (0xCE, "OE"),
    (0xCF, "oe"),
    (0xD0, "endash"),
    (0xD1, "emdash"),
    (0xD2, "quotedblleft"),
    (0xD3, "quotedblright"),
    (0xD4, "quoteleft"),
    (0xD5, "quoteright"),
    (0xD6, "divide"),
    (0xD7, "lozenge"),
    (0xD8, "ydieresis"),
    (0xD9, "Ydieresis"),
    (0xDA, "fraction"),
    (0xDB, "currency"),
    (0xDC, "guilsinglleft"),
    (0xDD, "guilsinglright"),
    (0xDE, "fi"),
    (0xDF, "fl"),
    (0xE0, "daggerdbl"),
    (0xE1, "periodcentered"),
    (0xE2, "quotesinglbase"),
    (0xE3, "quotedblbase"),
    (0xE4, "perthousand"),
    (0xE5, "Acircumflex"),
    (0xE6, "Ecircumflex"),
    (0xE7, "Aacute"),
    (0xE8, "Edieresis"),
    (0xE9, "Egrave"),
    (0xEA, "Iacute"),
    (0xEB, "Icircumflex"),
    (0xEC, "Idieresis"),
    (0xED, "Igrave"),
    (0xEE, "Oacute"),
    (0xEF, "Ocircumflex"),
    (0xF0, "apple"),
    (0xF1, "Ograve"),
    (0xF2, "Uacute"),
    (0xF3, "Ucircumflex"),
    (0xF4, "Ugrave"),
    (0xF5, "dotlessi"),
    (0xF6, "circumflex"),
    (0xF7, "tilde"),
    (0xF8, "macron"),
    (0xF9, "breve"),
    (0xFA, "dotaccent"),
    (0xFB, "ring"),
    (0xFC, "cedilla"),
    (0xFD, "hungarumlaut"),
    (0xFE, "ogonek"),
    (0xFF, "caron"),
];

const WIN_ANSI_GLYPHS: &[(u8, &str)] = &[
    (0x20, "space"),
    (0x21, "exclam"),
    (0x22, "quotedbl"),
    (0x23, "numbersign"),
    (0x24, "dollar"),
    (0x25, "percent"),
    (0x26, "ampersand"),
    (0x27, "quotesingle"),
    (0x28, "parenleft"),
    (0x29, "parenright"),
    (0x2A, "asterisk"),
    (0x2B, "plus"),
    (0x2C, "comma"),
    (0x2D, "hyphen"),
    (0x2E, "period"),
    (0x2F, "slash"),
    (0x30, "zero"),
    (0x31, "one"),
    (0x32, "two"),
    (0x33, "three"),
    (0x34, "four"),
    (0x35, "five"),
    (0x36, "six"),
    (0x37, "seven"),
    (0x38, "eight"),
    (0x39, "nine"),
    (0x3A, "colon"),
    (0x3B, "semicolon"),
    (0x3C, "less"),
    (0x3D, "equal"),
    (0x3E, "greater"),
    (0x3F, "question"),
    (0x40, "at"),
    (0x41, "A"),
    (0x42, "B"),
    (0x43, "C"),
    (0x44, "D"),
    (0x45, "E"),
    (0x46, "F"),
    (0x47, "G"),
    (0x48, "H"),
    (0x49, "I"),
    (0x4A, "J"),
    (0x4B, "K"),
    (0x4C, "L"),
    (0x4D, "M"),
    (0x4E, "N"),
    (0x4F, "O"),
    (0x50, "P"),
    (0x51, "Q"),
    (0x52, "R"),
    (0x53, "S"),
    (0x54, "T"),
    (0x55, "U"),
    (0x56, "V"),
    (0x57, "W"),
    (0x58, "X"),
    (0x59, "Y"),
    (0x5A, "Z"),
    (0x5B, "bracketleft"),
    (0x5C, "backslash"),
    (0x5D, "bracketright"),
    (0x5E, "asciicircum"),
    (0x5F, "underscore"),
    (0x60, "grave"),
    (0x61, "a"),
    (0x62, "b"),
    (0x63, "c"),
    (0x64, "d"),
    (0x65, "e"),
    (0x66, "f"),
    (0x67, "g"),
    (0x68, "h"),
    (0x69, "i"),
    (0x6A, "j"),
    (0x6B, "k"),
    (0x6C, "l"),
    (0x6D, "m"),
    (0x6E, "n"),
    (0x6F, "o"),
    (0x70, "p"),
    (0x71, "q"),
    (0x72, "r"),
    (0x73, "s"),
    (0x74, "t"),
    (0x75, "u"),
    (0x76, "v"),
    (0x77, "w"),
    (0x78, "x"),
    (0x79, "y"),
    (0x7A, "z"),
    (0x7B, "braceleft"),
    (0x7C, "bar"),
    (0x7D, "braceright"),
    (0x7E, "asciitilde"),
    (0x7F, "bullet"),
    (0x80, "Euro"),
    (0x81, "bullet"),
    (0x82, "quotesinglbase"),
    (0x83, "florin"),
    (0x84, "quotedblbase"),
    (0x85, "ellipsis"),
    (0x86, "dagger"),
    (0x87, "daggerdbl"),
    (0x88, "circumflex"),
    (0x89, "perthousand"),
    (0x8A, "Scaron"),
    (0x8B, "guilsinglleft"),
    (0x8C, "OE"),
    (0x8D, "bullet"),
    (0x8E, "Zcaron"),
    (0x8F, "bullet"),
    (0x90, "bullet"),
    (0x91, "quoteleft"),
    (0x92, "quoteright"),
    (0x93, "quotedblleft"),
    (0x94, "quotedblright"),
    (0x95, "bullet"),
    (0x96, "endash"),
    (0x97, "emdash"),
    (0x98, "tilde"),
    (0x99, "trademark"),
    (0x9A, "scaron"),
    (0x9B, "guilsinglright"),
    (0x9C, "oe"),
    (0x9D, "bullet"),
    (0x9E, "zcaron"),
    (0x9F, "Ydieresis"),
    (0xA0, "space"),
    (0xA1, "exclamdown"),
    (0xA2, "cent"),
    (0xA3, "sterling"),
    (0xA4, "currency"),
    (0xA5, "yen"),
    (0xA6, "brokenbar"),
    (0xA7, "section"),
    (0xA8, "dieresis"),
    (0xA9, "copyright"),
    (0xAA, "ordfeminine"),
    (0xAB, "guillemotleft"),
    (0xAC, "logicalnot"),
    (0xAD, "hyphen"),
    (0xAE, "registered"),
    (0xAF, "macron"),
    (0xB0, "degree"),
    (0xB1, "plusminus"),
    (0xB2, "twosuperior"),
    (0xB3, "threesuperior"),
    (0xB4, "acute"),
    (0xB5, "mu"),
    (0xB6, "paragraph"),
    (0xB7, "periodcentered"),
    (0xB8, "cedilla"),
    (0xB9, "onesuperior"),
    (0xBA, "ordmasculine"),
    (0xBB, "guillemotright"),
    (0xBC, "onequarter"),
    (0xBD, "onehalf"),
    (0xBE, "threequarters"),
    (0xBF, "questiondown"),
    (0xC0, "Agrave"),
    (0xC1, "Aacute"),
    (0xC2, "Acircumflex"),
    (0xC3, "Atilde"),
    (0xC4, "Adieresis"),
    (0xC5, "Aring"),
    (0xC6, "AE"),
    (0xC7, "Ccedilla"),
    (0xC8, "Egrave"),
    (0xC9, "Eacute"),
    (0xCA, "Ecircumflex"),
    (0xCB, "Edieresis"),
    (0xCC, "Igrave"),
    (0xCD, "Iacute"),
    (0xCE, "Icircumflex"),
    (0xCF, "Idieresis"),
    (0xD0, "Eth"),
    (0xD1, "Ntilde"),
    (0xD2, "Ograve"),
    (0xD3, "Oacute"),
    (0xD4, "Ocircumflex"),
    (0xD5, "Otilde"),
    (0xD6, "Odieresis"),
    (0xD7, "multiply"),
    (0xD8, "Oslash"),
    (0xD9, "Ugrave"),
    (0xDA, "Uacute"),
    (0xDB, "Ucircumflex"),
    (0xDC, "Udieresis"),
    (0xDD, "Yacute"),
    (0xDE, "Thorn"),
    (0xDF, "germandbls"),
    (0xE0, "agrave"),
    (0xE1, "aacute"),
    (0xE2, "acircumflex"),
    (0xE3, "atilde"),
    (0xE4, "adieresis"),
    (0xE5, "aring"),
    (0xE6, "ae"),
    (0xE7, "ccedilla"),
    (0xE8, "egrave"),
    (0xE9, "eacute"),
    (0xEA, "ecircumflex"),
    (0xEB, "edieresis"),
    (0xEC, "igrave"),
    (0xED, "iacute"),
    (0xEE, "icircumflex"),
    (0xEF, "idieresis"),
    (0xF0, "eth"),
    (0xF1, "ntilde"),
    (0xF2, "ograve"),
    (0xF3, "oacute"),
    (0xF4, "ocircumflex"),
    (0xF5, "otilde"),
    (0xF6, "odieresis"),
    (0xF7, "divide"),
    (0xF8, "oslash"),
    (0xF9, "ugrave"),
    (0xFA, "uacute"),
    (0xFB, "ucircumflex"),
    (0xFC, "udieresis"),
    (0xFD, "yacute"),
    (0xFE, "thorn"),
    (0xFF, "ydieresis"),
];

const STANDARD_ENCODING_GLYPHS: &[(u8, &str)] = &[
    (0x20, "space"),
    (0x21, "exclam"),
    (0x22, "quotedbl"),
    (0x23, "numbersign"),
    (0x24, "dollar"),
    (0x25, "percent"),
    (0x26, "ampersand"),
    (0x27, "quoteright"),
    (0x28, "parenleft"),
    (0x29, "parenright"),
    (0x2A, "asterisk"),
    (0x2B, "plus"),
    (0x2C, "comma"),
    (0x2D, "hyphen"),
    (0x2E, "period"),
    (0x2F, "slash"),
    (0x30, "zero"),
    (0x31, "one"),
    (0x32, "two"),
    (0x33, "three"),
    (0x34, "four"),
    (0x35, "five"),
    (0x36, "six"),
    (0x37, "seven"),
    (0x38, "eight"),
    (0x39, "nine"),
    (0x3A, "colon"),
    (0x3B, "semicolon"),
    (0x3C, "less"),
    (0x3D, "equal"),
    (0x3E, "greater"),
    (0x3F, "question"),
    (0x40, "at"),
    (0x41, "A"),
    (0x42, "B"),
    (0x43, "C"),
    (0x44, "D"),
    (0x45, "E"),
    (0x46, "F"),
    (0x47, "G"),
    (0x48, "H"),
    (0x49, "I"),
    (0x4A, "J"),
    (0x4B, "K"),
    (0x4C, "L"),
    (0x4D, "M"),
    (0x4E, "N"),
    (0x4F, "O"),
    (0x50, "P"),
    (0x51, "Q"),
    (0x52, "R"),
    (0x53, "S"),
    (0x54, "T"),
    (0x55, "U"),
    (0x56, "V"),
    (0x57, "W"),
    (0x58, "X"),
    (0x59, "Y"),
    (0x5A, "Z"),
    (0x5B, "bracketleft"),
    (0x5C, "backslash"),
    (0x5D, "bracketright"),
    (0x5E, "asciicircum"),
    (0x5F, "underscore"),
    (0x60, "quoteleft"),
    (0x61, "a"),
    (0x62, "b"),
    (0x63, "c"),
    (0x64, "d"),
    (0x65, "e"),
    (0x66, "f"),
    (0x67, "g"),
    (0x68, "h"),
    (0x69, "i"),
    (0x6A, "j"),
    (0x6B, "k"),
    (0x6C, "l"),
    (0x6D, "m"),
    (0x6E, "n"),
    (0x6F, "o"),
    (0x70, "p"),
    (0x71, "q"),
    (0x72, "r"),
    (0x73, "s"),
    (0x74, "t"),
    (0x75, "u"),
    (0x76, "v"),
    (0x77, "w"),
    (0x78, "x"),
    (0x79, "y"),
    (0x7A, "z"),
    (0x7B, "braceleft"),
    (0x7C, "bar"),
    (0x7D, "braceright"),
    (0x7E, "asciitilde"),
    (0xA1, "exclamdown"),
    (0xA2, "cent"),
    (0xA3, "sterling"),
    (0xA4, "fraction"),
    (0xA5, "yen"),
    (0xA6, "florin"),
    (0xA7, "section"),
    (0xA8, "currency"),
    (0xA9, "quotesingle"),
    (0xAA, "quotedblleft"),
    (0xAB, "guillemotleft"),
    (0xAC, "guilsinglleft"),
    (0xAD, "guilsinglright"),
    (0xAE, "fi"),
    (0xAF, "fl"),
    (0xB1, "endash"),
    (0xB2, "dagger"),
    (0xB3, "daggerdbl"),
    (0xB4, "periodcentered"),
    (0xB6, "paragraph"),
    (0xB7, "bullet"),
    (0xB8, "quotesinglbase"),
    (0xB9, "quotedblbase"),
    (0xBA, "quotedblright"),
    (0xBB, "guillemotright"),
    (0xBC, "ellipsis"),
    (0xBD, "perthousand"),
    (0xBF, "questiondown"),
    (0xC1, "grave"),
    (0xC2, "acute"),
    (0xC3, "circumflex"),
    (0xC4, "tilde"),
    (0xC5, "macron"),
    (0xC6, "breve"),
    (0xC7, "dotaccent"),
    (0xC8, "dieresis"),
    (0xCA, "ring"),
    (0xCB, "cedilla"),
    (0xCD, "hungarumlaut"),
    (0xCE, "ogonek"),
    (0xCF, "caron"),
    (0xD0, "emdash"),
    (0xE1, "AE"),
    (0xE3, "ordfeminine"),
    (0xE8, "Lslash"),
    (0xE9, "Oslash"),
    (0xEA, "OE"),
    (0xEB, "ordmasculine"),
    (0xF1, "ae"),
    (0xF5, "dotlessi"),
    (0xF8, "lslash"),
    (0xF9, "oslash"),
    (0xFA, "oe"),
    (0xFB, "germandbls"),
];

/// Get predefined byte-to-glyph-name mapping for standard PDF encodings.
///
/// This provides glyph name mappings for common PDF encodings like
/// WinAnsiEncoding, StandardEncoding, MacRomanEncoding, etc.
fn get_predefined_encoding(name: &str) -> HashMap<u8, String> {
    let table: &[(u8, &str)] = match name {
        "MacRomanEncoding" => MAC_ROMAN_GLYPHS,
        "StandardEncoding" => STANDARD_ENCODING_GLYPHS,
        // WinAnsiEncoding and unknown names: PDF viewers often fall back to WinAnsi
        // for simple text. Keep that default for robustness.
        _ => WIN_ANSI_GLYPHS,
    };
    let mut encoding = HashMap::new();
    for &(code, glyph) in table {
        encoding.insert(code, glyph.to_string());
    }
    #[cfg(feature = "debug-logging")]
    eprintln!(
        "DEBUG: predefined encoding '{}' with {} entries",
        name,
        encoding.len()
    );
    encoding
}

pub struct SkiaDevice<'a> {
    pixmap: PixmapMut<'a>,
    state_stack: Vec<SkiaGraphicsState>,
    path_builder: PathBuilder,
    pending_text_clip: Option<PathBuilder>,
    /// Font cache — borrowed from PDFDocument's font_cache so parsed fonts
    /// persist across renders. Keyed by resource name (e.g. "F1").
    font_cache: &'a mut HashMap<String, StoredFont>,
    draw_count: usize,
    colors_seen: std::collections::HashMap<String, usize>,
    /// Saved pixel data stack during offscreen rendering (for nested soft masks).
    offscreen_stack: Vec<Vec<u8>>,
    type3_glyph_depth: usize,
    standard_font_resolver: Option<fn(&str) -> Option<Vec<u8>>>,
    text_draw_override: Option<TextDrawOverride>,
}

struct PathConverter(PathBuilder);

impl OutlineBuilder for PathConverter {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.0.close();
    }
}

/// skrifa outline sink (same coordinate space as ttf_parser OutlineBuilder).
impl OutlinePen for PathConverter {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to(cx0, cy0, x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.cubic_to(cx0, cy0, cx1, cy1, x, y);
    }

    fn close(&mut self) {
        self.0.close();
    }
}

/// Hayro/PDF text-space em for outline placement (§9.2.4 / hayro UNITS_PER_EM).
const OUTLINE_UPEM: f32 = 1000.0;

/// ttf_parser outline scaled from face design units into 1000-em space so it
/// shares the same placement transform as skrifa outlines.
fn outline_truetype_glyph_ttf_parser_1000(
    face: &ttf_parser::Face<'_>,
    glyph_id: u16,
    face_upem: f32,
) -> Option<Path> {
    let mut converter = PathConverter(PathBuilder::new());
    let _ = face.outline_glyph(ttf_parser::GlyphId(glyph_id), &mut converter);
    let path = converter.0.finish()?;
    let upem = face_upem.max(1.0);
    if (upem - OUTLINE_UPEM).abs() < 0.5 {
        Some(path)
    } else {
        path.transform(Transform::from_scale(
            OUTLINE_UPEM / upem,
            OUTLINE_UPEM / upem,
        ))
    }
}

/// Outline a TrueType/CFF-in-OT glyph via skrifa, with TT interpreter when the
/// font requires it (hint-reliant fonts). Unhinted ttf_parser alone produces
/// wrong CJK composites for some CID TrueType subsets (pdftc_900k_0319 MingLiU).
///
/// Coordinates are returned in **hayro's 1000-unit em space** (not the face's
/// native `units_per_em`). Callers that place these paths must compose with
/// `compose_text_transform(..., units_per_em = 1000)` so the net scale is
/// `font_size/1000`, matching hayro (`UNITS_PER_EM = 1000`).
///
/// Source: hayro/hayro-interpret/src/font/blob.rs OpenTypeFontBlob::outline_glyph.
fn outline_truetype_glyph_skrifa(data: &[u8], glyph_id: u16) -> Option<Path> {
    let font_ref = FontRef::from_index(data, 0).ok()?;
    let outlines = font_ref.outline_glyphs();
    let outline = outlines.get(SkrifaGlyphId::new(u32::from(glyph_id)))?;

    // Match hayro: always outline at Size(1000); path stays in 1000-em coords.
    // Source: hayro UNITS_PER_EM = 1000 in font/blob.rs / font/mod.rs.
    let size = Size::new(OUTLINE_UPEM);

    let mut converter = PathConverter(PathBuilder::new());

    // Hayro enables the TT interpreter when `require_interpreter()` is set —
    // some fonts render incorrect path geometry without it (not just worse AA).
    // Source: hayro OpenTypeFontBlob::new / outline_glyph.
    let drew = if outlines.require_interpreter() {
        if let Ok(instance) = HintingInstance::new(
            &outlines,
            size,
            LocationRef::default(),
            HintingOptions {
                engine: Engine::Interpreter,
                target: Target::Mono,
            },
        ) {
            outline
                .draw(DrawSettings::hinted(&instance, false), &mut converter)
                .is_ok()
        } else {
            outline
                .draw(
                    DrawSettings::unhinted(size, LocationRef::default()),
                    &mut converter,
                )
                .is_ok()
        }
    } else {
        outline
            .draw(
                DrawSettings::unhinted(size, LocationRef::default()),
                &mut converter,
            )
            .is_ok()
    };
    if !drew {
        return None;
    }
    converter.0.finish()
}

impl<'a> SkiaDevice<'a> {
    pub fn new(pixmap: PixmapMut<'a>, font_cache: &'a mut HashMap<String, StoredFont>) -> Self {
        SkiaDevice {
            pixmap,
            state_stack: vec![SkiaGraphicsState::default()],
            path_builder: PathBuilder::new(),
            pending_text_clip: None,
            font_cache,
            draw_count: 0,
            colors_seen: std::collections::HashMap::new(),
            offscreen_stack: Vec::new(),
            type3_glyph_depth: 0,
            standard_font_resolver: None,
            text_draw_override: None,
        }
    }

    /// Enable native standard-font lookup for non-embedded Base14 fonts.
    ///
    /// This is intentionally adapter-side: core remains deterministic and WASM-clean,
    /// while native callers can opt into platform fonts for PDFium-like behavior.
    pub fn use_system_standard_fonts(&mut self) {
        self.standard_font_resolver = Some(system_standard_font_data);
    }

    /// Install a custom standard-font resolver.
    pub fn set_standard_font_resolver(&mut self, resolver: fn(&str) -> Option<Vec<u8>>) {
        self.standard_font_resolver = Some(resolver);
    }

    pub fn print_color_summary(&self) {
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Color summary ({} unique colors):",
            self.colors_seen.len()
        );
        let mut colors: Vec<_> = self.colors_seen.iter().collect();
        colors.sort_by(|a, b| b.1.cmp(a.1)); // Sort by count descending
        for (color, count) in colors {
            eprintln!("  {}: {} draws", color, count);
        }
    }

    /// Load a font from raw font data.
    ///
    /// # Arguments
    /// * `name` - The name to identify this font (e.g., the PDF font name)
    /// * `data` - The raw font data (TrueType, CFF, etc.)
    pub fn load_font(
        &mut self,
        name: &str,
        data: Vec<u8>,
        encoding: Option<&pdf_x_core::core::parser::PDFObject>,
    ) -> PDFResult<()> {
        // SAFETY: The StoredFont keeps the Arc<Vec<u8>> alive,
        // so the extended lifetime is safe
        let mut font = unsafe {
            StoredFont::new(data)
                .map_err(|e| PDFError::Generic(format!("Failed to load font: {}", e)))?
        };

        // Parse and set custom encoding if provided
        if let Some(enc_obj) = encoding {
            // Named / dictionary encodings for simple fonts (MacRoman, WinAnsi, …).
            font.simple_encoding = pdf_x_core::core::encoding::Encoding::from_pdf_object(enc_obj);
            if let Some(encoding_map) = parse_encoding_dictionary(enc_obj) {
                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Setting custom encoding for font '{}' with {} entries",
                    name,
                    encoding_map.len()
                );
                font.set_custom_encoding(encoding_map);
            }
        }
        #[cfg(feature = "native-freetype")]
        font.select_native_charmap(encoding);

        // Base-14 / other substitutes often omit `/Widths`. Seed PDF-unit advances
        // from the face so draw_text uses the single-byte path (not harfbuzz).
        font.ensure_simple_widths_from_face();

        self.font_cache.insert(name.to_string(), font);
        Ok(())
    }

    /// Get a font by name.
    pub fn get_font(&self, name: &str) -> Option<&StoredFont> {
        self.font_cache.get(name)
    }

    fn current_state(&self) -> &SkiaGraphicsState {
        self.state_stack.last().unwrap()
    }

    fn current_state_mut(&mut self) -> &mut SkiaGraphicsState {
        self.state_stack.last_mut().unwrap()
    }

    fn get_clip_mask(&self) -> Option<Mask> {
        let clip = self.current_state().clip_mask.clone();
        let soft = self.current_state().soft_mask.clone();
        match (clip, soft) {
            (Some(c), Some(s)) => {
                // Combine clip + soft mask: pixel-wise minimum
                if c.width() == s.width() && c.height() == s.height() {
                    let combined: Vec<u8> = c
                        .data()
                        .iter()
                        .zip(s.data().iter())
                        .map(|(&a, &b)| a.min(b))
                        .collect();
                    Mask::from_vec(
                        combined,
                        tiny_skia::IntSize::from_wh(c.width(), c.height())?,
                    )
                } else {
                    Some(c) // Fallback: use clip only if sizes mismatch
                }
            }
            (Some(c), None) => Some(c),
            (None, Some(s)) => Some(s),
            (None, None) => None,
        }
    }

    fn paint_text_path(
        &mut self,
        path: &tiny_skia::Path,
        default_paint: &Paint,
        // Path-local (font unit) → device (`text_matrix * font_scale * CTM`).
        path_to_device: Transform,
        // User space → device (CTM). PDF line width is user-space (§8.4.3.2);
        // MuPDF scales stroked-glyph linewidth by CTM expansion only, not font size.
        user_to_device: Transform,
        clip_mask: Option<&Mask>,
    ) {
        let blend_mode = self.current_state().blend_mode;
        let opacity = self.current_state().opacity;
        let fill_rule = SkiaFillRule::Winding;

        // Convert user-space line width into path-local units so that after
        // `path_to_device` the device width equals `line_width * |CTM|`.
        // path_to_device = font_to_user * CTM ⇒ local_w = user_w * |CTM| / |path_to_device|.
        let stroke_props_for_path = |props: &StrokeProps| -> StrokeProps {
            let path_scale = transform_expansion(path_to_device);
            let user_scale = transform_expansion(user_to_device);
            let mut adjusted = props.clone();
            if path_scale > 1e-6 {
                adjusted.line_width *= (user_scale / path_scale) as f64;
            }
            adjusted
        };

        if let Some(text_override) = &self.text_draw_override {
            match text_override.rendering_mode {
                TextRenderingMode::Invisible | TextRenderingMode::Clip => {}
                // Match hayro text min device width (hayro/src/renderer.rs
                // set_stroke_properties is_text threshold=0.25). Spec wants
                // ≥1 device px for paths, but that over-bolds text at 72 DPI.
                TextRenderingMode::Stroke | TextRenderingMode::StrokeClip => {
                    let sk_paint = to_skia_paint(&text_override.stroke_paint, blend_mode, opacity);
                    let props = stroke_props_for_path(&text_override.stroke_props);
                    let sk_stroke = to_skia_stroke(&props, path_to_device, 0.25);
                    self.pixmap
                        .stroke_path(path, &sk_paint, &sk_stroke, path_to_device, clip_mask);
                }
                TextRenderingMode::FillStroke | TextRenderingMode::FillStrokeClip => {
                    let fill_paint = to_skia_paint(default_paint, blend_mode, opacity);
                    self.pixmap
                        .fill_path(path, &fill_paint, fill_rule, path_to_device, clip_mask);

                    let stroke_paint =
                        to_skia_paint(&text_override.stroke_paint, blend_mode, opacity);
                    let props = stroke_props_for_path(&text_override.stroke_props);
                    let sk_stroke = to_skia_stroke(&props, path_to_device, 0.25);
                    self.pixmap.stroke_path(
                        path,
                        &stroke_paint,
                        &sk_stroke,
                        path_to_device,
                        clip_mask,
                    );
                }
                TextRenderingMode::Fill | TextRenderingMode::FillClip => {
                    let sk_paint = to_skia_paint(default_paint, blend_mode, opacity);
                    self.pixmap
                        .fill_path(path, &sk_paint, fill_rule, path_to_device, clip_mask);
                }
            }
        } else {
            let sk_paint = to_skia_paint(default_paint, blend_mode, opacity);
            self.pixmap
                .fill_path(path, &sk_paint, fill_rule, path_to_device, clip_mask);
        }
    }
}

impl<'a> Device for SkiaDevice<'a> {
    fn supports_native_cid_to_gid(&self) -> bool {
        cfg!(feature = "native-freetype")
    }

    fn font_has_native_cid_to_gid(&self, name: &str) -> bool {
        #[cfg(feature = "native-freetype")]
        {
            self.font_cache
                .get(name)
                .map(|font| font.native_charmap_available)
                .unwrap_or(false)
        }
        #[cfg(not(feature = "native-freetype"))]
        {
            let _ = name;
            false
        }
    }

    fn begin_path(&mut self) {
        self.path_builder = PathBuilder::new();
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.path_builder.move_to(x as f32, y as f32);
    }

    fn line_to(&mut self, x: f64, y: f64) {
        self.path_builder.line_to(x as f32, y as f32);
    }

    fn curve_to(&mut self, cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64) {
        self.path_builder.cubic_to(
            cp1x as f32,
            cp1y as f32,
            cp2x as f32,
            cp2y as f32,
            x as f32,
            y as f32,
        );
    }

    fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        // Handle invalid rectangles gracefully
        // PDFs may have negative or zero dimensions, or NaN values
        let xf = x as f32;
        let yf = y as f32;
        let wf = width as f32;
        let hf = height as f32;

        // Create a valid rectangle, clamping to reasonable bounds
        if wf > 0.0
            && hf > 0.0
            && xf.is_finite()
            && yf.is_finite()
            && wf.is_finite()
            && hf.is_finite()
        {
            if let Some(rect) = Rect::from_xywh(xf, yf, wf, hf) {
                self.path_builder.push_rect(rect);
                return;
            }
        }

        // Fallback: draw rectangle using lines for invalid rects
        self.move_to(x, y);
        self.line_to(x + width, y);
        self.line_to(x + width, y + height);
        self.line_to(x, y + height);
        self.close_path();
    }

    fn close_path(&mut self) {
        self.path_builder.close();
    }

    fn draw_path(
        &mut self,
        mode: PathDrawMode,
        paint: &Paint,
        stroke_props: &StrokeProps,
    ) -> PDFResult<()> {
        // Take ownership of the path_builder and create a new one
        let path = std::mem::replace(&mut self.path_builder, PathBuilder::new())
            .finish()
            .ok_or(PDFError::Generic("Invalid path".into()))?;

        let transform = self.current_state().transform;

        // Debug logging for path info
        #[cfg(feature = "debug-logging")]
        {
            if self.draw_count < 5 {
                eprintln!(
                    "DEBUG: draw_path #{} path.bounds={:?}",
                    self.draw_count,
                    path.bounds()
                );
                eprintln!(
                    "DEBUG: draw_path #{} transform={:?}",
                    self.draw_count, transform
                );
                eprintln!(
                    "DEBUG: draw_path #{} pixmap.size={}x{}",
                    self.draw_count,
                    self.pixmap.width(),
                    self.pixmap.height()
                );
            }
        }

        // Track colors
        let color_str = format!("{:?}", paint);
        *self.colors_seen.entry(color_str.clone()).or_insert(0) += 1;

        // Debug logging (first 20 draws only, always log non-white)
        #[cfg(feature = "debug-logging")]
        {
            let is_white = matches!(
                paint,
                Paint::Solid(pdf_x_core::rendering::Color::RGB(1.0, 1.0, 1.0))
            );
            if self.draw_count < 20 || !is_white {
                eprintln!(
                    "DEBUG: draw_path #{} mode={:?}, paint={}",
                    self.draw_count, mode, color_str
                );
            }

            // Milestone logging
            if self.draw_count == 100 || self.draw_count == 500 || self.draw_count == 1000 {
                eprintln!("DEBUG: {} draws completed, color summary:", self.draw_count);
                let mut colors: Vec<_> = self.colors_seen.iter().collect();
                colors.sort_by(|a, b| b.1.cmp(a.1));
                for (color, count) in colors.iter().take(5) {
                    eprintln!("  {}: {} draws", color, count);
                }
            }
        }

        self.draw_count += 1;

        let blend_mode = self.current_state().blend_mode;
        let sk_paint = to_skia_paint(paint, blend_mode, self.current_state().opacity);
        let clip_mask = self.get_clip_mask();

        match mode {
            PathDrawMode::Fill(fill_rule) => {
                self.pixmap.fill_path(
                    &path,
                    &sk_paint,
                    to_skia_fill_rule(fill_rule),
                    transform,
                    clip_mask.as_ref(),
                );
            }
            PathDrawMode::Stroke => {
                let sk_stroke = to_skia_stroke(
                    stroke_props,
                    transform,
                    stroke_props.min_device_width as f32,
                );
                self.pixmap.stroke_path(
                    &path,
                    &sk_paint,
                    &sk_stroke,
                    transform,
                    clip_mask.as_ref(),
                );
            }
            PathDrawMode::FillStroke(fill_rule) => {
                self.pixmap.fill_path(
                    &path,
                    &sk_paint,
                    to_skia_fill_rule(fill_rule),
                    transform,
                    clip_mask.as_ref(),
                );
                let sk_stroke = to_skia_stroke(
                    stroke_props,
                    transform,
                    stroke_props.min_device_width as f32,
                );
                self.pixmap.stroke_path(
                    &path,
                    &sk_paint,
                    &sk_stroke,
                    transform,
                    clip_mask.as_ref(),
                );
            }
        }

        Ok(())
    }

    fn clip_path(&mut self, rule: FillRule) -> PDFResult<()> {
        // Take ownership of the path_builder and create a new one
        let path = std::mem::replace(&mut self.path_builder, PathBuilder::new())
            .finish()
            .ok_or(PDFError::Generic("Invalid path".into()))?;

        // Create the clip mask immediately using the current CTM
        // This ensures the mask is in the correct coordinate space
        let transform = self.current_state().transform;
        let fill_rule = match rule {
            FillRule::NonZero => SkiaFillRule::Winding,
            FillRule::EvenOdd => SkiaFillRule::EvenOdd,
        };

        if let Some(mut mask) = Mask::new(self.pixmap.width(), self.pixmap.height()) {
            mask.fill_path(&path, fill_rule, true, transform);
            self.current_state_mut().clip_mask = match self.current_state().clip_mask.as_ref() {
                Some(existing) => intersect_masks(existing, &mask),
                None => Some(mask),
            };
        } else {
            // If mask creation failed, just don't clip
            self.current_state_mut().clip_mask = None;
        }

        Ok(())
    }

    fn clip_stroke_path(&mut self, stroke_props: &StrokeProps) -> PDFResult<()> {
        let path = std::mem::replace(&mut self.path_builder, PathBuilder::new())
            .finish()
            .ok_or(PDFError::Generic("Invalid path".into()))?;

        let transform = self.current_state().transform;
        let Some(mut stroke_pixmap) = Pixmap::new(self.pixmap.width(), self.pixmap.height()) else {
            self.current_state_mut().clip_mask = None;
            return Ok(());
        };

        let mut paint = SkiaPaint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;
        let stroke = to_skia_stroke(
            stroke_props,
            transform,
            stroke_props.min_device_width as f32,
        );
        stroke_pixmap.stroke_path(&path, &paint, &stroke, transform, None);

        let stroke_mask = Mask::from_pixmap(stroke_pixmap.as_ref(), MaskType::Alpha);
        self.current_state_mut().clip_mask = match self.current_state().clip_mask.as_ref() {
            Some(existing) => {
                if existing.width() == stroke_mask.width()
                    && existing.height() == stroke_mask.height()
                {
                    let combined: Vec<u8> = existing
                        .data()
                        .iter()
                        .zip(stroke_mask.data().iter())
                        .map(|(&a, &b)| a.min(b))
                        .collect();
                    Mask::from_vec(
                        combined,
                        tiny_skia::IntSize::from_wh(existing.width(), existing.height())
                            .ok_or(PDFError::Generic("Invalid clip mask size".into()))?,
                    )
                } else {
                    Some(stroke_mask)
                }
            }
            None => Some(stroke_mask),
        };

        Ok(())
    }

    fn save_state(&mut self) {
        let current_state = self.current_state().clone();
        self.state_stack.push(current_state);
    }

    fn restore_state(&mut self) {
        if self.state_stack.len() > 1 {
            self.state_stack.pop();
        }
    }

    fn concat_matrix(&mut self, matrix: &[f64; 6]) {
        let transform = Transform::from_row(
            matrix[0] as f32,
            matrix[1] as f32,
            matrix[2] as f32,
            matrix[3] as f32,
            matrix[4] as f32,
            matrix[5] as f32,
        );
        // PDF spec: CTM' = M × CTM (multiply on the LEFT)
        let new_transform = self.current_state().transform.pre_concat(transform);
        self.current_state_mut().transform = new_transform;
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: concat_matrix: [{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}] -> {:?}",
            matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5], new_transform
        );
    }

    fn set_matrix(&mut self, matrix: &[f64; 6]) {
        let new_transform = Transform::from_row(
            matrix[0] as f32,
            matrix[1] as f32,
            matrix[2] as f32,
            matrix[3] as f32,
            matrix[4] as f32,
            matrix[5] as f32,
        );
        self.current_state_mut().transform = new_transform;
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: set_matrix: [{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}] -> {:?}",
            matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5], new_transform
        );
    }

    fn set_blend_mode(&mut self, mode: BlendMode) {
        self.current_state_mut().blend_mode = mode;
    }

    fn draw_text(
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

    fn draw_image(&mut self, image: ImageData, _transform: &[f64; 6]) -> PDFResult<()> {
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

    fn begin_type3_glyph(&mut self) {
        self.type3_glyph_depth = self.type3_glyph_depth.saturating_add(1);
    }

    fn end_type3_glyph(&mut self) {
        self.type3_glyph_depth = self.type3_glyph_depth.saturating_sub(1);
    }

    fn page_bounds(&self) -> (f64, f64) {
        (self.pixmap.width() as f64, self.pixmap.height() as f64)
    }

    fn load_font_data(
        &mut self,
        name: &str,
        data: Vec<u8>,
        encoding: Option<&pdf_x_core::core::parser::PDFObject>,
    ) -> PDFResult<()> {
        // Skip re-parsing if already cached (cross-render persistence).
        if self.font_cache.contains_key(name) {
            return Ok(());
        }
        self.load_font(name, data, encoding)
    }

    fn resolve_standard_font_data(&mut self, base_font: &str) -> Option<Vec<u8>> {
        self.standard_font_resolver
            .and_then(|resolver| resolver(base_font))
    }

    fn set_font_widths(
        &mut self,
        name: &str,
        first_char: u32,
        widths: &[f64],
        default_width: f64,
    ) -> PDFResult<()> {
        if let Some(font) = self.font_cache.get_mut(name) {
            font.set_pdf_widths(first_char, widths, default_width);

            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Set PDF widths for font '{}': first_char={}, count={}, default_width={}",
                name,
                first_char,
                widths.len(),
                default_width
            );
        }

        Ok(())
    }

    fn set_font_raw_cid_codes(&mut self, name: &str, enabled: bool) -> PDFResult<()> {
        if let Some(font) = self.font_cache.get_mut(name) {
            font.set_raw_cid_codes(enabled);
        }
        Ok(())
    }

    fn has_font(&self, name: &str) -> bool {
        self.font_cache.contains_key(name)
    }

    fn font_wants_raw_cid_codes(&self, name: &str) -> bool {
        self.font_cache
            .get(name)
            .map(StoredFont::uses_raw_cid_codes)
            .unwrap_or(false)
    }

    fn font_is_vertical(&self, name: &str) -> bool {
        self.font_cache
            .get(name)
            .map(|font| font.cid_vertical())
            .unwrap_or(false)
    }

    fn set_font_cid_vertical_metrics(
        &mut self,
        name: &str,
        vertical: bool,
        dw2: (f64, f64),
        widths2: &[(u32, f64, f64, f64)],
    ) -> PDFResult<()> {
        if let Some(font) = self.font_cache.get_mut(name) {
            font.set_cid_vertical_metrics(vertical, dw2, widths2);
        }
        Ok(())
    }

    fn set_font_cid_to_gid_map(&mut self, name: &str, map: &[u16]) -> PDFResult<()> {
        if let Some(font) = self.font_cache.get_mut(name) {
            font.set_cid_to_gid_map(map);
        }
        Ok(())
    }

    fn draw_text_advance(
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

    fn draw_text_advance_with_mode(
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

    fn draw_type0_glyph_run(
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

    fn draw_type0_glyph_run_with_mode(
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

    fn clip_text(
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

    fn flush_text_clip(&mut self) -> PDFResult<()> {
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

    fn draw_shading(
        &mut self,
        shading: &pdf_x_core::rendering::shading::Shading,
        ctm: &[f64; 6],
    ) -> PDFResult<()> {
        let w = self.pixmap.width();
        let h = self.pixmap.height();
        let clip_mask = self.get_clip_mask();

        // Inverse CTM to map device pixels → shading space.
        let (a, b, c, d, e, f) = (
            ctm[0] as f32,
            ctm[1] as f32,
            ctm[2] as f32,
            ctm[3] as f32,
            ctm[4] as f32,
            ctm[5] as f32,
        );
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            return Ok(());
        }
        let inv_det = 1.0 / det;

        let clip_data = clip_mask.as_ref().map(|m| m.data());
        let opacity = self.current_state().opacity.clamp(0.0, 1.0);
        let sample_offset = if shading.uses_pixel_center_sampling() {
            0.5
        } else {
            0.0
        };

        for y in 0..h {
            for x in 0..w {
                let px = x as usize;
                let py = y as usize;
                let pixel_idx = py * w as usize + px;

                // Check clip mask
                let mask_alpha = if let Some(cd) = &clip_data {
                    let alpha = cd[pixel_idx] as f32 / 255.0;
                    if alpha <= 0.0 {
                        continue;
                    }
                    alpha
                } else {
                    1.0
                };

                // Device pixel → shading space via inverse CTM
                let dx = x as f32 + sample_offset;
                let dy = y as f32 + sample_offset;
                let sx = (d * (dx - e) - c * (dy - f)) * inv_det;
                let sy = (-b * (dx - e) + a * (dy - f)) * inv_det;

                if let Some((r, g, b_val, a_val)) = shading.sample(sx as f64, sy as f64) {
                    let offset = pixel_idx * 4;
                    let data = self.pixmap.data_mut();
                    if offset + 3 < data.len() {
                        // Alpha blend with existing pixel (source-over)
                        let alpha = (a_val as f32 / 255.0) * opacity * mask_alpha;
                        if alpha <= 0.0 {
                            continue;
                        }
                        let inv_alpha = 1.0 - alpha;
                        data[offset] = (r as f32 * alpha + data[offset] as f32 * inv_alpha)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                        data[offset + 1] = (g as f32 * alpha + data[offset + 1] as f32 * inv_alpha)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                        data[offset + 2] = (b_val as f32 * alpha
                            + data[offset + 2] as f32 * inv_alpha)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                        data[offset + 3] = ((alpha + (data[offset + 3] as f32 / 255.0) * inv_alpha)
                            * 255.0)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }

        Ok(())
    }

    fn begin_offscreen(&mut self) -> PDFResult<()> {
        self.offscreen_stack.push(self.pixmap.data_mut().to_vec());
        self.pixmap.data_mut().fill(0);
        Ok(())
    }

    fn end_offscreen_as_mask(
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

    fn end_offscreen_as_rgba_region(&mut self, width: u32, height: u32) -> Option<Vec<u8>> {
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

    fn end_offscreen_composite(&mut self, opacity: f64) -> PDFResult<()> {
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

    fn set_soft_mask(&mut self, mask: Option<(&[u8], u32, u32)>) -> PDFResult<()> {
        match mask {
            None => self.current_state_mut().soft_mask = None,
            Some((data, w, h)) => {
                if let Some(size) = tiny_skia::IntSize::from_wh(w, h) {
                    if let Some(m) = Mask::from_vec(data.to_vec(), size) {
                        self.current_state_mut().soft_mask = Some(m);
                    }
                }
            }
        }
        Ok(())
    }

    fn set_opacity(&mut self, opacity: f64) {
        self.current_state_mut().opacity = opacity as f32;
    }

    fn supports_pattern_cell_fill(&self) -> bool {
        true
    }

    fn fill_path_with_pattern_cell(
        &mut self,
        rule: FillRule,
        rgba: &[u8],
        width: u32,
        height: u32,
        _image_to_device: &[f64; 6],
        pattern_to_device: &[f64; 6],
        bbox: &[f64; 4],
        x_step: f64,
        y_step: f64,
        tile_bounds: (i32, i32, i32, i32),
    ) -> PDFResult<bool> {
        if pattern_to_device[1].abs() > 1e-6 || pattern_to_device[2].abs() > 1e-6 {
            return Ok(false);
        }
        if width == 0 || height == 0 || x_step.abs() <= f64::EPSILON || y_step.abs() <= f64::EPSILON
        {
            return Ok(false);
        }

        let path = std::mem::replace(&mut self.path_builder, PathBuilder::new())
            .finish()
            .ok_or(PDFError::Generic("Invalid path".into()))?;

        #[cfg(feature = "debug-logging")]
        trace_pattern_cell_fill(
            rgba,
            width,
            height,
            _image_to_device,
            path.bounds(),
            self.current_state().transform,
        );

        let mut path_mask = Mask::new(self.pixmap.width(), self.pixmap.height())
            .ok_or_else(|| PDFError::Generic("Failed to allocate tiling pattern mask".into()))?;
        path_mask.fill_path(
            &path,
            to_skia_fill_rule(rule),
            true,
            self.current_state().transform,
        );
        let clip_mask = self.get_clip_mask();
        let combined_mask = match clip_mask.as_ref() {
            Some(clip) => intersect_masks(&path_mask, clip),
            None => Some(path_mask),
        };

        let cell_bbox = transform_rect((bbox[0], bbox[1], bbox[2], bbox[3]), pattern_to_device);
        let left_offset = cell_bbox.0 - pattern_to_device[4];
        let top_offset = cell_bbox.1 - pattern_to_device[5];
        let mut pixmap_paint = tiny_skia::PixmapPaint::default();
        pixmap_paint.opacity = self.current_state().opacity;
        pixmap_paint.blend_mode = to_skia_blend_mode(self.current_state().blend_mode);
        pixmap_paint.quality = FilterQuality::Nearest;

        let Some(pattern_pixmap) = PixmapRef::from_bytes(rgba, width, height) else {
            return Ok(false);
        };
        let (ix0, ix1, iy0, iy1) = tile_bounds;
        let mut tile_count = 0usize;
        for ix in ix0..=ix1 {
            for iy in iy0..=iy1 {
                if tile_count >= 20_000 {
                    return Ok(true);
                }
                tile_count += 1;

                let original_x = pattern_to_device[0] * (ix as f64 * x_step) + pattern_to_device[4];
                let original_y = pattern_to_device[3] * (iy as f64 * y_step) + pattern_to_device[5];
                let draw_x = original_x + left_offset;
                let draw_y = original_y + top_offset;
                let int_x = draw_x.floor();
                let int_y = draw_y.floor();
                let transform =
                    Transform::from_translate((draw_x - int_x) as f32, (draw_y - int_y) as f32);
                self.pixmap.draw_pixmap(
                    int_x as i32,
                    int_y as i32,
                    pattern_pixmap,
                    &pixmap_paint,
                    transform,
                    combined_mask.as_ref(),
                );
            }
        }
        Ok(true)
    }
}

/// Intersect two masks (AND operation). Returns None if dimensions don't match.
fn intersect_masks(a: &Mask, b: &Mask) -> Option<Mask> {
    // Masks are 8-bit alpha. Intersect = min(a, b) per pixel.
    let a_data = a.data();
    let b_data = b.data();
    if a_data.len() != b_data.len() || a.width() != b.width() || a.height() != b.height() {
        return None;
    }
    let merged: Vec<u8> = a_data
        .iter()
        .zip(b_data.iter())
        .map(|(&x, &y)| x.min(y))
        .collect();
    let size = tiny_skia::IntSize::from_wh(a.width(), a.height())?;
    Mask::from_vec(merged, size)
}

fn transform_rect(bbox: (f64, f64, f64, f64), matrix: &[f64; 6]) -> (f64, f64, f64, f64) {
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

fn transform_point(matrix: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (
        matrix[0] * x + matrix[2] * y + matrix[4],
        matrix[1] * x + matrix[3] * y + matrix[5],
    )
}

#[cfg(feature = "debug-logging")]
fn trace_pattern_cell_fill(
    rgba: &[u8],
    width: u32,
    height: u32,
    image_to_device: &[f64; 6],
    path_bounds: Rect,
    current_transform: Transform,
) {
    let mut alpha_sum = 0u64;
    let mut color_sum = [0u64; 4];
    let mut nonzero = 0usize;
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..height {
        for x in 0..width {
            let i = (y as usize * width as usize + x as usize) * 4;
            let Some(px) = rgba.get(i..i + 4) else {
                continue;
            };
            for c in 0..4 {
                color_sum[c] += px[c] as u64;
            }
            let alpha = px[3] as u64;
            alpha_sum += alpha;
            if alpha != 0 {
                nonzero += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    let alpha_bbox = if nonzero == 0 {
        None
    } else {
        Some((min_x, min_y, max_x + 1, max_y + 1))
    };
    eprintln!(
        "[tiling-cell] size={}x{} nonzero={} alpha_sum={} rgba_sum={:?} alpha_bbox={:?}",
        width, height, nonzero, alpha_sum, color_sum, alpha_bbox
    );
    eprintln!(
        "[tiling-cell] image_to_device={:?} path_bounds={:?} current_transform={:?}",
        image_to_device, path_bounds, current_transform
    );
}

fn premultiply_rgba(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for pixel in data.chunks_exact(4) {
        let alpha = pixel[3];
        out.push(premultiply_channel(pixel[0], alpha));
        out.push(premultiply_channel(pixel[1], alpha));
        out.push(premultiply_channel(pixel[2], alpha));
        out.push(alpha);
    }
    out
}

/// Expand a device-space image transform so the painted footprint is at least
/// `min_px` device pixels on each axis (when that axis was already non-zero).
/// Used for 1×1 ImageMask hairlines stretched to sub-pixel widths/heights.
fn expand_image_mask_to_min_device_pixels(t: &mut Transform, width: u32, height: u32, min_px: f32) {
    let w = width as f32;
    let h = height as f32;
    if w <= 0.0 || h <= 0.0 || min_px <= 0.0 {
        return;
    }

    // Device-space vectors of the image X and Y edges.
    let x_len = (t.sx * t.sx + t.ky * t.ky).sqrt() * w;
    let y_len = (t.kx * t.kx + t.sy * t.sy).sqrt() * h;

    let boost_x = if x_len > 1e-6 && x_len < min_px {
        min_px / x_len
    } else {
        1.0
    };
    let boost_y = if y_len > 1e-6 && y_len < min_px {
        min_px / y_len
    } else {
        1.0
    };
    if (boost_x - 1.0).abs() < 1e-6 && (boost_y - 1.0).abs() < 1e-6 {
        return;
    }

    // Scale linear part about the image center in image pixel space.
    let cx = w * 0.5;
    let cy = h * 0.5;
    let mid_x = t.sx * cx + t.kx * cy + t.tx;
    let mid_y = t.ky * cx + t.sy * cy + t.ty;

    t.sx *= boost_x;
    t.ky *= boost_x;
    t.kx *= boost_y;
    t.sy *= boost_y;

    // Keep center fixed: M' * c + t' = mid  ⇒  t' = mid - M' * c
    t.tx = mid_x - (t.sx * cx + t.kx * cy);
    t.ty = mid_y - (t.ky * cx + t.sy * cy);
}

fn should_prefilter_downsample(data: &[u8], width: u32, height: u32, transform: Transform) -> bool {
    if width == 0 || height == 0 || data.len() < width as usize * height as usize * 4 {
        return false;
    }
    // The resize crate treats RGBA as independent channels. Resampling
    // premultiplied alpha this way creates halos around SMask edges; leave
    // transparent images to the backend sampler.
    if data
        .chunks_exact(4)
        .take(width as usize * height as usize)
        .any(|pixel| pixel[3] != 255)
    {
        return false;
    }

    downsample_scales(transform).is_some()
}

/// Type3 binary stencils allow alpha≠255; coverage is the alpha channel.
fn should_prefilter_type3_mask(width: u32, height: u32, transform: Transform) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    downsample_scales(transform).is_some()
}

fn downsample_scales(transform: Transform) -> Option<(f32, f32)> {
    let x_scale = (transform.sx * transform.sx + transform.ky * transform.ky).sqrt();
    let y_scale = (transform.kx * transform.kx + transform.sy * transform.sy).sqrt();
    if !x_scale.is_finite() || !y_scale.is_finite() || (x_scale >= 1.0 && y_scale >= 1.0) {
        return None;
    }
    Some((x_scale, y_scale))
}

fn catmull_rom_resize_rgba(
    data: Vec<u8>,
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Result<Vec<u8>, Vec<u8>> {
    if data.len() != src_width as usize * src_height as usize * 4 {
        return Err(data);
    }
    let src_image = FirImage::from_vec_u8(src_width, src_height, data, PixelType::U8x4)
        .expect("validated RGBA image buffer length");
    let mut dst_image = FirImage::new(dst_width, dst_height, PixelType::U8x4);

    let mut resizer = Resizer::new();
    resizer
        .resize(
            &src_image,
            &mut dst_image,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(
                fast_image_resize::FilterType::CatmullRom,
            )),
        )
        .map_err(|_| src_image.into_vec())?;

    Ok(dst_image.into_vec())
}

fn mupdf_simple_resize_rgba(
    data: Vec<u8>,
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Result<Vec<u8>, Vec<u8>> {
    if data.len() != src_width as usize * src_height as usize * 4 {
        return Err(data);
    }
    if src_width == dst_width && src_height == dst_height {
        return Ok(data);
    }

    let horizontal =
        mupdf_simple_resize_axis(&data, src_width, src_height, dst_width, true).ok_or(data)?;
    let vertical = mupdf_simple_resize_axis(&horizontal, dst_width, src_height, dst_height, false)
        .ok_or(horizontal)?;
    Ok(vertical)
}

fn mupdf_simple_resize_axis(
    data: &[u8],
    width: u32,
    height: u32,
    dst_len: u32,
    horizontal: bool,
) -> Option<Vec<u8>> {
    let src_len = if horizontal { width } else { height };
    if src_len == 0 || dst_len == 0 {
        return None;
    }
    if src_len == dst_len {
        return Some(data.to_vec());
    }

    let out_width = if horizontal { dst_len } else { width };
    let out_height = if horizontal { height } else { dst_len };
    let mut out = vec![0; out_width as usize * out_height as usize * 4];
    let scale = dst_len as f32 / src_len as f32;
    let window = if dst_len < src_len {
        1.0 / scale
    } else {
        scale.recip()
    };
    let f = if dst_len < src_len { scale } else { 1.0 };
    let g = if dst_len < src_len {
        1.0
    } else {
        src_len as f32 / dst_len as f32
    };

    for j in 0..dst_len {
        let centre = (j as f32 + 0.5) * src_len as f32 / dst_len as f32 - 0.5;
        let left = (centre - window).ceil() as i32;
        let right = (centre + window).floor() as i32;
        let mut weights = Vec::new();
        let mut sum = 0i32;
        for i in left..=right {
            if i < 0 || i >= src_len as i32 {
                continue;
            }
            let mut dist = j as f32 + 0.5 - ((i as f32 + 0.5) * dst_len as f32 / src_len as f32);
            dist = (dist * g).abs();
            let value = mupdf_simple_filter(dist) * f;
            let weight = (256.0 * value + 0.5) as i32;
            if weight != 0 {
                weights.push((i as u32, weight));
                sum += weight;
            }
        }
        if weights.is_empty() {
            weights.push((centre.floor().clamp(0.0, (src_len - 1) as f32) as u32, 256));
            sum = 256;
        }
        if sum != 256 {
            if let Some((_, max_weight)) = weights.iter_mut().max_by_key(|(_, weight)| *weight) {
                *max_weight += 256 - sum;
            }
        }

        let cross_len = if horizontal { height } else { width };
        for k in 0..cross_len {
            let (dst_x, dst_y) = if horizontal { (j, k) } else { (k, j) };
            for channel in 0..4usize {
                let mut value = 0i32;
                for (i, weight) in &weights {
                    let (src_x, src_y) = if horizontal { (*i, k) } else { (k, *i) };
                    let src_idx =
                        ((src_y as usize * width as usize + src_x as usize) * 4) + channel;
                    value += data[src_idx] as i32 * *weight;
                }
                let dst_idx =
                    ((dst_y as usize * out_width as usize + dst_x as usize) * 4) + channel;
                out[dst_idx] = ((value + 128) >> 8).clamp(0, 255) as u8;
            }
        }
    }

    Some(out)
}

fn mupdf_simple_filter(x: f32) -> f32 {
    if x >= 1.0 {
        0.0
    } else {
        1.0 + (2.0 * x - 3.0) * x * x
    }
}

fn mupdf_subsample_rgba(
    data: &[u8],
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Option<(Vec<u8>, u32)> {
    let mut factor = 0u32;
    while (src_width >> (factor + 1)) >= dst_width.saturating_add(2)
        && (src_height >> (factor + 1)) >= dst_height.saturating_add(2)
        && factor < 6
    {
        factor += 1;
    }
    if factor == 0 {
        return None;
    }

    let block = 1u32 << factor;
    let out_width = src_width.div_ceil(block);
    let out_height = src_height.div_ceil(block);
    let expected = src_width as usize * src_height as usize * 4;
    if data.len() < expected {
        return None;
    }

    let mut out = Vec::with_capacity(out_width as usize * out_height as usize * 4);
    for oy in 0..out_height {
        let y0 = oy * block;
        let y1 = (y0 + block).min(src_height);
        for ox in 0..out_width {
            let x0 = ox * block;
            let x1 = (x0 + block).min(src_width);
            let count = (x1 - x0) * (y1 - y0);
            for channel in 0..4usize {
                let mut sum = 0u32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let idx = ((y as usize * src_width as usize + x as usize) * 4) + channel;
                        sum += data[idx] as u32;
                    }
                }
                out.push((sum / count).min(255) as u8);
            }
        }
    }

    Some((out, factor))
}

fn should_auto_bilinear(image: &ImageData, transform: Transform) -> bool {
    if image.is_mask || image.bits_per_component == 1 || image.width == 0 || image.height == 0 {
        return false;
    }

    let x_scale = (transform.sx * transform.sx + transform.ky * transform.ky).sqrt();
    let y_scale = (transform.kx * transform.kx + transform.sy * transform.sy).sqrt();
    if !x_scale.is_finite() || !y_scale.is_finite() {
        return false;
    }

    let dest_width = (x_scale * image.width as f32).abs().round() as i32;
    let dest_height = (y_scale * image.height as f32).abs().round() as i32;
    if dest_width == 0 || dest_height == 0 {
        return false;
    }

    // Mirrors PDFium CStretchEngine::UseInterpolateBilinear.
    ((dest_height.abs() / 8) as i64)
        < (image.width as i64 * image.height as i64) / dest_width.abs() as i64
}

fn premultiply_channel(component: u8, alpha: u8) -> u8 {
    let value = component as u32 * alpha as u32 + 127;
    ((value + (value >> 8)) >> 8) as u8
}
