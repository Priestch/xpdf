//! PDF outline (bookmark) parsing
//!
//! This module implements parsing of PDF document outlines/bookmarks following
//! the PDF specification and PDF.js's implementation (catalog.js #readDocumentOutline).
//!
//! Outlines provide a hierarchical table of contents for navigating PDF documents.

use crate::core::error::{PDFError, PDFResult};
use crate::core::parser::{PDFObject, Ref};
use crate::core::PDFDocument;
use std::collections::{HashMap, HashSet};

/// Decodes a PDF string to a Rust String, handling various encodings.
///
/// PDF strings can be encoded in several ways:
/// - PDFDocEncoding (Latin-1 based)
/// - UTF-16BE with BOM (0xFE 0xFF)
/// - UTF-16LE with BOM (0xFF 0xFE)
/// - UTF-8 with BOM (0xEF 0xBB 0xBF)
///
/// Based on PDF.js's stringToPDFString function.
fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    // Check for BOM (Byte Order Mark)
    if bytes.len() >= 2 {
        // UTF-16BE BOM
        if bytes[0] == 0xFE && bytes[1] == 0xFF {
            if bytes.len() % 2 != 0 {
                // Remove trailing byte if odd length
                let data = &bytes[..bytes.len() - 1];
                return decode_utf16be(data).unwrap_or_else(|_| String::from_utf8_lossy(data).to_string());
            }
            return decode_utf16be(bytes).unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string());
        }

        // UTF-16LE BOM
        if bytes[0] == 0xFF && bytes[1] == 0xFE {
            if bytes.len() % 2 != 0 {
                let data = &bytes[..bytes.len() - 1];
                return decode_utf16le(data).unwrap_or_else(|_| String::from_utf8_lossy(data).to_string());
            }
            return decode_utf16le(bytes).unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string());
        }
    }

    if bytes.len() >= 3 {
        // UTF-8 BOM
        if bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
            return String::from_utf8_lossy(&bytes[3..]).to_string();
        }
    }

    // Check if the string might be UTF-16BE without BOM
    // (common in PDFs - first byte > 0x7F indicates potential UTF-16)
    if bytes[0] > 0x7F && bytes.len() >= 2 {
        // Try UTF-16BE
        if let Ok(decoded) = decode_utf16be(bytes) {
            // Only use if it looks like valid UTF-16 (no replacement characters)
            if !decoded.contains('\u{FFFD}') {
                return remove_escape_sequences(decoded);
            }
        }
    }

    // Fallback: Try UTF-8 first, then PDFDocEncoding (Latin-1)
    if let Ok(utf8_str) = std::str::from_utf8(bytes) {
        // Check if it's valid UTF-8
        if !utf8_str.contains('\u{FFFD}') {
            return remove_escape_sequences(utf8_str.to_string());
        }
    }

    // Last resort: treat as PDFDocEncoding (Latin-1)
    // PDFDocEncoding is similar to Latin-1 but with some differences
    let decoded: String = bytes
        .iter()
        .map(|&b| b as char)
        .collect();

    remove_escape_sequences(decoded)
}

/// Decodes bytes as UTF-16BE
fn decode_utf16be(bytes: &[u8]) -> Result<String, std::string::FromUtf16Error> {
    if bytes.len() % 2 != 0 {
        // Odd length, can't be valid UTF-16
        return Ok(String::from_utf8_lossy(bytes).to_string());
    }

    let utf16_chars: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();

    String::from_utf16(&utf16_chars)
}

/// Decodes bytes as UTF-16LE
fn decode_utf16le(bytes: &[u8]) -> Result<String, std::string::FromUtf16Error> {
    if bytes.len() % 2 != 0 {
        return Ok(String::from_utf8_lossy(bytes).to_string());
    }

    let utf16_chars: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    String::from_utf16(&utf16_chars)
}

/// Removes PDF escape sequences (0x1b) from the string.
/// These are used for language codes and other metadata.
fn remove_escape_sequences(s: String) -> String {
    // Remove sequences like 0x1b ... 0x1b
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    let mut in_escape = false;

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            in_escape = true;
            continue;
        }

        if in_escape {
            // Skip characters until we find another 0x1b or end
            if c == '\x1b' {
                in_escape = false;
            }
            continue;
        }

        result.push(c);
    }

    result
}

/// Represents a single outline item (bookmark) in a PDF document.
///
/// Outlines form a hierarchical tree structure where each item can have
/// children and point to a destination within the document.
#[derive(Debug, Clone)]
pub struct OutlineItem {
    /// The title text displayed for this bookmark
    pub title: String,

