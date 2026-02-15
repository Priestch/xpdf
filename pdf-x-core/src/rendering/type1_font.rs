//! Type1 and CFF font support using hayro-font
//!
//! This module provides Type1 and CFF font parsing and rendering support
//! by integrating hayro-font with tiny-skia.

use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::PathBuilder;

#[cfg(feature = "hayro-font")]
use hayro_font::{OutlineBuilder as HayroOutlineBuilder, type1::Table as Type1Table};

/// Font variant - either Type1 or CFF
#[cfg(feature = "hayro-font")]
#[derive(Clone)]
enum FontVariant {
    Type1(Type1Table),
    CFF(Arc<hayro_font::cff::Table<'static>>),
}

/// A Type1 or CFF font that can be used for rendering
#[derive(Clone)]
pub struct Type1Font {
    /// The font data (kept alive to satisfy 'static requirement)
    _data: Arc<Vec<u8>>,
    /// The font variant (Type1 or CFF)
    #[cfg(feature = "hayro-font")]
    variant: FontVariant,
    /// Glyph name to character code mapping (for reverse lookup, Type1 only)
    name_to_code: HashMap<String, u8>,
    /// Custom PDF encoding: byte code -> glyph name
    /// This maps PDF byte codes to the actual glyph names in the font
    custom_encoding: HashMap<u8, String>,
}

impl Type1Font {
    /// Create a new Type1 or CFF font from raw data.
    ///
    /// # Arguments
    /// * `data` - Raw font data (PFB, ASCII, or CFF format)
    ///
    /// # Returns
    /// Ok(Type1Font) if the data is valid Type1 or CFF, Err otherwise
    pub fn new(data: Vec<u8>) -> Result<Self, String> {
        #[cfg(feature = "hayro-font")]
        {
            // Check for CFF font first (binary format in FontFile3)
            let is_cff = data.len() >= 4 && data[0] == 1 && (data[2] == 4 || data[2] == 5);

            if is_cff {
                // CFF font - use the CFF parser
                // SAFETY: We extend the lifetime to 'static since we keep the data alive via Arc
                let static_data: &'static [u8] =
                    unsafe { std::slice::from_raw_parts(data.as_ptr(), data.len()) };

                let cff_table =
                    hayro_font::cff::Table::parse(static_data).ok_or("Failed to parse CFF font")?;

                return Ok(Type1Font {
                    _data: Arc::new(data),
                    variant: FontVariant::CFF(Arc::new(cff_table)),
                    name_to_code: HashMap::new(),
                    custom_encoding: HashMap::new(),
                });
            }

            // Type1 font - use Type1 parser
            if !Self::is_type1(&data) {
                return Err("Not a Type1 or CFF font".to_string());
            }

            let table = Type1Table::parse(&data).ok_or("Failed to parse Type1 font")?;

            // Build name to code mapping for reverse lookup
            let mut name_to_code: HashMap<String, u8> = HashMap::new();
            for code in 0..=255u8 {
                if let Some(name) = table.code_to_string(code) {
                    name_to_code.insert(name.to_string(), code);
                }
            }

            Ok(Type1Font {
                _data: Arc::new(data),
                variant: FontVariant::Type1(table),
                name_to_code,
                custom_encoding: HashMap::new(),
            })
        }

        #[cfg(not(feature = "hayro-font"))]
        {
            Err("hayro-font feature is not enabled".to_string())
        }
    }

    /// Check if data appears to be a Type1 or CFF font
    pub fn is_type1(data: &[u8]) -> bool {
        // PFB format starts with 0x80 0x01
        if data.len() >= 2 && data[0] == 0x80 && data[1] == 0x01 {
            return true;
        }

        // ASCII Type1 starts with "%!"
        if data.len() >= 2 && data[0] == b'%' && data[1] == b'!' {
            return true;
        }

        // CFF font (Compact Font Format) - used in FontFile3
        if data.len() >= 4 && data[0] == 1 && (data[2] == 4 || data[2] == 5) {
            return true;
        }

        // Embedded Type1 font programs (stripped-down, without header)
        let data_str = std::str::from_utf8(data);

        if let Ok(text) = data_str {
            let type1_keywords = ["/FontName", "/FontMatrix", "/CharStrings", "/Encoding"];
            let mut found_count = 0;
            for keyword in &type1_keywords {
                if text.contains(keyword) {
                    found_count += 1;
                }
            }

            if found_count >= 2 || text.contains("eexec") {
                return true;
            }
        }

        false
    }

