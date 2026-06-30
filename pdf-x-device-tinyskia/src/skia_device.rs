//! A tiny-skia based rendering device.

use crate::type1_font::Type1Font;
use pdf_x_core::core::error::{PDFError, PDFResult};
use pdf_x_core::rendering::device::{Device, ImageData, Paint, PathDrawMode};
use pdf_x_core::rendering::{BlendMode, Color, FillRule, LineCap, LineJoin, StrokeProps};
use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::{
    BlendMode as SkiaBlendMode, FillRule as SkiaFillRule, FilterQuality, LineCap as SkiaLineCap,
    LineJoin as SkiaLineJoin, Mask, Paint as SkiaPaint, PathBuilder, Pixmap, PixmapMut, Rect,
    Stroke, Transform,
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

fn to_skia_stroke(stroke_props: &StrokeProps) -> Stroke {
    let dash = if stroke_props.dash_array.len() >= 2 {
        tiny_skia::StrokeDash::new(
            stroke_props.dash_array.iter().map(|&v| v as f32).collect(),
            stroke_props.dash_offset as f32,
        )
    } else {
        None
    };
    Stroke {
        width: stroke_props.line_width as f32,
        miter_limit: stroke_props.miter_limit as f32,
        line_cap: to_skia_line_cap(stroke_props.line_cap),
        line_join: to_skia_line_join(stroke_props.line_join),
        dash,
    }
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

#[derive(Clone)]
struct SkiaGraphicsState {
    transform: Transform,
    clip_mask: Option<Mask>,
    blend_mode: BlendMode,
    soft_mask: Option<Mask>,
    opacity: f32,
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
    /// Optional PDF width table (byte code -> width in glyph space units).
    pdf_widths: Option<[f32; 256]>,
    /// Cached glyph outlines for Type1 fonts, keyed by byte code.
    /// Type1 charstring → bezier Path is expensive; caching avoids re-outlining
    /// the same glyph on every draw_text call.
    glyph_cache: HashMap<u8, Option<tiny_skia::Path>>,
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
                glyph_cache: HashMap::new(),
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
                glyph_cache: HashMap::new(),
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

    /// Set custom encoding for Type1/CFF fonts.
    pub fn set_custom_encoding(&mut self, encoding: std::collections::HashMap<u8, String>) {
        if let FontType::Type1 { font } = &mut self.font_type {
            font.set_custom_encoding(encoding);
        }
    }

    /// Set PDF character widths (from /FirstChar + /Widths).
    pub fn set_pdf_widths(&mut self, first_char: u32, widths: &[f64], default_width: f64) {
        if widths.is_empty() {
            return;
        }

        let mut table = [default_width as f32; 256];
        let start = first_char as usize;

        for (i, width) in widths.iter().enumerate() {
            let code = start + i;
            if code > u8::MAX as usize {
                break;
            }

            if width.is_finite() {
                table[code] = *width as f32;
            }
        }

        self.pdf_widths = Some(table);
    }
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
            // Check for Differences array (custom encoding)
            if let Some(diff_obj) = dict.get("Differences") {
                if let PDFObject::Array(arr) = diff_obj {
                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: Parsing custom encoding with {} entries", arr.len());

                    let mut encoding = HashMap::new();
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
                                    // Increment for next name
                                    current_code = code.checked_add(1);
                                }
                            }
                            _ => {}
                        }
                    }

                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: Parsed {} encoding entries", encoding.len());
                    return Some(encoding);
                }
            }

            // Check for BaseEncoding (can be a named encoding)
            if let Some(base_obj) = dict.get("BaseEncoding") {
                if let PDFObject::Name(base_name) = base_obj {
                    #[cfg(feature = "debug-logging")]
                    eprintln!(
                        "DEBUG: BaseEncoding '{}', loading predefined mapping",
                        base_name
                    );
                    return Some(get_predefined_encoding(base_name.as_str()));
                }
            }

            None
        }
        _ => None,
    }
}