    /// The destination this bookmark points to (page, location, etc.)
    pub dest: Option<OutlineDestination>,

    /// RGB color for the bookmark text (defaults to black [0, 0, 0])
    pub color: Option<[u8; 3]>,

    /// Whether the bookmark text should be bold
    pub bold: bool,

    /// Whether the bookmark text should be italic
    pub italic: bool,

    /// Number of visible descendants (negative if closed by default)
    pub count: Option<i32>,

    /// Child outline items (nested bookmarks)
    pub children: Vec<OutlineItem>,
}

/// The destination an outline item points to.
#[derive(Debug, Clone)]
pub enum OutlineDestination {
    /// Explicit destination with page index and destination type
    Explicit {
        /// Zero-based page index
        page_index: usize,
        /// Type of destination and its parameters
        dest_type: DestinationType,
    },

    /// Named destination (requires lookup in /Dests dictionary)
    Named(String),

    /// External URL (from /URI action)
    URL(String),

    /// GoTo action (remote PDF or embedded file)
    GoToRemote {
        /// URL or file path
        url: String,
        /// Destination within the remote document (optional)
        dest: Option<String>,
        /// Whether to open in new window
        new_window: bool,
    },
}

/// Types of explicit destinations in PDF.
///
/// These determine how the destination page is displayed when navigating.
#[derive(Debug, Clone)]
pub enum DestinationType {
    /// XYZ destination - explicit left, top, and zoom coordinates
    /// [page, /XYZ, left, top, zoom]
    XYZ {
        left: Option<f64>,
        top: Option<f64>,
        zoom: Option<f64>,
    },

    /// Fit destination - fit page to window
    /// [page, /Fit]
    Fit,

    /// FitH destination - fit page horizontally
    /// [page, /FitH, top]
    FitH { top: Option<f64> },

    /// FitV destination - fit page vertically
    /// [page, /FitV, left]
    FitV { left: Option<f64> },

    /// FitB destination - fit bounding box to window
    /// [page, /FitB]
    FitB,

    /// FitBH destination - fit bounding box horizontally
    /// [page, /FitBH, top]
    FitBH { top: Option<f64> },

    /// FitBV destination - fit bounding box vertically
    /// [page, /FitBV, left]
    FitBV { left: Option<f64> },
}

impl OutlineItem {
    /// Creates a new outline item with minimal required fields.
    pub fn new(title: String) -> Self {
        Self {
            title,
            dest: None,
            color: None,
            bold: false,
            italic: false,
            count: None,
            children: Vec::new(),
        }
    }
}

