//! CMap (Character Map) parsing for font encoding support.
//!
//! CMaps map character codes (CIDs) to Unicode values. They are used in PDF
//! fonts to enable text extraction. The /ToUnicode entry in a font dictionary
//! points to a CMap stream that defines these mappings.
//!
//! Based on PDF.js's CMap parser in src/core/cmap.js

use crate::core::error::{PDFError, PDFResult};
use rustc_hash::FxHashMap;

/// CMap (Character Map) for mapping character codes to Unicode.
///
/// CMaps support two types of mappings:
/// - **bfchar**: Single character mappings (CID -> Unicode)
/// - **bfrange**: Range mappings (CID range -> Unicode range)
///
/// Example CMap stream:
/// ```text
/// /CIDInit /ProcSet findresource begin
/// 12 dict begin
/// begincmap
/// /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/// /CMapName /Adobe-Identity-UCS def
/// /CMapType 2 def
/// 1 begincodespacerange
/// <0000> <FFFF>
/// endcodespacerange
/// 2 beginbfchar
/// <0003> <0020>
/// <0005> <0041>
/// endbfchar
/// 1 beginbfrange
/// <0010> <0020> <0030>
/// endbfrange
/// endcmap
/// ```
pub struct CMap {
    /// CID → Unicode mappings
    mappings: FxHashMap<u16, char>,
}

impl CMap {
    /// Creates an empty CMap.
    pub fn new() -> Self {
        CMap {
            mappings: FxHashMap::default(),
        }
    }

    /// Parses a /ToUnicode CMap stream.
    ///
    /// # Arguments
    /// * `stream_data` - The decompressed CMap stream data
    ///
    /// # Returns
    /// A CMap with all character mappings extracted
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::cmap::CMap;
    ///
    /// let cmap_data = b"1 beginbfchar\n<03> <0020>\nendbfchar\n";
    /// let cmap = CMap::parse(cmap_data).unwrap();
    /// assert_eq!(cmap.to_unicode(3), Some(' '));
    /// ```
    pub fn parse(stream_data: &[u8]) -> PDFResult<Self> {
        let mut cmap = CMap::new();
        let content = std::str::from_utf8(stream_data)
            .map_err(|e| PDFError::Generic(format!("Invalid UTF-8 in CMap stream: {}", e)))?;

        // Parse the CMap stream line by line
        let mut lines = content.lines().peekable();

        while let Some(line) = lines.next() {
            let trimmed = line.trim();

            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with('%') {
                continue;
            }

            // Parse bfchar mappings (single character mappings)
            if trimmed.ends_with("beginbfchar") {
                let count = Self::parse_count(trimmed)?;
                Self::parse_bfchar(&mut cmap, &mut lines, count)?;
            }
            // Parse bfrange mappings (range mappings)
            else if trimmed.ends_with("beginbfrange") {
                let count = Self::parse_count(trimmed)?;
                Self::parse_bfrange(&mut cmap, &mut lines, count)?;
            }
        }

        Ok(cmap)
    }

