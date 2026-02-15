//! PDF writer for incremental updates.
//!
//! This module handles serializing the delta layer as PDF incremental updates.
//! Incremental updates append changes to the end of the original PDF file,
//! preserving the original data and following the PDF specification (section 7.5.6).
//!
//! ## Incremental Update Format
//!
//! ```text
//! [Original PDF Data]
//! [New Objects]
//! [New XRef Table]
//! [New Trailer pointing to new XRef]
//! %%EOF
//! ```

use super::delta::DeltaLayer;
use super::error::{PDFError, PDFResult};
use super::parser::PDFObject;
use std::collections::HashMap;
use std::io::Write;

/// PDF writer for incremental updates.
///
/// This writer serializes delta layer changes as PDF incremental updates,
/// which are appended to the original file without modifying existing data.
pub struct PDFWriter;

impl PDFWriter {
    /// Write an incremental update for the delta layer.
    ///
    /// This method creates a PDF incremental update containing:
    /// 1. All modified and new objects from the delta layer
    /// 2. A new xref table (hybrid format supporting both compressed and uncompressed)
    /// 3. A new trailer pointing to the new xref table
    ///
    /// # Arguments
    /// * `delta` - The delta layer to serialize
    /// * `original_size` - The size of the original PDF file (for xref offset)
    /// * `total_object_count` - Total number of objects in the document (original + new)
    /// * `prev_xref_offset` - The offset of the previous xref table (from original trailer)
    ///
    /// # Returns
    /// The incremental update as a byte vector that can be appended to the original PDF
    ///
    /// # Example
    /// ```no_run
    /// # use pdf_x_core::core::PDFWriter;
    /// # use pdf_x_core::core::DeltaLayer;
    /// # let delta = DeltaLayer::new(100);
    /// # let original_size = 5000;
    /// # let total_object_count = 100;
    /// # let prev_xref_offset = 4500;
    /// let update = PDFWriter::write_incremental_update(&delta, original_size, total_object_count, prev_xref_offset)?;
    /// // Append update to original PDF file
    /// # std::io::Result::Ok(())
    /// ```
    pub fn write_incremental_update(
        delta: &DeltaLayer,
        original_size: usize,
        total_object_count: u32,
        prev_xref_offset: usize,
    ) -> PDFResult<Vec<u8>> {
        let mut buffer = Vec::new();

        // Track object offsets for the new xref table
        let mut object_offsets: HashMap<(u32, u32), u64> = HashMap::new();
        let mut current_offset = original_size as u64;

        // Collect all objects to write (modified + new)
        let objects_to_write: Vec<_> = delta
            .iter_modified()
            .map(|(id, obj)| (*id, obj))
            .chain(
                delta
                    .iter_new_objects()
                    .map(|obj| ((obj.obj_num, obj.generation), obj)),
            )
            .collect();

        // Write each object
        for (obj_id, delta_obj) in &objects_to_write {
            object_offsets.insert(*obj_id, current_offset);

            // Write object header: "N G obj"
            write!(buffer, "{} {} obj\n", obj_id.0, obj_id.1)
                .map_err(|e| PDFError::Generic(format!("Failed to write object header: {}", e)))?;

            // Write object content
            Self::write_object(&mut buffer, &delta_obj.object)?;

            // Write object footer
            buffer.extend_from_slice(b"endobj\n");

            // Update offset (account for what we just wrote)
            current_offset = (original_size as u64) + (buffer.len() as u64);
        }

        // Write the new xref table
        let xref_start_offset = (original_size as u64) + (buffer.len() as u64);
        Self::write_xref_table(
            &mut buffer,
            &object_offsets,
            original_size,
            prev_xref_offset,
        )?;

        // Write the new trailer
        Self::write_trailer(
            &mut buffer,
            xref_start_offset,
            total_object_count,
            prev_xref_offset,
        )?;

        // Write EOF marker
        buffer.extend_from_slice(b"%%EOF\n");

        Ok(buffer)
    }

