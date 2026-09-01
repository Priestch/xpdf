//! Font handling for text extraction and rendering.
//!
//! This module provides font processing capabilities using hayro-font for
//! CFF and Type1 font parsing, along with CMap support for character encoding.
//!
//! The main components are:
//! - **FontDict**: PDF font dictionary representation
//! - **Font**: Complete font with encoding, metrics, and glyph access
//! - Integration with hayro-font for CFF/Type1 glyph metrics
//! - Integration with CMap for character-to-Unicode mapping
//!
//! Based on PDF.js's font handling in src/core/fonts.js

use crate::core::cmap::CMap;
use crate::core::decode;
use crate::core::encoding::Encoding;
use crate::core::error::{PDFError, PDFResult};
use crate::core::parser::PDFObject;
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// PDF font type enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontType {
    /// Type1 font
    Type1,
    /// Type1 Compact Font Format (CFF)
    Type1C,
    /// TrueType font
    TrueType,
    /// Type3 font (user-defined glyphs)
    Type3,
    /// CID font (multi-byte character ID)
    CIDFontType0,
    /// CID TrueType font
    CIDFontType2,
    /// Type0 composite font (wraps a CID font via /DescendantFonts)
    Type0,
    /// Unknown font type
    Unknown,
}

impl FontType {
    /// Parse font type from PDF font dictionary Subtype.
    pub fn from_subtype(subtype: &str) -> Self {
        match subtype {
            "Type1" => FontType::Type1,
            "Type1C" => FontType::Type1C,
            "TrueType" => FontType::TrueType,
            "Type3" => FontType::Type3,
            "Type0" => FontType::Type0,
            "CIDFontType0" => FontType::CIDFontType0,
            "CIDFontType2" => FontType::CIDFontType2,
            _ => FontType::Unknown,
        }
    }

    /// Returns true if this is a CID font (multi-byte character IDs).
    /// Type0 is a composite font that always uses a CIDFont descendant.
    pub fn is_cid_font(&self) -> bool {
        matches!(
            self,
            FontType::Type0 | FontType::CIDFontType0 | FontType::CIDFontType2
        )
    }
}

/// Represents a PDF font dictionary.
///
/// Contains all the information from a /Font resource dictionary,
/// including encoding, metrics, and embedded font data.
#[derive(Debug, Clone)]
pub struct FontDict {
    /// Font type (Type1, TrueType, CIDFont, etc.)
    pub font_type: FontType,

    /// Font base name (e.g., "Helvetica", "Times-Roman")
    pub base_font: String,

    /// Encoding (built-in like "WinAnsiEncoding" or custom)
    pub encoding: Option<PDFObject>,

    /// /ToUnicode CMap stream for character mapping
    pub to_unicode: Option<PDFObject>,

    /// Font descriptor (contains embedded font data)
    pub font_descriptor: Option<PDFObject>,

    /// /Widths array for character widths
    pub widths: Option<Vec<f64>>,

    /// /FirstChar (first character code in widths array)
    pub first_char: Option<u32>,

    /// /LastChar (last character code in widths array)
    pub last_char: Option<u32>,

    /// Default width for missing characters
    pub default_width: f64,

    /// CID font information (for CIDFonts)
    pub descendant_fonts: Option<PDFObject>,
}