/// Parses the document outline from a PDF document.
///
/// This function follows PDF.js's #readDocumentOutline algorithm:
/// 1. Get /Outlines from catalog
/// 2. Get /First reference (first top-level item)
/// 3. Use queue-based traversal to avoid recursion
/// 4. Track visited references to prevent cycles
/// 5. Parse title, dest, color, flags, count for each item
/// 6. Follow /First (children) and /Next (siblings) links
///
/// # Returns
///
/// - `Ok(Some(items))` - Outline items found
/// - `Ok(None)` - No outline in document
/// - `Err` - Parsing error
pub fn parse_document_outline(doc: &mut PDFDocument) -> PDFResult<Option<Vec<OutlineItem>>> {
    // Get the /Outlines dictionary from the catalog
    let outlines_obj = match doc.document_outline()? {
        Some(o) => o,
        None => return Ok(None),
    };

    // Get /First reference (first top-level outline item)
    let first_ref = match &outlines_obj {
        PDFObject::Dictionary(dict) => match dict.get("First") {
            Some(PDFObject::Ref(ref_obj)) => Some((ref_obj.num, ref_obj.generation)),
            _ => None,
        },
        _ => None,
    };

    let first_ref = match first_ref {
        Some(r) => r,
        None => return Ok(None), // No outline items
    };

    // Queue-based traversal: each entry is (outline_ref, parent_item)
    // We use indices into a Vec to avoid lifetime issues with references
    let mut queue: Vec<((u32, u32), usize)> = vec![(first_ref, 0)];
    let mut items: Vec<OutlineItem> = vec![OutlineItem::new(String::new())]; // Root placeholder at index 0

    // Track visited references to prevent cycles
    let mut visited: HashSet<(u32, u32)> = HashSet::new();
    visited.insert(first_ref);

    while let Some((ref_num_gen, parent_idx)) = queue.pop() {
        let (num, generation) = ref_num_gen;

        // Fetch the outline dictionary
        let outline_dict = match doc.xref_mut().fetch_if_ref(&PDFObject::Ref(Ref::new(num, generation))) {
            Ok(PDFObject::Dictionary(dict)) => dict,
            Ok(_) => continue, // Not a dictionary, skip
            Err(PDFError::DataMissing { .. }) => return Err(PDFError::DataMissing {
                position: 0,
                length: 0,
            }),
            Err(_) => continue,
        };

        // Title is required
        let title = match outline_dict.get("Title") {
            Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                decode_pdf_string(bytes)
            }
            _ => {
                // Invalid outline item, skip
                continue;
            }
        };

        // Parse destination
        let dest = parse_destination(&outline_dict, doc)?;

        // Parse color (optional, defaults to black)
        let color = parse_color(&outline_dict);

        // Parse flags (bit 1 = bold, bit 0 = italic)
        let (bold, italic) = parse_flags(&outline_dict);

        // Parse count (number of descendants, negative = closed)
        let count = outline_dict.get("Count").and_then(|c| {
            if let PDFObject::Number(n) = c {
                Some(*n as i32)
            } else {
                None
            }
        });

        // Create the outline item
        let mut item = OutlineItem::new(title);
        item.dest = dest;
        item.color = color;
        item.bold = bold;
        item.italic = italic;
        item.count = count;

        // Add item to its parent
        let item_idx = items.len();
        items.push(item);

        // Clone the item and add to parent (avoids borrow checker issues)
        let item_clone = items[item_idx].clone();
        items[parent_idx].children.push(item_clone);

        // Add /First (children) to queue
        if let Some(PDFObject::Ref(ref_obj)) = outline_dict.get("First") {
            let ref_tuple = (ref_obj.num, ref_obj.generation);
            if !visited.contains(&ref_tuple) {
                visited.insert(ref_tuple);
                queue.push((ref_tuple, item_idx));
            }
        }

        // Add /Next (siblings) to queue
        if let Some(PDFObject::Ref(ref_obj)) = outline_dict.get("Next") {
            let ref_tuple = (ref_obj.num, ref_obj.generation);
            if !visited.contains(&ref_tuple) {
                visited.insert(ref_tuple);
                queue.push((ref_tuple, parent_idx));
            }
        }
    }

    // Return root's children (top-level items)
    Ok(Some(items.remove(0).children))
}

/// Parses the destination from an outline dictionary.
///
/// Handles:
/// - /Dest entry (explicit destination array or named destination)
/// - /A entry with various action types (URI, GoTo, GoToR, etc.)
fn parse_destination(
    dict: &HashMap<String, PDFObject>,
    doc: &mut PDFDocument,
) -> PDFResult<Option<OutlineDestination>> {
    // Check for /Dest entry first
    if let Some(dest_obj) = dict.get("Dest") {
        return Ok(Some(parse_dest_entry(dest_obj, doc)?));
    }

    // Check for /A (action) entry
    if let Some(action_obj) = dict.get("A") {
        if let PDFObject::Dictionary(action_dict) = action_obj {
            return parse_action_destination(action_dict, doc);
        }
    }

    Ok(None)
}

/// Parses a /Dest entry (can be explicit array or named destination).
fn parse_dest_entry(dest_obj: &PDFObject, doc: &mut PDFDocument) -> PDFResult<OutlineDestination> {
    match dest_obj {
        PDFObject::Array(arr) => {
            // Explicit destination: [page_ref, /Type, params...]
            if arr.is_empty() {
                return Ok(OutlineDestination::Named(String::new()));
            }

            // First element is the page reference
            let page_ref = &arr[0];

            // Resolve page reference to page index
            let page_index = resolve_page_index(page_ref, doc)?;

            // Second element is the destination type name
            let dest_type = if arr.len() > 1 {
                match &*arr[1] {
                    PDFObject::Name(name) => name.as_str(),
                    _ => return Ok(OutlineDestination::Named(String::new())),
                }
            } else {
                return Ok(OutlineDestination::Named(String::new()));
            };

            // Parse the destination type and parameters
            let dest_type = parse_destination_type(dest_type, &arr[2..])?;

            Ok(OutlineDestination::Explicit {
                page_index,
                dest_type,
            })
        }
        PDFObject::String(bytes) | PDFObject::HexString(bytes) => {
            // Named destination - try to resolve it
            let name = String::from_utf8_lossy(bytes).to_string();

            // Try to resolve the named destination
            match doc.resolve_named_destination(&name) {
                Ok(Some((page_index, dest_type))) => {
                    Ok(OutlineDestination::Explicit {
                        page_index,
                        dest_type,
                    })
                }
                _ => {
                    // Could not resolve, keep as named destination
                    Ok(OutlineDestination::Named(name))
                }
            }
        }
        PDFObject::Name(name) => {
            // Named destination (as a name object) - try to resolve it
            // Remove leading '/' if present
            let clean_name = if name.starts_with('/') {
                &name[1..]
            } else {
                name
            };

            match doc.resolve_named_destination(clean_name) {
                Ok(Some((page_index, dest_type))) => {
                    Ok(OutlineDestination::Explicit {
                        page_index,
                        dest_type,
                    })
                }
                _ => {
                    // Could not resolve, keep as named destination
                    Ok(OutlineDestination::Named(name.clone()))
                }
            }
        }
        _ => Ok(OutlineDestination::Named(String::new())),
    }
}