    /// Write a PDF object to the buffer.
    fn write_object<W: Write>(buffer: &mut W, obj: &PDFObject) -> PDFResult<()> {
        match obj {
            PDFObject::Null => {
                buffer
                    .write_all(b"null")
                    .map_err(|e| PDFError::Generic(format!("Failed to write null: {}", e)))?;
            }
            PDFObject::Boolean(b) => {
                write!(buffer, "{}", if *b { "true" } else { "false" })
                    .map_err(|e| PDFError::Generic(format!("Failed to write boolean: {}", e)))?;
            }
            PDFObject::Number(n) => {
                // Write integers without decimal point
                if n.fract() == 0.0 {
                    write!(buffer, "{}", *n as i64)
                } else {
                    write!(buffer, "{}", n)
                }
                .map_err(|e| PDFError::Generic(format!("Failed to write number: {}", e)))?;
            }
            PDFObject::String(s) => {
                // Write as literal string with parentheses
                buffer.write_all(b"(").map_err(|e| {
                    PDFError::Generic(format!("Failed to write string prefix: {}", e))
                })?;
                Self::write_escaped_string(buffer, s)?;
                buffer.write_all(b")").map_err(|e| {
                    PDFError::Generic(format!("Failed to write string suffix: {}", e))
                })?;
            }
            PDFObject::HexString(s) => {
                // Write as hex string with angle brackets
                buffer
                    .write_all(b"<")
                    .map_err(|e| PDFError::Generic(format!("Failed to write hex prefix: {}", e)))?;
                for byte in s {
                    write!(buffer, "{:02X}", byte).map_err(|e| {
                        PDFError::Generic(format!("Failed to write hex byte: {}", e))
                    })?;
                }
                buffer
                    .write_all(b">")
                    .map_err(|e| PDFError::Generic(format!("Failed to write hex suffix: {}", e)))?;
            }
            PDFObject::Name(name) => {
                // Write name with leading slash
                buffer.write_all(b"/").map_err(|e| {
                    PDFError::Generic(format!("Failed to write name prefix: {}", e))
                })?;
                Self::write_escaped_name(buffer, name)?;
            }
            PDFObject::Array(arr) => {
                buffer.write_all(b"[").map_err(|e| {
                    PDFError::Generic(format!("Failed to write array prefix: {}", e))
                })?;
                for (i, item) in arr.iter().enumerate() {
                    if i > 0 {
                        buffer.write_all(b" ").map_err(|e| {
                            PDFError::Generic(format!("Failed to write array separator: {}", e))
                        })?;
                    }
                    Self::write_object(buffer, item)?;
                }
                buffer.write_all(b"]").map_err(|e| {
                    PDFError::Generic(format!("Failed to write array suffix: {}", e))
                })?;
            }
            PDFObject::Dictionary(dict) => {
                buffer.write_all(b"<<").map_err(|e| {
                    PDFError::Generic(format!("Failed to write dict prefix: {}", e))
                })?;
                for (key, value) in dict {
                    // Write key (name)
                    buffer.write_all(b"/").map_err(|e| {
                        PDFError::Generic(format!("Failed to write key prefix: {}", e))
                    })?;
                    Self::write_escaped_name(buffer, key)?;

                    // Write space separator
                    buffer.write_all(b" ").map_err(|e| {
                        PDFError::Generic(format!("Failed to write separator: {}", e))
                    })?;

                    // Write value
                    Self::write_object(buffer, value)?;

                    // Write space separator
                    buffer.write_all(b" ").map_err(|e| {
                        PDFError::Generic(format!("Failed to write separator: {}", e))
                    })?;
                }
                buffer.write_all(b">>").map_err(|e| {
                    PDFError::Generic(format!("Failed to write dict suffix: {}", e))
                })?;
            }
            PDFObject::Stream { dict, data } => {
                // Write stream dictionary
                Self::write_object(buffer, &PDFObject::Dictionary(dict.clone()))?;

                buffer.write_all(b"\nstream\n").map_err(|e| {
                    PDFError::Generic(format!("Failed to write stream prefix: {}", e))
                })?;
                buffer.write_all(data).map_err(|e| {
                    PDFError::Generic(format!("Failed to write stream data: {}", e))
                })?;
                buffer.write_all(b"\nendstream").map_err(|e| {
                    PDFError::Generic(format!("Failed to write stream suffix: {}", e))
                })?;
            }
            PDFObject::Ref(r) => {
                write!(buffer, "{} {} R", r.num, r.generation)
                    .map_err(|e| PDFError::Generic(format!("Failed to write reference: {}", e)))?;
            }
            PDFObject::EOF => {
                return Err(PDFError::Generic(
                    "Cannot write EOF marker as object".into(),
                ));
            }
            PDFObject::Command(_) => {
                return Err(PDFError::Generic(
                    "Cannot write command as PDF object".into(),
                ));
            }
        }

        Ok(())
    }

    /// Write an escaped literal string.
    ///
    /// PDF strings use backslash escaping for special characters.
    fn write_escaped_string<W: Write>(buffer: &mut W, s: &[u8]) -> PDFResult<()> {
        for &byte in s {
            match byte {
                b'(' => buffer.write_all(b"\\(").map_err(|e| {
                    PDFError::Generic(format!("Failed to write escaped paren: {}", e))
                })?,
                b')' => buffer.write_all(b"\\)").map_err(|e| {
                    PDFError::Generic(format!("Failed to write escaped paren: {}", e))
                })?,
                b'\\' => buffer.write_all(b"\\\\").map_err(|e| {
                    PDFError::Generic(format!("Failed to write escaped backslash: {}", e))
                })?,
                b'\n' => buffer.write_all(b"\\n").map_err(|e| {
                    PDFError::Generic(format!("Failed to write escaped newline: {}", e))
                })?,
                b'\r' => buffer
                    .write_all(b"\\r")
                    .map_err(|e| PDFError::Generic(format!("Failed to write escaped cr: {}", e)))?,
                b'\t' => buffer.write_all(b"\\t").map_err(|e| {
                    PDFError::Generic(format!("Failed to write escaped tab: {}", e))
                })?,
                _ => buffer
                    .write_all(&[byte])
                    .map_err(|e| PDFError::Generic(format!("Failed to write byte: {}", e)))?,
            };
        }
        Ok(())
    }