impl FontDict {
    /// Parse a font dictionary from a PDFObject.
    pub fn from_pdf_object(font_obj: &PDFObject) -> PDFResult<Self> {
        let dict = match font_obj {
            PDFObject::Dictionary(d) => d,
            _ => return Err(PDFError::Generic("Font is not a dictionary".to_string())),
        };

        // Get font type
        let subtype = dict
            .get("Subtype")
            .and_then(|obj| match obj {
                PDFObject::Name(name) => Some(name.as_str()),
                _ => None,
            })
            .unwrap_or("Unknown");

        let font_type = FontType::from_subtype(subtype);

        // Get base font name
        let base_font = dict
            .get("BaseFont")
            .and_then(|obj| match obj {
                PDFObject::Name(name) => Some(name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "Unknown".to_string());

        // Get encoding
        let encoding = dict.get("Encoding").cloned();

        // Get ToUnicode CMap
        let to_unicode = dict.get("ToUnicode").cloned();

        // Get font descriptor
        let font_descriptor = dict.get("FontDescriptor").cloned();

        // Get widths array
        let widths = dict.get("Widths").and_then(|obj| match obj {
            PDFObject::Array(arr) => {
                let mut widths = Vec::new();
                for item in arr {
                    if let PDFObject::Number(w) = &**item {
                        widths.push(*w);
                    }
                }
                Some(widths)
            }
            _ => None,
        });

        // Get FirstChar and LastChar
        let first_char = dict.get("FirstChar").and_then(|obj| match obj {
            PDFObject::Number(n) => Some(*n as u32),
            _ => None,
        });

        let last_char = dict.get("LastChar").and_then(|obj| match obj {
            PDFObject::Number(n) => Some(*n as u32),
            _ => None,
        });

        // Get DescendantFonts (for Type0 composite fonts)
        let descendant_fonts = dict.get("DescendantFonts").cloned();

        Ok(FontDict {
            font_type,
            base_font,
            encoding,
            to_unicode,
            font_descriptor,
            widths,
            first_char,
            last_char,
            default_width: 250.0, // PDF default width
            descendant_fonts,
        })
    }
}

/// Complete font with encoding, metrics, and glyph access.
///
/// This is the main font object used during text extraction.
/// It combines CMap encoding with glyph metrics from the PDF font.
pub struct Font {
    /// Font dictionary information
    pub dict: FontDict,

    /// ToUnicode CMap for character encoding
    pub cmap: Option<CMap>,

    /// Font encoding (Standard, WinAnsi, MacRoman, or custom)
    pub encoding: Encoding,

    /// Character width cache (CID -> width in glyph space units)
    pub width_cache: FxHashMap<u16, f64>,

    /// Embedded font data (CFF or Type1), if available
    pub embedded_font: Option<Vec<u8>>,
}

impl Font {
    /// Creates a new Font from a font dictionary.
    ///
    /// # Arguments
    /// * `font_dict` - The PDF font dictionary object
    /// * `xref` - Cross-reference table for fetching referenced objects
    pub fn new<R: crate::core::page::PageResolver>(
        font_dict: PDFObject,
        resolver: &mut R,
    ) -> PDFResult<Self> {
        let dict = FontDict::from_pdf_object(&font_dict)?;

        let encoding = if let Some(enc_obj) = &dict.encoding {
            let fetched = resolver
                .resolve_if_ref(enc_obj)
                .unwrap_or_else(|_| Arc::new(enc_obj.clone()));
            Encoding::from_pdf_object(&fetched).unwrap_or(Encoding::Standard)
        } else {
            Encoding::Standard
        };

        let cmap = if let Some(to_unicode_ref) = &dict.to_unicode {
            match &*resolver.resolve_if_ref(to_unicode_ref)? {
                PDFObject::Stream {
                    dict: stream_dict,
                    data,
                } => {
                    let decompressed = if let Some(filter) = stream_dict.get("Filter") {
                        decode::apply_filters(&data, filter).map_err(|e| {
                            PDFError::Generic(format!("ToUnicode stream decode error: {}", e))
                        })?
                    } else {
                        data.to_vec()
                    };
                    Some(CMap::parse(&decompressed)?)
                }
                _ => None,
            }
        } else {
            None
        };

        let mut width_cache = FxHashMap::default();
        if let (Some(widths), Some(first_char)) = (&dict.widths, dict.first_char) {
            for (i, &width) in widths.iter().enumerate() {
                let cid = first_char + i as u32;
                width_cache.insert(cid as u16, width);
            }
        }

        let embedded_font = if let Some(descriptor_ref) = &dict.font_descriptor {
            Self::extract_embedded_font(descriptor_ref, resolver)?
        } else {
            None
        };

        Ok(Font {
            dict,
            cmap,
            encoding,
            width_cache,
            embedded_font,
        })
    }

    fn extract_embedded_font<R: crate::core::page::PageResolver>(
        descriptor_ref: &PDFObject,
        resolver: &mut R,
    ) -> PDFResult<Option<Vec<u8>>> {
        let descriptor = resolver.resolve_if_ref(descriptor_ref)?;

        let descriptor_dict = match &*descriptor {
            PDFObject::Dictionary(d) => d.clone(),
            _ => return Ok(None),
        };

        if let Some(font_file3_ref) = descriptor_dict.get("FontFile3") {
            return Self::extract_font_stream(font_file3_ref, resolver);
        }

        if let Some(font_file_ref) = descriptor_dict.get("FontFile") {
            return Self::extract_font_stream(font_file_ref, resolver);
        }

        if descriptor_dict.contains_key("FontFile2") {
            return Ok(None);
        }

        Ok(None)
    }

    fn extract_font_stream<R: crate::core::page::PageResolver>(
        stream_ref: &PDFObject,
        resolver: &mut R,
    ) -> PDFResult<Option<Vec<u8>>> {
        let stream_obj = resolver.resolve_if_ref(stream_ref)?;

        match &*stream_obj {
            PDFObject::Stream { dict, data } => {
                // Use apply_filters (not decode_stream): FontFile streams commonly
                // use /ASCIIHexDecode, /ASCII85Decode, filter arrays, etc.
                // decode_stream only handles a bare FlateDecode name and rejects
                // everything else as "Unsupported filter" — which broke extract
                // for many Type1 embeddings (ASCIIHex FontFile).
                // Source: same path as ToUnicode / page content decoding.
                let decompressed = if let Some(filter) = dict.get("Filter") {
                    decode::apply_filters(data, filter).map_err(|e| {
                        PDFError::Generic(format!("Font stream decode error: {}", e))
                    })?
                } else {
                    data.clone()
                };

                Ok(Some(decompressed))
            }
            _ => Ok(None),
        }
    }

    /// Maps a character code (CID) to Unicode using the ToUnicode CMap.
    ///
    /// Falls back to the character code itself if no mapping exists.
    #[inline(always)] // Hot path during text extraction
    pub fn to_unicode(&self, cid: u16) -> char {
        self.char_code_to_unicode(cid)
    }

    /// Maps a character code to Unicode using the proper fallback chain.
    ///
    /// The fallback chain is:
    /// 1. ToUnicode CMap (if available)
    /// 2. Font encoding (Standard/WinAnsi/MacRoman/custom)
    /// 3. Direct mapping (code as-is if valid Unicode, else replacement char)
    ///
    /// This method implements the PDF spec's text decoding algorithm.
    #[inline(always)] // Hot path during text extraction
    pub fn char_code_to_unicode(&self, code: u16) -> char {
        // First, try the ToUnicode CMap (most reliable)
        if let Some(ref cmap) = self.cmap {
            if let Some(unicode_char) = cmap.to_unicode(code) {
                return unicode_char;
            }
        }

        // Next, try the font encoding (for simple fonts with 1-byte codes)
        if code <= 255 {
            return self.encoding.char_to_unicode(code as u8);
        }

        // Fallback: use the code as-is if it's a valid Unicode code point
        char::from_u32(code as u32).unwrap_or('\u{FFFD}') // U+FFFD = replacement character
    }

    /// Gets the width of a character in glyph space units (typically 1/1000 em).
    ///
    /// # Arguments
    /// * `cid` - The character ID
    ///
    /// # Returns
    /// Character width in glyph space units, or default width if not found
    #[inline(always)] // Hot path during text extraction
    pub fn get_char_width(&self, cid: u16) -> f64 {
        self.width_cache
            .get(&cid)
            .copied()
            .unwrap_or(self.dict.default_width)
    }

    /// Gets the width of a character in user space units.
    ///
    /// This applies the font size scaling to convert from glyph space to user space.
    ///
    /// # Arguments
    /// * `cid` - The character ID
    /// * `font_size` - The current font size in user space units
    ///
    /// # Returns
    /// Character width in user space units
    #[inline]
    pub fn get_char_width_user_space(&self, cid: u16, font_size: f64) -> f64 {
        self.get_char_width(cid) * font_size / 1000.0
    }

    /// Returns the font type.
    pub fn font_type(&self) -> &FontType {
        &self.dict.font_type
    }

    /// Returns the base font name.
    pub fn base_font(&self) -> &str {
        &self.dict.base_font
    }

    /// Returns true if this font has a ToUnicode CMap.
    pub fn has_to_unicode(&self) -> bool {
        self.cmap.is_some()
    }

    /// Returns true if this font has embedded font data.
    pub fn has_embedded_font(&self) -> bool {
        self.embedded_font.is_some()
    }

    // ── Inverse encoding (for text replacement) ─────────────────────────

    /// Build a char → glyph-code inverse map.
    ///
    /// For simple fonts: iterates codes 0..=255, skips entries whose
    /// `char_code_to_unicode` yields `\u{FFFD}` (unmapped).  When multiple
    /// codes map to the same char the lowest code wins.
    ///
    /// Returns `None` for CID fonts (v1 — core Font lacks /W widths).
    pub fn build_inverse_map(&self) -> Option<FxHashMap<char, u16>> {
        if self.dict.font_type.is_cid_font() {
            return None;
        }
        let mut map = FxHashMap::default();
        for code in 0u16..=255 {
            let ch = self.char_code_to_unicode(code);
            if ch == '\u{FFFD}' {
                continue;
            }
            // Keep the lowest code for each char.
            map.entry(ch).or_insert(code);
        }
        Some(map)
    }

    /// Check whether every character in `text` has an entry in the inverse map.
    pub fn can_encode(&self, inverse: &FxHashMap<char, u16>, text: &str) -> bool {
        text.chars().all(|ch| inverse.contains_key(&ch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::smallvec;

    #[test]
    fn test_font_type_from_subtype() {
        assert_eq!(FontType::from_subtype("Type1"), FontType::Type1);
        assert_eq!(FontType::from_subtype("Type1C"), FontType::Type1C);
        assert_eq!(FontType::from_subtype("TrueType"), FontType::TrueType);
        assert_eq!(FontType::from_subtype("Unknown"), FontType::Unknown);
    }

    #[test]
    fn test_font_type_is_cid_font() {
        assert!(FontType::CIDFontType0.is_cid_font());
        assert!(FontType::CIDFontType2.is_cid_font());
        assert!(!FontType::Type1.is_cid_font());
        assert!(!FontType::TrueType.is_cid_font());
    }

    #[test]
    fn test_font_dict_from_simple_font() {
        let mut dict = std::collections::HashMap::new();
        dict.insert("Type".to_string(), PDFObject::Name("Font".to_string()));
        dict.insert("Subtype".to_string(), PDFObject::Name("Type1".to_string()));
        dict.insert(
            "BaseFont".to_string(),
            PDFObject::Name("Helvetica".to_string()),
        );

        let font_dict = FontDict::from_pdf_object(&PDFObject::Dictionary(dict)).unwrap();

        assert_eq!(font_dict.font_type, FontType::Type1);
        assert_eq!(font_dict.base_font, "Helvetica");
        assert!(font_dict.encoding.is_none());
        assert!(font_dict.to_unicode.is_none());
    }

    #[test]
    fn test_font_dict_with_widths() {
        let mut dict = std::collections::HashMap::new();
        dict.insert("Subtype".to_string(), PDFObject::Name("Type1".to_string()));
        dict.insert(
            "BaseFont".to_string(),
            PDFObject::Name("CustomFont".to_string()),
        );
        dict.insert("FirstChar".to_string(), PDFObject::Number(32.0));
        dict.insert("LastChar".to_string(), PDFObject::Number(34.0));
        dict.insert(
            "Widths".to_string(),
            PDFObject::Array(smallvec![
                Box::new(PDFObject::Number(250.0)),
                Box::new(PDFObject::Number(300.0)),
                Box::new(PDFObject::Number(350.0)),
            ]),
        );

        let font_dict = FontDict::from_pdf_object(&PDFObject::Dictionary(dict)).unwrap();

        assert_eq!(font_dict.first_char, Some(32));
        assert_eq!(font_dict.last_char, Some(34));
        assert_eq!(font_dict.widths.as_ref().unwrap().len(), 3);
        assert_eq!(font_dict.widths.as_ref().unwrap()[0], 250.0);
        assert_eq!(font_dict.widths.as_ref().unwrap()[2], 350.0);
    }

    #[test]
    fn test_font_dict_default_values() {
        let dict = std::collections::HashMap::new();

        let font_dict = FontDict::from_pdf_object(&PDFObject::Dictionary(dict)).unwrap();

        assert_eq!(font_dict.font_type, FontType::Unknown);
        assert_eq!(font_dict.base_font, "Unknown");
        assert_eq!(font_dict.default_width, 250.0);
    }
}