    /// Check if this is a CFF font
    pub fn is_cff(&self) -> bool {
        #[cfg(feature = "hayro-font")]
        {
            matches!(self.variant, FontVariant::CFF(_))
        }
        #[cfg(not(feature = "hayro-font"))]
        {
            false
        }
    }

    /// Get the font matrix for transformation
    #[cfg(feature = "hayro-font")]
    pub fn font_matrix(&self) -> hayro_font::Matrix {
        match &self.variant {
            FontVariant::Type1(table) => table.matrix(),
            FontVariant::CFF(table) => table.matrix(),
        }
    }

    /// Outline a glyph by name to a tiny-skia path (Type1 fonts only)
    ///
    /// # Arguments
    /// * `glyph_name` - The name of the glyph (e.g., "A", "B", "exclam")
    /// * `path_builder` - The tiny-skia PathBuilder to receive the outline
    ///
    /// # Returns
    /// Some(()) if successful, None if the glyph doesn't exist
    pub fn outline_glyph(&self, glyph_name: &str, path_builder: &mut PathBuilder) -> Option<()> {
        #[cfg(feature = "hayro-font")]
        {
            let mut converter = PathConverter(path_builder);

            match &self.variant {
                FontVariant::Type1(table) => table.outline(glyph_name, &mut converter),
                FontVariant::CFF(table) => {
                    // For CFF, we need to convert glyph name to glyph ID first
                    let gid = table.glyph_index_by_name(glyph_name)?;
                    table.outline(gid, &mut converter).ok().map(|_| ())
                }
            }
        }

        #[cfg(not(feature = "hayro-font"))]
        {
            None
        }
    }

    /// Outline a glyph by character code to a tiny-skia path (works for both Type1 and CFF)
    ///
    /// # Arguments
    /// * `ch` - The character
    /// * `path_builder` - The tiny-skia PathBuilder to receive the outline
    ///
    /// # Returns
    /// Some(()) if successful, None if the glyph doesn't exist
    pub fn outline_glyph_char(&self, ch: char, path_builder: &mut PathBuilder) -> Option<()> {
        #[cfg(feature = "hayro-font")]
        {
            let mut converter = PathConverter(path_builder);
            let code = ch as u8;

            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: outline_glyph_char: char='{}' (code={}), is_cff={}, has_custom_encoding={}",
                ch,
                code,
                self.is_cff(),
                !self.custom_encoding.is_empty()
            );

            match &self.variant {
                FontVariant::Type1(table) => {
                    // Type1 uses glyph names
                    let name = table.code_to_string(code)?;
                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: Type1 code_to_string({}) -> Some({})", code, name);
                    table.outline(name, &mut converter)
                }
                FontVariant::CFF(table) => {
                    // For CFF, check custom encoding first
                    if let Some(glyph_name) = self.get_glyph_name_for_code(code) {
                        #[cfg(feature = "debug-logging")]
                        eprintln!(
                            "DEBUG: CFF using custom encoding: code={} -> name='{}'",
                            code, glyph_name
                        );

                        // Look up glyph by name using custom encoding
                        let gid = table.glyph_index_by_name(&glyph_name)?;
                        #[cfg(feature = "debug-logging")]
                        eprintln!(
                            "DEBUG: CFF glyph_index_by_name({}) -> Some({:?})",
                            glyph_name, gid
                        );
                        return table.outline(gid, &mut converter).ok().map(|_| ());
                    }

                    // Fall back to direct code lookup (standard encoding)
                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: CFF calling glyph_index({})", code);
                    let gid = table.glyph_index(code)?;
                    #[cfg(feature = "debug-logging")]
                    eprintln!("DEBUG: CFF glyph_index({}) -> Some({:?})", code, gid);
                    table.outline(gid, &mut converter).ok().map(|_| ())
                }
            }
        }