/// Get predefined byte-to-glyph-name mapping for standard PDF encodings.
///
/// This provides glyph name mappings for common PDF encodings like
/// WinAnsiEncoding, StandardEncoding, MacRomanEncoding, etc.
fn get_predefined_encoding(name: &str) -> HashMap<u8, String> {
    let mut encoding = HashMap::new();

    // Standard PDF glyph names for common characters
    // Based on Adobe Glyph List and PDF specification
    let standard_glyphs = [
        // ASCII range (0x20-0x7E) - use direct character names
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
        // WinAnsiEncoding specific mappings (0x80-0x9F)
        (0x80, "Euro"),
        (0x81, "comma"),
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
        (0x8D, "comma"),
        (0x8E, "Zcaron"),
        (0x8F, "comma"),
        (0x90, "comma"),
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
        (0x9D, "comma"),
        (0x9E, "zcaron"),
        (0x9F, "Ydieresis"),
        // Common Latin-1 supplements (0xA0-0xFF)
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

    // Fill in ASCII range for all encodings
    for &(code, name) in &standard_glyphs {
        encoding.insert(code, name.to_string());
    }

    // Encoding-specific adjustments
    match name {
        "WinAnsiEncoding" => {
            // WinAnsiEncoding uses the mappings above (Windows ANSI / CP1252)
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: WinAnsiEncoding mapping with {} entries",
                encoding.len()
            );
        }
        "StandardEncoding" => {
            // StandardEncoding (Adobe Standard) - some differences in 0x80+ range
            // For now, use same as WinAnsi for common characters
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: StandardEncoding mapping with {} entries",
                encoding.len()
            );
        }
        "MacRomanEncoding" => {
            // MacRoman has different mappings for 0x80+ range
            // For simplicity, using common subset for now
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: MacRomanEncoding mapping with {} entries",
                encoding.len()
            );
        }
        "MacExpertEncoding" => {
            // Expert encoding for expert fonts
            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: MacExpertEncoding mapping with {} entries",
                encoding.len()
            );
        }
        _ => {
            #[cfg(feature = "debug-logging")]
            eprintln!("DEBUG: Unknown encoding '{}', using standard mapping", name);
        }
    }

    encoding
}

pub struct SkiaDevice<'a> {
    pixmap: PixmapMut<'a>,
    state_stack: Vec<SkiaGraphicsState>,
    path_builder: PathBuilder,
    /// Font cache — borrowed from PDFDocument's font_cache so parsed fonts
    /// persist across renders. Keyed by resource name (e.g. "F1").
    font_cache: &'a mut HashMap<String, StoredFont>,
    draw_count: usize,
    colors_seen: std::collections::HashMap<String, usize>,
    /// Saved pixel data stack during offscreen rendering (for nested soft masks).
    offscreen_stack: Vec<Vec<u8>>,
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

impl<'a> SkiaDevice<'a> {
    pub fn new(pixmap: PixmapMut<'a>, font_cache: &'a mut HashMap<String, StoredFont>) -> Self {
        SkiaDevice {
            pixmap,
            state_stack: vec![SkiaGraphicsState::default()],
            path_builder: PathBuilder::new(),
            font_cache,
            draw_count: 0,
            colors_seen: std::collections::HashMap::new(),
            offscreen_stack: Vec::new(),
        }
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
}

impl<'a> Device for SkiaDevice<'a> {
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
                let sk_stroke = to_skia_stroke(stroke_props);
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
                let sk_stroke = to_skia_stroke(stroke_props);
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
            mask.fill_path(&path, fill_rule, false, transform);
            self.current_state_mut().clip_mask = Some(mask);
        } else {
            // If mask creation failed, just don't clip
            self.current_state_mut().clip_mask = None;
        }

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
        let old_transform = self.current_state().transform;
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
            // Decode text bytes for shaping. Prefer UTF-16 when bytes look like
            // composite-font character codes; otherwise keep legacy Latin-1 mapping.
            let decoded_run = decode_text_run(text_bytes);
            let text = decoded_run.text.as_str();

            let font = match self.font_cache.get(font_name) {
                Some(f) => f,
                None => return Ok(0.0),
            };

            let shaped_buffer = match font.shape(&text) {
                Some(buffer) => buffer,
                None => return Ok(0.0), // Shaping failed
            };
            let glyph_infos = shaped_buffer.glyph_infos();
            let glyph_positions = shaped_buffer.glyph_positions();

