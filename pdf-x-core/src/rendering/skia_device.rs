//! A tiny-skia based rendering device.

use crate::core::error::{PDFError, PDFResult};
use crate::rendering::device::{Device, ImageData, Paint, PathDrawMode};
use crate::rendering::type1_font::Type1Font;
use crate::rendering::{Color, FillRule, LineCap, LineJoin, StrokeProps};
use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::{
    FillRule as SkiaFillRule, LineCap as SkiaLineCap, LineJoin as SkiaLineJoin, Mask,
    Paint as SkiaPaint, Path, PathBuilder, Pixmap, PixmapMut, Rect, Stroke, Transform,
};
use ttf_parser::OutlineBuilder;

// --- Conversion helpers ---

fn to_skia_color(color: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(color.r(), color.g(), color.b(), color.a())
}

fn to_skia_paint(paint: &Paint) -> SkiaPaint {
    let mut sk_paint = SkiaPaint::default();
    match paint {
        Paint::Solid(color) => {
            sk_paint.set_color(to_skia_color(*color));
        }
    }
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
    Stroke {
        width: stroke_props.line_width as f32,
        miter_limit: stroke_props.miter_limit as f32,
        line_cap: to_skia_line_cap(stroke_props.line_cap),
        line_join: to_skia_line_join(stroke_props.line_join),
        dash: None, // TODO: Add dash pattern support
    }
}

#[derive(Clone)]
struct SkiaGraphicsState {
    transform: Transform,
    clip_mask: Option<Mask>,
}

impl Default for SkiaGraphicsState {
    fn default() -> Self {
        SkiaGraphicsState {
            transform: Transform::identity(),
            clip_mask: None,
        }
    }
}

/// Font data storage with Arc for shared ownership
struct StoredFont {
    /// The font data (kept alive to satisfy 'static requirement)
    _data: Arc<Vec<u8>>,
    /// Font type (TrueType or Type1)
    font_type: FontType,
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
                Some(rustybuzz::shape(buzz_face, &[], buffer))
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

    /// Set custom encoding for Type1/CFF fonts.
    pub fn set_custom_encoding(&mut self, encoding: std::collections::HashMap<u8, String>) {
        if let FontType::Type1 { font } = &mut self.font_type {
            font.set_custom_encoding(encoding);
        }
    }
}