    /// Parses the count from a "N beginbfchar" or "N beginbfrange" line.
    fn parse_count(line: &str) -> PDFResult<usize> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(PDFError::Generic(format!("Invalid CMap line: '{}'", line)));
        }

        parts[0]
            .parse::<usize>()
            .map_err(|_| PDFError::Generic(format!("Invalid count in CMap line: '{}'", line)))
    }

    /// Parses bfchar entries (single character mappings).
    ///
    /// Format: `<srcCode> <dstUnicode>`
    /// Example: `<0003> <0020>` maps CID 3 to Unicode U+0020 (space)
    fn parse_bfchar<'a, I>(cmap: &mut CMap, lines: &mut I, count: usize) -> PDFResult<()>
    where
        I: Iterator<Item = &'a str>,
    {
        for _ in 0..count {
            let line = lines
                .next()
                .ok_or_else(|| PDFError::Generic("Unexpected end of bfchar section".to_string()))?;

            let trimmed = line.trim();

            // Check for end marker
            if trimmed == "endbfchar" {
                break;
            }

            // Parse the mapping: <srcCode> <dstUnicode>
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() < 2 {
                continue; // Skip invalid lines
            }

            let src_code = Self::parse_hex_code(parts[0])?;
            let dst_unicode = Self::parse_hex_unicode(parts[1])?;

            cmap.mappings.insert(src_code, dst_unicode);
        }

        Ok(())
    }

    /// Parses bfrange entries (range mappings).
    ///
    /// Format: `<srcCodeLo> <srcCodeHi> <dstUnicode>`
    /// Example: `<0010> <0020> <0030>` maps CIDs 0x10-0x20 to Unicode U+0030-U+0040
    fn parse_bfrange<'a, I>(cmap: &mut CMap, lines: &mut I, count: usize) -> PDFResult<()>
    where
        I: Iterator<Item = &'a str>,
    {
        for _ in 0..count {
            let line = lines.next().ok_or_else(|| {
                PDFError::Generic("Unexpected end of bfrange section".to_string())
            })?;

            let trimmed = line.trim();

            // Check for end marker
            if trimmed == "endbfrange" {
                break;
            }

            // Parse the range: <srcCodeLo> <srcCodeHi> <dstUnicode>
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() < 3 {
                continue; // Skip invalid lines
            }

            let src_code_lo = Self::parse_hex_code(parts[0])?;
            let src_code_hi = Self::parse_hex_code(parts[1])?;
            let dst_unicode = Self::parse_hex_unicode(parts[2])?;

            // Map the range
            let dst_code = dst_unicode as u32;
            for (i, src_code) in (src_code_lo..=src_code_hi).enumerate() {
                if let Some(unicode_char) = char::from_u32(dst_code + i as u32) {
                    cmap.mappings.insert(src_code, unicode_char);
                }
            }
        }

        Ok(())
    }

    /// Parses a hex code like `<0003>` or `<03>` into a u16.
    #[inline]
    fn parse_hex_code(hex_str: &str) -> PDFResult<u16> {
        // Remove angle brackets
        let hex = hex_str.trim_start_matches('<').trim_end_matches('>');

        u16::from_str_radix(hex, 16)
            .map_err(|_| PDFError::Generic(format!("Invalid hex code: '{}'", hex_str)))
    }

    /// Parses a hex Unicode value like `<0020>` into a char.
    #[inline]
    fn parse_hex_unicode(hex_str: &str) -> PDFResult<char> {
        // Remove angle brackets
        let hex = hex_str.trim_start_matches('<').trim_end_matches('>');

        let code = u32::from_str_radix(hex, 16)
            .map_err(|_| PDFError::Generic(format!("Invalid hex Unicode: '{}'", hex_str)))?;

        char::from_u32(code)
            .ok_or_else(|| PDFError::Generic(format!("Invalid Unicode code point: 0x{:X}", code)))
    }

    /// Maps a character code (CID) to Unicode.
    ///
    /// # Arguments
    /// * `cid` - The character ID to look up
    ///
    /// # Returns
    /// The Unicode character if the mapping exists, otherwise None
    ///
    /// # Example
    /// ```no_run
    /// # use pdf_x::core::cmap::CMap;
    /// let cmap = CMap::parse(b"1 beginbfchar\n<03> <0020>\nendbfchar\n").unwrap();
    /// assert_eq!(cmap.to_unicode(3), Some(' '));
    /// assert_eq!(cmap.to_unicode(999), None);
    /// ```
    #[inline(always)] // Hot path - called for every character during text extraction
    pub fn to_unicode(&self, cid: u16) -> Option<char> {
        self.mappings.get(&cid).copied()
    }

    /// Returns the number of mappings in this CMap.
    pub fn len(&self) -> usize {
        self.mappings.len()
    }

    /// Returns true if this CMap has no mappings.
    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }
}