            let face = font.face().expect("TrueType font should have face");
            let units_per_em = face.units_per_em() as f32;
            let units_per_em_f64 = units_per_em as f64;
            let horizontal_scale = horizontal_scaling / 100.0;
            let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
                units_per_em_f64 / font_size
            } else {
                0.0
            };

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
                let blend_mode = self.current_state().blend_mode;
                let sk_paint = to_skia_paint(paint, blend_mode, self.current_state().opacity);
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

                self.pixmap.fill_path(
                    &path,
                    &sk_paint,
                    SkiaFillRule::Winding,
                    final_transform,
                    clip_mask.as_ref(),
                );
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
        let (type1_font, pdf_widths) = {
            let font = self.font_cache.get(font_name).unwrap();
            (font.type1_font().unwrap().clone(), font.pdf_widths.clone())
        };

        // Type1 fonts use 1000 units per em
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

        for &byte in text_bytes {
            let ch = char::from(byte);
            let glyph_width = pdf_widths
                .as_ref()
                .map(|w| w[byte as usize])
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
            total_advance_user +=
                (glyph_width as f64 * (font_size / units_per_em_f64)) + spacing_user;
        }

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Text rendering: {}/{} glyphs rendered",
            glyphs_rendered, glyphs_total
        );

        let total_rendered_width = total_advance_user * horizontal_scale;

        if let Some(path) = text_path_builder.finish() {
            #[cfg(feature = "debug-logging")]
            eprintln!("DEBUG: Text path created successfully");

            let blend_mode = self.current_state().blend_mode;
            let sk_paint = to_skia_paint(paint, blend_mode, self.current_state().opacity);
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

            self.pixmap.fill_path(
                &path,
                &sk_paint,
                SkiaFillRule::Winding,
                final_transform,
                clip_mask.as_ref(),
            );

            #[cfg(feature = "debug-logging")]
            eprintln!("DEBUG: Text path drawn to pixmap");
        } else {
            #[cfg(feature = "debug-logging")]
            eprintln!("DEBUG: Failed to create text path from path builder");
        }

        Ok(total_rendered_width)
    }

    fn draw_image(&mut self, image: ImageData, transform: &[f64; 6]) -> PDFResult<()> {
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: SkiaDevice::draw_image: {}x{}, transform=[{:?}]",
            image.width, image.height, transform
        );

        // Convert image data to RGBA format if needed
        let pixel_count = (image.width * image.height) as usize;

        // Image mask: 1-bpp stencil. Set bit = mask_color, unset = transparent.
        let (data, _has_alpha) = if image.is_mask {
            let [fr, fg, fb] = image.mask_color;
            // Check for inverted decode ([1 0] means 0=paint)
            let inverted = false; // TODO: read Decode array

            let mut rgba = Vec::with_capacity(pixel_count * 4);
            let mut bit_idx = 0usize;
            for &byte in &image.data {
                for bit in (0..8).rev() {
                    let set = ((byte >> bit) & 1) != 0;
                    let paint = if inverted { !set } else { set };
                    if paint {
                        rgba.extend_from_slice(&[fr, fg, fb, 255]);
                    } else {
                        rgba.extend_from_slice(&[0, 0, 0, 0]);
                    }
                    bit_idx += 1;
                    if bit_idx >= pixel_count {
                        break;
                    }
                }
                if bit_idx >= pixel_count {
                    break;
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

                    // Convert 1-bit to RGBA
                    let mut rgba =
                        Vec::with_capacity(image.width as usize * image.height as usize * 4);
                    let mut bit_idx = 0;

                    for byte in image.data.iter() {
                        for bit in (0..8).rev() {
                            let pixel_is_set = (byte >> bit) & 1;
                            let color_value = if pixel_is_set != 0 { 0 } else { 255 };

                            rgba.push(color_value); // R
                            rgba.push(color_value); // G
                            rgba.push(color_value); // B
                            rgba.push(255); // A

                            bit_idx += 1;
                            if bit_idx >= pixel_count {
                                break;
                            }
                        }
                        if bit_idx >= pixel_count {
                            break;
                        }
                    }

                    (rgba, false)
                }
                // Already RGBA (8 bpc, 4 channels, has_alpha flag set)
                // Check this FIRST to avoid confusing RGBA with CMYK (both have 4 channels)
                (8, _) if image.has_alpha && image.data.len() >= pixel_count * 4 => {
                    (image.data.clone(), true)
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

        let int_size = tiny_skia::IntSize::from_wh(image.width, image.height)
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

        let image_transform = Transform::from_row(
            transform[0] as f32,
            transform[1] as f32,
            transform[2] as f32,
            transform[3] as f32,
            transform[4] as f32,
            transform[5] as f32,
        );

        let ctm = self.current_state().transform;

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: CTM={:?}, image_transform={:?}",
            ctm, image_transform
        );

        let clip_mask = self.get_clip_mask();

        // SIMPLE APPROACH: Use the CTM directly
        // The CTM maps the image's unit square to the page area
        // But the image pixmap is in pixel space, so we need to scale it
        //
        // Create a transform that:
        // 1. Scales the image pixmap by (1/width, 1/height) to map to unit square
        // 2. Applies the CTM to map to screen space

        let image_to_unit =
            Transform::from_scale(1.0 / image.width as f32, 1.0 / image.height as f32);

        // IMPORTANT: The transform order matters!
        // We want: final = CTM × image_to_unit
        // Which means: apply image_to_unit first, then CTM
        // In tiny-skia, post_concat means: new = old × transform
        // So we need: image_to_unit.post_concat(ctm)

        let final_transform = image_to_unit.post_concat(ctm);

        // But there's a problem: CTM has Y-flip (sy is negative), which draws the image upside down.
        // To fix this, we need to flip the Y scale and adjust the Y position.
        //
        // The CTM maps (0,0) to (tx, ty) and (1,1) to (tx+sx, ty+sy).
        // Due to Y-flip, ty > ty+sy, so the image is drawn from ty down to ty+sy.
        // We want to draw it from ty+sy up to ty.
        //
        // To do this, we flip the Y scale and adjust the Y position:
        // - new_sy = -final_transform.sy (flip the scale)
        // - new_ty = final_transform.ty + final_transform.sy * image.height (adjust position)

        let mut adjusted_transform = final_transform;
        adjusted_transform.sy = -final_transform.sy; // Flip Y scale
        adjusted_transform.ty = final_transform.ty + final_transform.sy * image.height as f32; // Adjust Y position

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
        if image.interpolate {
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

    fn has_font(&self, name: &str) -> bool {
        self.font_cache.contains_key(name)
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
        // Only Type1 fonts support text clipping (TrueType path would need
        // separate outline extraction). Silently skip for unsupported fonts.
        let is_type1 = self.font_cache.get(font_name).is_some_and(|f| f.is_type1());
        if !is_type1 {
            return;
        }

        // Build the glyph path — same logic as draw_text's Type1 branch but
        // without painting; just collect outlines into one PathBuilder.
        let (type1_font, pdf_widths) = {
            let font = self.font_cache.get(font_name).unwrap();
            (font.type1_font().unwrap().clone(), font.pdf_widths.clone())
        };

        let units_per_em = 1000.0f32;
        let spacing_to_font_units = if font_size.abs() > f64::EPSILON {
            units_per_em as f64 / font_size
        } else {
            0.0
        };

        let mut text_path = PathBuilder::new();
        let mut current_x = 0.0f32;

        for &byte in text_bytes {
            let ch = char::from(byte);
            let glyph_width = pdf_widths
                .as_ref()
                .map(|w| w[byte as usize])
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

        // Create a mask from the text path and intersect with existing clip.
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

            if let Some(mut mask) = Mask::new(self.pixmap.width(), self.pixmap.height()) {
                mask.fill_path(&path, SkiaFillRule::Winding, false, final_transform);

                // Intersect with existing clip mask
                if let Some(existing) = &self.current_state().clip_mask {
                    if let Some(merged) = intersect_masks(existing, &mask) {
                        self.current_state_mut().clip_mask = Some(merged);
                    }
                } else {
                    self.current_state_mut().clip_mask = Some(mask);
                }
            }
        }
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

        for y in 0..h {
            for x in 0..w {
                let px = x as usize;
                let py = y as usize;
                let pixel_idx = py * w as usize + px;

                // Check clip mask
                if let Some(cd) = &clip_data {
                    if cd[pixel_idx] == 0 {
                        continue;
                    }
                }

                // Device pixel → shading space via inverse CTM
                let dx = x as f32;
                let dy = y as f32;
                let sx = (d * (dx - e) - c * (dy - f)) * inv_det;
                let sy = (-b * (dx - e) + a * (dy - f)) * inv_det;

                if let Some((r, g, b_val, a_val)) = shading.sample(sx as f64, sy as f64) {
                    let offset = pixel_idx * 4;
                    let data = self.pixmap.data_mut();
                    if offset + 3 < data.len() {
                        // Alpha blend with existing pixel (source-over)
                        let alpha = a_val as f32 / 255.0;
                        let inv_alpha = 1.0 - alpha;
                        data[offset] = (r as f32 * alpha + data[offset] as f32 * inv_alpha) as u8;
                        data[offset + 1] =
                            (g as f32 * alpha + data[offset + 1] as f32 * inv_alpha) as u8;
                        data[offset + 2] =
                            (b_val as f32 * alpha + data[offset + 2] as f32 * inv_alpha) as u8;
                        data[offset + 3] = 255;
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

    fn end_offscreen_as_mask(&mut self, luminosity: bool) -> Option<(Vec<u8>, u32, u32)> {
        let saved = self.offscreen_stack.pop()?;
        let w = self.pixmap.width();
        let h = self.pixmap.height();
        let src = self.pixmap.data_mut();

        let mask_data: Vec<u8> = (0..(w * h) as usize)
            .map(|i| {
                let offset = i * 4;
                let a = src[offset + 3] as f32 / 255.0;
                if luminosity {
                    // Un-premultiply, compute luminosity, re-multiply by alpha
                    if a > 0.0 {
                        let r = (src[offset] as f32 / 255.0) / a;
                        let g = (src[offset + 1] as f32 / 255.0) / a;
                        let b = (src[offset + 2] as f32 / 255.0) / a;
                        // PDF spec: luminosity for DeviceRGB = 0.299R + 0.587G + 0.114B
                        ((0.299 * r + 0.587 * g + 0.114 * b) * a * 255.0) as u8
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