/// Parses an action dictionary from /A entry.
fn parse_action_destination(
    action_dict: &HashMap<String, PDFObject>,
    doc: &mut PDFDocument,
) -> PDFResult<Option<OutlineDestination>> {
    // Get action type (/S)
    let action_type = match action_dict.get("S") {
        Some(PDFObject::Name(name)) => name.as_str(),
        _ => return Ok(None),
    };

    match action_type {
        "URI" => {
            // URI action
            if let Some(uri_obj) = action_dict.get("URI") {
                let uri = match uri_obj {
                    PDFObject::String(bytes) | PDFObject::HexString(bytes) => {
                        String::from_utf8_lossy(bytes).to_string()
                    }
                    PDFObject::Name(name) => format!("/{}", name),
                    _ => String::new(),
                };
                return Ok(Some(OutlineDestination::URL(uri)));
            }
        }
        "GoTo" => {
            // GoTo action - has /D entry with destination
            if let Some(dest_obj) = action_dict.get("D") {
                return Ok(Some(parse_dest_entry(dest_obj, doc)?));
            }
        }
        "GoToR" => {
            // GoToR (remote PDF)
            let url = match action_dict.get("F") {
                Some(PDFObject::String(bytes)) => {
                    String::from_utf8_lossy(bytes).to_string()
                }
                _ => String::new(),
            };

            // Check for NewWindow flag
            let new_window = match action_dict.get("NewWindow") {
                Some(PDFObject::Boolean(b)) => *b,
                _ => false,
            };

            return Ok(Some(OutlineDestination::GoToRemote {
                url,
                dest: None, // MVP: don't parse remote destination
                new_window,
            }));
        }
        // Other action types (Launch, GoToE, JavaScript, etc.) not implemented for MVP
        _ => {}
    }

    Ok(None)
}

/// Parses the destination type and parameters from an explicit destination array.
pub fn parse_destination_type(
    type_name: &str,
    params: &[Box<PDFObject>],
) -> PDFResult<DestinationType> {
    let get_num = |idx: usize| -> Option<f64> {
        params
            .get(idx)
            .and_then(|p| match &**p {
                PDFObject::Number(n) => Some(*n),
                _ => None,
            })
    };

    Ok(match type_name {
        "XYZ" => DestinationType::XYZ {
            left: get_num(0),
            top: get_num(1),
            zoom: get_num(2),
        },
        "Fit" => DestinationType::Fit,
        "FitH" => DestinationType::FitH { top: get_num(0) },
        "FitV" => DestinationType::FitV { left: get_num(0) },
        "FitB" => DestinationType::FitB,
        "FitBH" => DestinationType::FitBH { top: get_num(0) },
        "FitBV" => DestinationType::FitBV { left: get_num(0) },
        _ => DestinationType::Fit, // Default to Fit for unknown types
    })
}

/// Resolves a page reference to a zero-based page index.
///
/// This uses the document's page reference cache to efficiently look up
/// page indices from page object references.
fn resolve_page_index(page_ref: &PDFObject, doc: &mut PDFDocument) -> PDFResult<usize> {
    match page_ref {
        PDFObject::Ref(ref_obj) => {
            // Use the document's resolve_page_index method
            doc.resolve_page_index(ref_obj.num, ref_obj.generation)
                .ok_or_else(|| {
                    PDFError::Generic(format!(
                        "Page reference {} {} not found in document",
                        ref_obj.num, ref_obj.generation
                    ))
                })
        }
        _ => Err(PDFError::Generic(
            "Page reference is not a Ref object".to_string(),
        )),
    }
}