impl Default for CMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_bfchar() {
        let cmap_data = b"\
1 beginbfchar
<03> <0020>
endbfchar
";
        let cmap = CMap::parse(cmap_data).unwrap();
        assert_eq!(cmap.to_unicode(3), Some(' '));
        assert_eq!(cmap.to_unicode(4), None);
    }

    #[test]
    fn test_parse_multiple_bfchar() {
        let cmap_data = b"\
3 beginbfchar
<03> <0020>
<05> <0041>
<07> <0042>
endbfchar
";
        let cmap = CMap::parse(cmap_data).unwrap();
        assert_eq!(cmap.to_unicode(3), Some(' '));
        assert_eq!(cmap.to_unicode(5), Some('A'));
        assert_eq!(cmap.to_unicode(7), Some('B'));
        assert_eq!(cmap.len(), 3);
    }

    #[test]
    fn test_parse_bfrange() {
        let cmap_data = b"\
1 beginbfrange
<0010> <0012> <0041>
endbfrange
";
        let cmap = CMap::parse(cmap_data).unwrap();
        assert_eq!(cmap.to_unicode(0x10), Some('A'));
        assert_eq!(cmap.to_unicode(0x11), Some('B'));
        assert_eq!(cmap.to_unicode(0x12), Some('C'));
        assert_eq!(cmap.to_unicode(0x13), None);
    }

    #[test]
    fn test_parse_mixed_bfchar_and_bfrange() {
        let cmap_data = b"\
2 beginbfchar
<03> <0020>
<05> <0041>
endbfchar
1 beginbfrange
<0010> <0012> <0061>
endbfrange
";
        let cmap = CMap::parse(cmap_data).unwrap();

        // bfchar mappings
        assert_eq!(cmap.to_unicode(3), Some(' '));
        assert_eq!(cmap.to_unicode(5), Some('A'));

        // bfrange mappings
        assert_eq!(cmap.to_unicode(0x10), Some('a'));
        assert_eq!(cmap.to_unicode(0x11), Some('b'));
        assert_eq!(cmap.to_unicode(0x12), Some('c'));

        assert_eq!(cmap.len(), 5);
    }

    #[test]
    fn test_parse_with_comments_and_whitespace() {
        let cmap_data = b"\
% This is a comment
1 beginbfchar
  <03>   <0020>
endbfchar

% Another comment
";
        let cmap = CMap::parse(cmap_data).unwrap();
        assert_eq!(cmap.to_unicode(3), Some(' '));
    }

    #[test]
    fn test_empty_cmap() {
        let cmap_data = b"";
        let cmap = CMap::parse(cmap_data).unwrap();
        assert!(cmap.is_empty());
        assert_eq!(cmap.len(), 0);
    }

    #[test]
    fn test_parse_hex_codes() {
        // Test various hex formats
        assert_eq!(CMap::parse_hex_code("<03>").unwrap(), 3);
        assert_eq!(CMap::parse_hex_code("<0003>").unwrap(), 3);
        assert_eq!(CMap::parse_hex_code("<00AB>").unwrap(), 0xAB);
        assert_eq!(CMap::parse_hex_code("<FFFF>").unwrap(), 0xFFFF);
    }

    #[test]
    fn test_parse_hex_unicode() {
        assert_eq!(CMap::parse_hex_unicode("<0020>").unwrap(), ' ');
        assert_eq!(CMap::parse_hex_unicode("<0041>").unwrap(), 'A');
        assert_eq!(CMap::parse_hex_unicode("<4E2D>").unwrap(), '中');
    }

    #[test]
    fn test_invalid_hex_code() {
        assert!(CMap::parse_hex_code("<GGGG>").is_err());
        assert!(CMap::parse_hex_code("not-hex").is_err());
    }

    #[test]
    fn test_real_world_cmap() {
        // Simplified example from a real PDF
        let cmap_data = b"\
/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo
<< /Registry (Adobe)
/Ordering (UCS)
/Supplement 0
>> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
2 beginbfchar
<0003> <0020>
<0005> <0041>
endbfchar
1 beginbfrange
<0010> <0020> <0061>
endbfrange
endcmap
CMapName currentdict /CMap defineresource pop
end
end
";
        let cmap = CMap::parse(cmap_data).unwrap();

        // bfchar mappings
        assert_eq!(cmap.to_unicode(3), Some(' '));
        assert_eq!(cmap.to_unicode(5), Some('A'));

        // bfrange mappings (0x10-0x20 -> 0x61-0x71)
        assert_eq!(cmap.to_unicode(0x10), Some('a'));
        assert_eq!(cmap.to_unicode(0x15), Some('f'));
        assert_eq!(cmap.to_unicode(0x20), Some('q'));

        assert_eq!(cmap.len(), 19); // 2 bfchar + 17 bfrange
    }
}