    /// Write an escaped name.
    ///
    /// PDF names use #XX escaping for special characters.
    fn write_escaped_name<W: Write>(buffer: &mut W, name: &str) -> PDFResult<()> {
        for byte in name.bytes() {
            match byte {
                b'/' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%' | b'#' => {
                    // Escape as #XX
                    write!(buffer, "#{:02X}", byte).map_err(|e| {
                        PDFError::Generic(format!("Failed to write escaped char: {}", e))
                    })?;
                }
                b' ' => {
                    // Space can't be in a name, but handle it gracefully
                    write!(buffer, "#20").map_err(|e| {
                        PDFError::Generic(format!("Failed to write escaped space: {}", e))
                    })?;
                }
                _ => buffer
                    .write_all(&[byte])
                    .map_err(|e| PDFError::Generic(format!("Failed to write name byte: {}", e)))?,
            };
        }
        Ok(())
    }

    /// Write a cross-reference table.
    ///
    /// This writes a hybrid xref table that can reference both objects in the
    /// original PDF and new/modified objects in the incremental update.
    ///
    /// Format per PDF 1.5+ specification (hybrid xref):
    /// ```text
    /// xref
    /// start_index count
    /// offset generation n  (for in-use objects)
    /// 0000000000 65535 f   (for free objects)
    /// ```
    fn write_xref_table<W: Write>(
        buffer: &mut W,
        object_offsets: &HashMap<(u32, u32), u64>,
        original_size: usize,
        prev_xref_offset: usize,
    ) -> PDFResult<()> {
        buffer
            .write_all(b"xref\n")
            .map_err(|e| PDFError::Generic(format!("Failed to write xref header: {}", e)))?;

        // Collect all object numbers we need to reference
        let mut obj_nums: Vec<u32> = object_offsets.keys().map(|(num, _)| *num).collect();
        obj_nums.sort();

        // Group consecutive objects into subsections
        // PDF spec requires subsection headers: "start_index count"
        let mut subsection_start: Option<u32> = None;
        let mut subsection_objs: Vec<(u32, u64)> = Vec::new();

        for obj_num in obj_nums {
            let offset = object_offsets.get(&(obj_num, 0)).ok_or_else(|| {
                PDFError::Generic(format!("Missing offset for object {}", obj_num))
            })?;

            match subsection_start {
                None => {
                    // Start a new subsection
                    subsection_start = Some(obj_num);
                    subsection_objs.push((obj_num, *offset));
                }
                Some(start) => {
                    let last_obj = subsection_objs.last().map(|(n, _)| *n).unwrap_or(start);
                    if obj_num == last_obj + 1 {
                        // Consecutive - add to current subsection
                        subsection_objs.push((obj_num, *offset));
                    } else {
                        // Non-consecutive - write current subsection and start new one
                        Self::write_xref_subsection(buffer, start, subsection_objs.len() as u32)?;
                        subsection_start = Some(obj_num);
                        subsection_objs = vec![(obj_num, *offset)];
                    }
                }
            }
        }

        // Write the last subsection
        if let Some(start) = subsection_start {
            Self::write_xref_subsection(buffer, start, subsection_objs.len() as u32)?;
            for (_obj_num, offset) in subsection_objs {
                // Format: offset (10 digits) + space + generation (5 digits) + space + type (n/f) + newline
                write!(buffer, "{:010} {:05} n \n", offset, 0)
                    .map_err(|e| PDFError::Generic(format!("Failed to write xref entry: {}", e)))?;
            }
        }

        Ok(())
    }

    /// Write a single xref subsection header.
    ///
    /// Format: "start_index count\n"
    fn write_xref_subsection<W: Write>(
        buffer: &mut W,
        start_index: u32,
        count: u32,
    ) -> PDFResult<()> {
        write!(buffer, "{} {}\n", start_index, count).map_err(|e| {
            PDFError::Generic(format!("Failed to write xref subsection header: {}", e))
        })
    }

    /// Write the trailer dictionary.
    ///
    /// The trailer points to the new xref table and includes a /Prev entry
    /// pointing to the previous xref table (for incremental update chain).
    fn write_trailer<W: Write>(
        buffer: &mut W,
        xref_start_offset: u64,
        total_object_count: u32,
        prev_xref_offset: usize,
    ) -> PDFResult<()> {
        buffer
            .write_all(b"trailer\n")
            .map_err(|e| PDFError::Generic(format!("Failed to write trailer header: {}", e)))?;
        buffer.write_all(b"<<").map_err(|e| {
            PDFError::Generic(format!("Failed to write trailer dict prefix: {}", e))
        })?;

        // Size: total number of objects (original + new)
        write!(buffer, "/Size {}", total_object_count)
            .map_err(|e| PDFError::Generic(format!("Failed to write /Size: {}", e)))?;

        // Previous: offset of previous xref table
        write!(buffer, " /Prev {}", prev_xref_offset)
            .map_err(|e| PDFError::Generic(format!("Failed to write /Prev: {}", e)))?;

        buffer.write_all(b">>\n").map_err(|e| {
            PDFError::Generic(format!("Failed to write trailer dict suffix: {}", e))
        })?;

        // Write startxref
        write!(buffer, "startxref\n{}\n", xref_start_offset)
            .map_err(|e| PDFError::Generic(format!("Failed to write startxref: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::parser::Ref; // Import Ref for test code
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_write_number_integer() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::Number(42.0)).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "42");
    }

    #[test]
    fn test_write_number_float() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::Number(3.14)).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "3.14");
    }

    #[test]
    fn test_write_boolean() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::Boolean(true)).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "true");

        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::Boolean(false)).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "false");
    }

    #[test]
    fn test_write_null() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::Null).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "null");
    }

    #[test]
    fn test_write_string() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::String(b"hello".to_vec())).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "(hello)");
    }

    #[test]
    fn test_write_string_with_special_chars() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::String(b"hello(world)".to_vec())).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), r"(hello\(world\))");
    }

    #[test]
    fn test_write_hex_string() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(
            &mut buffer,
            &PDFObject::HexString(vec![0x48, 0x65, 0x6C, 0x6C, 0x6F]),
        )
        .unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "<48656C6C6F>");
    }

    #[test]
    fn test_write_name() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::Name("Type".to_string())).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "/Type");
    }

    #[test]
    fn test_write_name_with_special_chars() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::Name("Font/Name".to_string())).unwrap();
        // The slash should be escaped as #2F
        assert_eq!(String::from_utf8(buffer).unwrap(), "/Font#2FName");
    }

    #[test]
    fn test_write_array() {
        let mut buffer = Vec::new();
        let arr = PDFObject::Array(
            vec![
                Box::new(PDFObject::Number(1.0)),
                Box::new(PDFObject::Number(2.0)),
                Box::new(PDFObject::Number(3.0)),
            ]
            .into(),
        );
        PDFWriter::write_object(&mut buffer, &arr).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "[1 2 3]");
    }

    #[test]
    fn test_write_dictionary() {
        let mut buffer = Vec::new();
        let mut dict = HashMap::new();
        dict.insert("Type".to_string(), PDFObject::Name("Page".to_string()));
        dict.insert("Rotate".to_string(), PDFObject::Number(90.0));

        PDFWriter::write_object(&mut buffer, &PDFObject::Dictionary(dict)).unwrap();
        let result = String::from_utf8(buffer).unwrap();
        assert!(result.contains("/Type"));
        assert!(result.contains("/Page"));
        assert!(result.contains("/Rotate"));
        assert!(result.contains("90"));
    }

    #[test]
    fn test_write_reference() {
        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &PDFObject::Ref(Ref::new(5, 0))).unwrap();
        assert_eq!(String::from_utf8(buffer).unwrap(), "5 0 R");
    }

    #[test]
    fn test_incremental_update_with_delta() {
        // Create a delta layer with one modified object
        let mut delta = DeltaLayer::new(100);

        let mut dict = HashMap::new();
        dict.insert("Type".to_string(), PDFObject::Name("Page".to_string()));
        dict.insert("Rotate".to_string(), PDFObject::Number(90.0));

        delta.modify_object(Ref::new(10, 0), PDFObject::Dictionary(dict));

        // Write incremental update (original has 100 objects)
        let update = PDFWriter::write_incremental_update(&delta, 5000, 100, 4500).unwrap();

        // Verify it contains expected parts
        let update_str = String::from_utf8_lossy(&update);
        assert!(update_str.contains("10 0 obj"));
        assert!(update_str.contains("/Page"));
        assert!(update_str.contains("90"));
        assert!(update_str.contains("endobj"));
        assert!(update_str.contains("xref"));
        assert!(update_str.contains("trailer"));
        assert!(update_str.contains("%%EOF"));
        // Verify /Size is in the trailer
        assert!(update_str.contains("/Size 100"));
    }
}