/// Parse a PDF Encoding dictionary into a byte-to-glyph-name mapping.
///
/// This handles both named encodings (like "WinAnsiEncoding") and custom
/// encodings with a Differences array.
fn parse_encoding_dictionary(
    enc_obj: &crate::core::parser::PDFObject,
) -> Option<std::collections::HashMap<u8, String>> {
    use crate::core::parser::PDFObject;
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
    font_cache: HashMap<String, StoredFont>,
    draw_count: usize,
    colors_seen: std::collections::HashMap<String, usize>,
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
    pub fn new(pixmap: PixmapMut<'a>) -> Self {
        SkiaDevice {
            pixmap,
            state_stack: vec![SkiaGraphicsState::default()],
            path_builder: PathBuilder::new(),
            font_cache: HashMap::new(),
            draw_count: 0,
            colors_seen: std::collections::HashMap::new(),
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
        encoding: Option<&crate::core::parser::PDFObject>,
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
        // The clip mask is created when the clip is set, so we just return it
        // This ensures the mask is in the correct coordinate space (the CTM at the time the clip was set)
        self.current_state().clip_mask.clone()
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
                Paint::Solid(crate::rendering::Color::RGB(1.0, 1.0, 1.0))
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

        let sk_paint = to_skia_paint(paint);
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

    fn draw_text(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        paint: &Paint,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
    ) -> PDFResult<f64> {
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: draw_text called with font='{}', size={}, bytes={:?}, text_matrix={:?}",
            font_name, font_size, text_bytes, text_matrix
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
            // Convert bytes to string using Latin-1 (ISO-8859-1) encoding
            // This preserves byte values 0-255 as Unicode code points 0-255
            // Not perfect but better than UTF-8 conversion for PDF encodings
            let text = text_bytes.iter().map(|&b| b as char).collect::<String>();

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
            let font_scale = font_size as f32 / units_per_em;

            let mut text_path_builder = PathBuilder::new();
            let scale = font_scale;

            // Keep glyph positions local to the text run; text_matrix applies run placement.
            let mut current_x = 0.0f32;
            let mut current_y = 0.0f32;

            for (info, pos) in glyph_infos.iter().zip(glyph_positions.iter()) {
                let mut converter = PathConverter(PathBuilder::new());
                let transform =
                    Transform::from_scale(scale, scale).post_translate(current_x, current_y);

                let _ = font
                    .face()
                    .expect("TrueType font should have face")
                    .outline_glyph(ttf_parser::GlyphId(info.glyph_id as u16), &mut converter);

                if let Some(p) = converter.0.finish() {
                    if let Some(path_transformed) = p.transform(transform) {
                        text_path_builder.push_path(&path_transformed);
                    }
                }

                // Advance in font space (advance * units_per_em / 1000, then scaled by font_size/units_per_em)
                // Simplifies to: advance * font_size / 1000
                current_x += pos.x_advance as f32 * font_scale;
                current_y += pos.y_advance as f32 * font_scale;
            }

            // current_x/current_y are tracked in text space units after applying font_scale.
            let total_rendered_width = current_x as f64;

            if let Some(path) = text_path_builder.finish() {
                let sk_paint = to_skia_paint(paint);
                let ctm = self.current_state().transform;

                // Reference: hayro/hayro-interpret/src/interpret/state.rs:104-179
                // The full text transform is: text_matrix * temp_transform
                // where temp_transform includes font_size, horizontal_scaling, and text_rise
                //
                // temp_transform = |font_size * h_scale  0        0|
                //                   |0                      font_size  rise|
                //                   |0                      0        1|
                //
                // Then: full_transform = text_matrix * temp_transform * CTM

                let tm_a = text_matrix[0] as f32;
                let tm_b = text_matrix[1] as f32;
                let tm_c = text_matrix[2] as f32;
                let tm_d = text_matrix[3] as f32;
                let tm_e = text_matrix[4] as f32;
                let tm_f = text_matrix[5] as f32;

                let font_size_f = font_size as f32;
                let h_scale_pct = horizontal_scaling as f32 / 100.0; // Tz is percentage
                let rise_f = text_rise as f32;

                // Compose: text_matrix * temp_transform
                // Result is the full text transform before CTM
                let full_text = Transform::from_row(
                    tm_a * font_size_f * h_scale_pct,
                    tm_b * font_size_f,
                    tm_c * font_size_f * h_scale_pct,
                    tm_d * font_size_f,
                    tm_e * font_size_f * h_scale_pct + tm_c * rise_f + tm_e,
                    tm_f * font_size_f * h_scale_pct + tm_d * rise_f + tm_f,
                );

                // Then apply CTM
                let final_transform = full_text.post_concat(ctm);

                let clip_mask = self.get_clip_mask();

                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Text CTM={:?}, text_matrix=[{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}], hscale={:.1}%, rise={:.1}, full_text={:?}, final={:?}",
                    ctm, tm_a, tm_b, tm_c, tm_d, tm_e, tm_f, horizontal_scaling, text_rise, full_text, final_transform
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

        // Clone the font data to avoid borrow issues
        let font = self.font_cache.get(font_name).unwrap();
        let type1_font = font.type1_font().unwrap().clone();
        // font reference is now dropped

        // Now render the text with Type1 font
        // Type1 fonts use 1000 units per em
        let units_per_em = 1000.0;
        let font_scale = font_size as f32 / units_per_em;

        let mut text_path_builder = PathBuilder::new();
        let scale = font_scale;

        // Keep glyph positions local to the text run; text_matrix applies run placement.
        let mut current_x = 0.0f32;
        let current_y = 0.0f32;

        #[cfg(feature = "debug-logging")]
        let mut glyphs_rendered = 0;
        #[cfg(feature = "debug-logging")]
        let mut glyphs_total = 0;

        // For each byte, outline it directly (works for both Type1 and CFF)
        // PDF text strings use font encoding, where each byte maps to a glyph
        for &byte in text_bytes {
            #[cfg(feature = "debug-logging")]
            {
                glyphs_total += 1;
            }
            let mut glyph_path_builder = PathBuilder::new();

            // Convert byte to char (for code-to-glyph mapping)
            // The font's encoding maps byte codes to glyphs
            let ch = char::from(byte);

            // Use outline_glyph_char which works for both Type1 and CFF
            if type1_font
                .outline_glyph_char(ch, &mut glyph_path_builder)
                .is_some()
            {
                #[cfg(feature = "debug-logging")]
                {
                    glyphs_rendered += 1;
                }
                if let Some(glyph_path) = glyph_path_builder.finish() {
                    // Transform the glyph to the current position
                    let transform =
                        Transform::from_scale(scale, scale).post_translate(current_x, current_y);

                    if let Some(path_transformed) = glyph_path.transform(transform) {
                        text_path_builder.push_path(&path_transformed);
                    }
                }

                // Advance using actual glyph width from font metrics
                let glyph_width = type1_font.glyph_width(ch) as f32;
                current_x += scale * glyph_width;
            }
        }

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Text rendering: {}/{} glyphs rendered",
            glyphs_rendered, glyphs_total
        );

        // current_x is tracked in text space units after applying font_scale.
        let total_rendered_width = current_x as f64;

        if let Some(path) = text_path_builder.finish() {
            #[cfg(feature = "debug-logging")]
            eprintln!("DEBUG: Text path created successfully");

            let sk_paint = to_skia_paint(paint);
            let ctm = self.current_state().transform;

            // Reference: hayro/hayro-interpret/src/interpret/state.rs:104-179
            // The full text transform is: text_matrix * temp_transform
            // where temp_transform includes font_size, horizontal_scaling, and text_rise
            //
            // temp_transform = |font_size * h_scale  0        0|
            //                   |0                      font_size  rise|
            //                   |0                      0        1|
            //
            // Then: full_transform = text_matrix * temp_transform * CTM

            let tm_a = text_matrix[0] as f32;
            let tm_b = text_matrix[1] as f32;
            let tm_c = text_matrix[2] as f32;
            let tm_d = text_matrix[3] as f32;
            let tm_e = text_matrix[4] as f32;
            let tm_f = text_matrix[5] as f32;

            let font_size_f = font_size as f32;
            let h_scale_pct = horizontal_scaling as f32 / 100.0; // Tz is percentage
            let rise_f = text_rise as f32;

            // Compose: text_matrix * temp_transform
            // Result is the full text transform before CTM
            let full_text = Transform::from_row(
                tm_a * font_size_f * h_scale_pct,
                tm_b * font_size_f,
                tm_c * font_size_f * h_scale_pct,
                tm_d * font_size_f,
                tm_e * font_size_f * h_scale_pct + tm_c * rise_f + tm_e,
                tm_f * font_size_f * h_scale_pct + tm_d * rise_f + tm_f,
            );

            // Then apply CTM
            let final_transform = full_text.post_concat(ctm);

            let clip_mask = self.get_clip_mask();

            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: CTM={:?}, text_matrix=[{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}], hscale={:.1}%, rise={:.1}, full_text={:?}, final={:?}",
                ctm, tm_a, tm_b, tm_c, tm_d, tm_e, tm_f, horizontal_scaling, text_rise, full_text, final_transform
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

        let (data, has_alpha) = match (image.bits_per_component, image.data.len()) {
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
                let mut rgba = Vec::with_capacity(image.width as usize * image.height as usize * 4);
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

            // CMYK to RGBA conversion (8 bpc, 4 channels per pixel)
            // Must check this BEFORE RGB (3 channels) since CMYK also has data_len >= pixel_count * 3
            // Must check AFTER RGBA to avoid treating RGBA as CMYK
            // Reference: pdf.js/src/core/colorspace.js - DeviceCmykCS.#toRgb
            // Uses polynomial coefficients derived from US Web Coated (SWOP) colorspace
            (8, data_len) if !image.has_alpha && data_len >= pixel_count * 4 => {
                let mut rgba = Vec::with_capacity(pixel_count * 4);
                for chunk in image.data.chunks(4).take(pixel_count) {
                    if chunk.len() == 4 {
                        // CMYK values are 0-255, need to normalize to 0-1 for the formula
                        // PDF.js uses srcScale parameter to normalize, we do it directly
                        let c = (chunk[0] as f32) / 255.0;
                        let m = (chunk[1] as f32) / 255.0;
                        let y = (chunk[2] as f32) / 255.0;
                        let k = (chunk[3] as f32) / 255.0;

                        // CMYK to RGB conversion using PDF.js polynomial coefficients
                        // Derived from numerical analysis of US Web Coated (SWOP) colorspace
                        let r = 255.0
                            + c * (-4.387332384609988 * c
                                + 54.48615194189176 * m
                                + 18.82290502165302 * y
                                + 212.25662451639585 * k
                                - 285.2331026137004)
                            + m * (1.7149763477362134 * m
                                - 5.6096736904047315 * y
                                - 17.873870861415444 * k
                                - 5.497006427196366)
                            + y * (-2.5217340131683033 * y - 21.248923337353073 * k
                                + 17.5119270841813)
                            + k * (-21.86122147463605 * k - 189.48180835922747);

                        let g = 255.0
                            + c * (8.841041422036149 * c
                                + 60.118027045597366 * m
                                + 6.871425592049007 * y
                                + 31.159100130055922 * k
                                - 79.2970844816548)
                            + m * (-15.310361306967817 * m
                                + 17.575251261109482 * y
                                + 131.35250912493976 * k
                                - 190.9453302588951)
                            + y * (4.444339102852739 * y + 9.8632861493405 * k - 24.86741582555878)
                            + k * (-20.737325471181034 * k - 187.80453709719578);

                        let b = 255.0
                            + c * (0.8842522430003296 * c
                                + 8.078677503112928 * m
                                + 30.89978309703729 * y
                                - 0.23883238689178934 * k
                                - 14.183576799673286)
                            + m * (10.49593273432072 * m
                                + 63.02378494754052 * y
                                + 50.606957656360734 * k
                                - 112.23884253719248)
                            + y * (0.03296041114873217 * y + 115.60384449646641 * k
                                - 193.58209356861505)
                            + k * (-22.33816807309886 * k - 180.12613974708367);

                        rgba.push(r.clamp(0.0, 255.0) as u8);
                        rgba.push(g.clamp(0.0, 255.0) as u8);
                        rgba.push(b.clamp(0.0, 255.0) as u8);
                        rgba.push(255); // A (opaque)
                    }
                }
                (rgba, true)
            }

            // RGB to RGBA conversion (8 bpc, 3 channels per pixel)
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
        };

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

        self.pixmap.draw_pixmap(
            0,
            0,
            image_pixmap.as_ref(),
            &tiny_skia::PixmapPaint::default(),
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
        encoding: Option<&crate::core::parser::PDFObject>,
    ) -> PDFResult<()> {
        self.load_font(name, data, encoding)
    }
}