        #[cfg(not(feature = "hayro-font"))]
        {
            None
        }
    }

    /// Convert a character code to a glyph name (Type1 only)
    pub fn char_to_glyph_name(&self, ch: char) -> Option<String> {
        let code = ch as u32;
        if code > 255 {
            return None;
        }

        #[cfg(feature = "hayro-font")]
        {
            match &self.variant {
                FontVariant::Type1(table) => table
                    .code_to_string(code as u8)
                    .map(|s: &str| s.to_string()),
                FontVariant::CFF(_) => None, // CFF doesn't use names for code->glyph mapping
            }
        }

        #[cfg(not(feature = "hayro-font"))]
        {
            None
        }
    }

    /// Convert a glyph name back to a character code (Type1 only)
    pub fn glyph_name_to_char(&self, name: &str) -> Option<u8> {
        self.name_to_code.get(name).copied()
    }

    /// Get the width of a glyph by character code.
    ///
    /// Returns the glyph width in font units, or a default width if unavailable.
    pub fn glyph_width(&self, ch: char) -> u16 {
        #[cfg(feature = "hayro-font")]
        {
            let code = ch as u8;

            match &self.variant {
                FontVariant::Type1(_table) => {
                    // Type1 widths would need to be parsed from the font
                    // For now, use a reasonable default
                    500
                }
                FontVariant::CFF(table) => {
                    // For CFF, try to get the glyph width
                    if let Some(glyph_name) = self.get_glyph_name_for_code(code) {
                        if let Some(gid) = table.glyph_index_by_name(&glyph_name) {
                            if let Some(width) = table.glyph_width(gid) {
                                return width;
                            }
                        }
                    }
                    // Fallback to default width
                    500
                }
            }
        }

        #[cfg(not(feature = "hayro-font"))]
        {
            500
        }
    }

    /// Set a custom PDF encoding (byte code -> glyph name mapping).
    ///
    /// This is used when the PDF font dictionary has a custom Encoding
    /// that maps byte codes to different glyph names than the font's
    /// built-in encoding.
    ///
    /// # Arguments
    /// * `encoding` - HashMap mapping byte codes to glyph names
    pub fn set_custom_encoding(&mut self, encoding: HashMap<u8, String>) {
        self.custom_encoding = encoding;
    }

    /// Get the custom encoding for a byte code, if available.
    fn get_glyph_name_for_code(&self, code: u8) -> Option<String> {
        // First check custom encoding from PDF
        if let Some(name) = self.custom_encoding.get(&code) {
            return Some(name.clone());
        }

        // Fall back to built-in encoding (Type1 only)
        #[cfg(feature = "hayro-font")]
        {
            if let FontVariant::Type1(table) = &self.variant {
                if let Some(name) = table.code_to_string(code) {
                    return Some(name.to_string());
                }
            }
        }

        None
    }
}

/// Adapter to convert hayro-font OutlineBuilder calls to tiny-skia PathBuilder
struct PathConverter<'a>(&'a mut PathBuilder);

impl<'a> HayroOutlineBuilder for PathConverter<'a> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "hayro-font")]
    fn test_type1_detection() {
        // PFB format
        assert!(Type1Font::is_type1(&[0x80, 0x01, 0x00, 0x00]));

        // ASCII format
        assert!(Type1Font::is_type1(b"%!PS-AdobeFont"));

        // Not Type1
        assert!(!Type1Font::is_type1(b"OTTO"));
        assert!(!Type1Font::is_type1(b"true"));
    }

    #[test]
    fn test_type1_detection_no_feature() {
        // Should still detect format even without hayro-font feature
        assert!(Type1Font::is_type1(&[0x80, 0x01]));
        assert!(Type1Font::is_type1(b"%!PS-AdobeFont"));
    }
}