/// Parses the /C entry (color) from an outline dictionary.
///
/// Returns RGB color as [r, g, b] or None if default black.
fn parse_color(dict: &HashMap<String, PDFObject>) -> Option<[u8; 3]> {
    dict.get("C").and_then(|color_obj| {
        if let PDFObject::Array(arr) = color_obj {
            if arr.len() >= 3 {
                let r = match &*arr[0] {
                    PDFObject::Number(n) => (*n * 255.0) as u8,
                    _ => 0,
                };
                let g = match &*arr[1] {
                    PDFObject::Number(n) => (*n * 255.0) as u8,
                    _ => 0,
                };
                let b = match &*arr[2] {
                    PDFObject::Number(n) => (*n * 255.0) as u8,
                    _ => 0,
                };

                // Only return non-default colors (not black)
                if r != 0 || g != 0 || b != 0 {
                    return Some([r, g, b]);
                }
            }
        }
        None
    })
}

/// Parses the /F entry (flags) from an outline dictionary.
///
/// Returns (bold, italic) tuple.
fn parse_flags(dict: &HashMap<String, PDFObject>) -> (bool, bool) {
    let flags = dict
        .get("F")
        .and_then(|f| {
            if let PDFObject::Number(n) = f {
                Some(*n as i32)
            } else {
                None
            }
        })
        .unwrap_or(0);

    // Bit 1 = bold, Bit 0 = italic
    let bold = (flags & 2) != 0;
    let italic = (flags & 1) != 0;

    (bold, italic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;

    #[test]
    fn test_outline_item_new() {
        let item = OutlineItem::new("Test".to_string());
        assert_eq!(item.title, "Test");
        assert!(item.dest.is_none());
        assert!(item.color.is_none());
        assert!(!item.bold);
        assert!(!item.italic);
        assert!(item.count.is_none());
        assert!(item.children.is_empty());
    }

    #[test]
    fn test_parse_flags() {
        let mut dict = HashMap::new();

        // No flags
        assert_eq!(parse_flags(&dict), (false, false));

        // Italic (bit 0)
        dict.insert("F".to_string(), PDFObject::Number(1.0));
        assert_eq!(parse_flags(&dict), (false, true));

        // Bold (bit 1)
        dict.insert("F".to_string(), PDFObject::Number(2.0));
        assert_eq!(parse_flags(&dict), (true, false));

        // Bold and italic
        dict.insert("F".to_string(), PDFObject::Number(3.0));
        assert_eq!(parse_flags(&dict), (true, true));
    }

    #[test]
    fn test_parse_color() {
        let mut dict = HashMap::new();

        // No color
        assert!(parse_color(&dict).is_none());

        // Black (default)
        dict.insert(
            "C".to_string(),
            PDFObject::Array(SmallVec::from_vec(vec![
                Box::new(PDFObject::Number(0.0)),
                Box::new(PDFObject::Number(0.0)),
                Box::new(PDFObject::Number(0.0)),
            ])),
        );
        assert!(parse_color(&dict).is_none());

        // Red
        dict.insert(
            "C".to_string(),
            PDFObject::Array(SmallVec::from_vec(vec![
                Box::new(PDFObject::Number(1.0)),
                Box::new(PDFObject::Number(0.0)),
                Box::new(PDFObject::Number(0.0)),
            ])),
        );
        assert_eq!(parse_color(&dict), Some([255, 0, 0]));

        // Gray (0.5, 0.5, 0.5)
        dict.insert(
            "C".to_string(),
            PDFObject::Array(SmallVec::from_vec(vec![
                Box::new(PDFObject::Number(0.5)),
                Box::new(PDFObject::Number(0.5)),
                Box::new(PDFObject::Number(0.5)),
            ])),
        );
        assert_eq!(parse_color(&dict), Some([127, 127, 127]));
    }

    #[test]
    fn test_parse_destination_type_xyz() {
        let arr = vec![
            Box::new(PDFObject::Number(100.0)),
            Box::new(PDFObject::Number(200.0)),
            Box::new(PDFObject::Number(1.5)),
        ];

        let result = parse_destination_type("XYZ", &arr).unwrap();
        match result {
            DestinationType::XYZ { left, top, zoom } => {
                assert_eq!(left, Some(100.0));
                assert_eq!(top, Some(200.0));
                assert_eq!(zoom, Some(1.5));
            }
            _ => panic!("Expected XYZ destination"),
        }
    }

    #[test]
    fn test_parse_destination_type_fit() {
        let result = parse_destination_type("Fit", &[]).unwrap();
        assert!(matches!(result, DestinationType::Fit));
    }

    #[test]
    fn test_parse_destination_type_fith() {
        let arr = vec![Box::new(PDFObject::Number(300.0))];
        let result = parse_destination_type("FitH", &arr).unwrap();
        match result {
            DestinationType::FitH { top } => {
                assert_eq!(top, Some(300.0));
            }
            _ => panic!("Expected FitH destination"),
        }
    }
}
