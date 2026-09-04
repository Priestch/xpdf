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

use super::delta::{DeltaLayer, ObjectId};
use super::error::{PDFError, PDFResult};
use super::parser::PDFObject;
use std::collections::BTreeMap;
use std::io::Write;

/// One cross-reference row to emit for a single object number in this update.
///
/// `InUse` marks an object written into this update (offset of its `N G obj`
/// header); `Free` marks an object deleted from the document (a member of the
/// free list chained from the object-0 head — spec §7.5.4).
enum XRefOutEntry {
    InUse { offset: u64, generation: u32 },
    Free { next_free: u32, generation: u32 },
}

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
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn write_incremental_update(
        delta: &DeltaLayer,
        original_size: usize,
        total_object_count: u32,
        prev_xref_offset: usize,
    ) -> PDFResult<Vec<u8>> {
        Self::write_incremental_update_with_trailer(
            delta,
            original_size,
            total_object_count,
            prev_xref_offset,
            None,
        )
    }

    /// Write an incremental update while inheriting document trailer entries.
    ///
    /// `delta` provides object changes (modified/new objects, plus deleted
    /// objects serialized as free xref entries — spec §7.5.6) and trailer
    /// overrides.
    pub fn write_incremental_update_with_trailer(
        delta: &DeltaLayer,
        original_size: usize,
        total_object_count: u32,
        prev_xref_offset: usize,
        original_trailer: Option<&PDFObject>,
    ) -> PDFResult<Vec<u8>> {
        let mut buffer = vec![b'\n'];

        // Cross-reference entries for this update, keyed by object number. A
        // BTreeMap keeps rows sorted (subsections must be contiguous ascending
        // runs) and makes each row lookup O(log n).
        let mut entries: BTreeMap<u32, XRefOutEntry> = BTreeMap::new();

        // Free-list head: object 0, "next free" pointing at the first freed
        // object (spec §7.5.4). Written unconditionally — classic producers
        // always emit the head row.
        let mut freed: Vec<ObjectId> = delta
            .iter_deleted()
            .map(|(num, generation)| (*num, *generation))
            .filter(|(num, _)| *num != 0) // object 0 IS the head, never freed
            .collect();
        freed.sort_unstable();
        entries.insert(
            0,
            XRefOutEntry::Free {
                next_free: freed.first().map_or(0, |(num, _)| *num),
                generation: 65535,
            },
        );
        // Chain the freed objects: each points at the next, the last back to
        // the head. Freeing bumps the generation so readers never resolve the
        // freed generation again (pdf.js writer.js:313-321).
        for (i, &(num, freed_gen)) in freed.iter().enumerate() {
            let next_free = freed.get(i + 1).map_or(0, |&(next, _)| next);
            entries.insert(
                num,
                XRefOutEntry::Free {
                    next_free,
                    generation: (freed_gen + 1).min(u16::MAX as u32),
                },
            );
        }

        let mut current_offset = original_size as u64 + 1;

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
            entries.insert(
                obj_id.0,
                XRefOutEntry::InUse {
                    offset: current_offset,
                    generation: obj_id.1,
                },
            );

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
        Self::write_xref_table(&mut buffer, &entries)?;

        // Write the new trailer
        Self::write_trailer(
            &mut buffer,
            xref_start_offset,
            total_object_count,
            prev_xref_offset,
            original_trailer,
            delta,
            &entries,
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
            PDFObject::InlineImageData(_) => {
                buffer.write_all(b"").map_err(|e| {
                    PDFError::Generic(format!("Failed to write inline image data: {}", e))
                })?;
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
                // /Length must equal the bytes actually written (spec §7.3.8.1).
                // Delta streams normally carry correct lengths already, but
                // normalize defensively so a stale value can never corrupt the
                // update — readers cut the stream at /Length and would feed a
                // decoder the wrong byte count.
                let mut dict = dict.clone();
                dict.insert("Length".to_string(), PDFObject::Number(data.len() as f64));
                Self::write_object(buffer, &PDFObject::Dictionary(dict))?;

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
    /// This writes a hybrid xref table covering the objects in this update —
    /// new/modified objects as in-use rows, deleted objects as free rows
    /// (spec §7.5.4/§7.5.6).
    ///
    /// Format:
    /// ```text
    /// xref
    /// start_index count
    /// offset generation n  (for in-use objects)
    /// next_free generation f  (for free objects; object 0 heads the list)
    /// ```
    fn write_xref_table<W: Write>(
        buffer: &mut W,
        entries: &BTreeMap<u32, XRefOutEntry>,
    ) -> PDFResult<()> {
        buffer
            .write_all(b"xref\n")
            .map_err(|e| PDFError::Generic(format!("Failed to write xref header: {}", e)))?;

        // Group consecutive object numbers into subsections; the spec requires
        // each subsection header to be "start_index count" followed by exactly
        // `count` 20-byte rows.
        let obj_nums: Vec<u32> = entries.keys().copied().collect();
        let mut index = 0;
        while index < obj_nums.len() {
            let start = index;
            while index + 1 < obj_nums.len() && obj_nums[index + 1] == obj_nums[index] + 1 {
                index += 1;
            }
            Self::write_xref_subsection(buffer, obj_nums[start], (index - start + 1) as u32)?;
            for obj_num in &obj_nums[start..=index] {
                match &entries[obj_num] {
                    XRefOutEntry::InUse { offset, generation } => {
                        write!(buffer, "{:010} {:05} n \n", offset, generation).map_err(|e| {
                            PDFError::Generic(format!("Failed to write xref entry: {}", e))
                        })?;
                    }
                    XRefOutEntry::Free {
                        next_free,
                        generation,
                    } => {
                        write!(buffer, "{:010} {:05} f \n", next_free, generation).map_err(
                            |e| PDFError::Generic(format!("Failed to write xref entry: {}", e)),
                        )?;
                    }
                }
            }
            index += 1;
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
        original_trailer: Option<&PDFObject>,
        delta: &DeltaLayer,
        entries: &BTreeMap<u32, XRefOutEntry>,
    ) -> PDFResult<()> {
        buffer
            .write_all(b"trailer\n")
            .map_err(|e| PDFError::Generic(format!("Failed to write trailer header: {}", e)))?;
        buffer.write_all(b"<<").map_err(|e| {
            PDFError::Generic(format!("Failed to write trailer dict prefix: {}", e))
        })?;

        // /Size must cover every object number that has ever existed (object
        // numbers are never reused — spec §7.5.4), so take the max of the
        // caller's count, the original trailer's /Size, and the highest entry
        // in this table.
        let mut size = total_object_count;
        if let Some(PDFObject::Dictionary(dict)) = original_trailer {
            if let Some(PDFObject::Number(n)) = dict.get("Size") {
                size = size.max(*n as u32);
            }
        }
        if let Some(&max_num) = entries.keys().next_back() {
            size = size.max(max_num + 1);
        }
        write!(buffer, "/Size {}", size)
            .map_err(|e| PDFError::Generic(format!("Failed to write /Size: {}", e)))?;

        // Previous: offset of previous xref table
        write!(buffer, " /Prev {}", prev_xref_offset)
            .map_err(|e| PDFError::Generic(format!("Failed to write /Prev: {}", e)))?;

        if let Some(PDFObject::Dictionary(dict)) = original_trailer {
            for key in ["Root", "Info", "ID", "Encrypt"] {
                // Trailer overrides from delta take precedence.
                if let Some(override_val) = delta.get_trailer_override(key) {
                    write!(buffer, " /{} ", key).map_err(|e| {
                        PDFError::Generic(format!("Failed to write trailer /{}: {}", key, e))
                    })?;
                    Self::write_object(buffer, override_val)?;
                } else if let Some(value) = dict.get(key) {
                    write!(buffer, " /{} ", key).map_err(|e| {
                        PDFError::Generic(format!("Failed to write trailer /{}: {}", key, e))
                    })?;
                    Self::write_object(buffer, value)?;
                }
            }
        } else if !delta.iter_trailer_overrides().next().is_none() {
            // No original trailer but we have overrides — write them.
            for (key, value) in delta.iter_trailer_overrides() {
                write!(buffer, " /{} ", key).map_err(|e| {
                    PDFError::Generic(format!("Failed to write trailer /{}: {}", key, e))
                })?;
                Self::write_object(buffer, value)?;
            }
        }

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
        // Parsed from the expected output text: same f64 value as a bare
        // literal, without tripping clippy::approx_const (3.14 resembles PI).
        let value: f64 = "3.14".parse().unwrap();
        PDFWriter::write_object(&mut buffer, &PDFObject::Number(value)).unwrap();
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
        // Free-list head is written even with no deletions (spec §7.5.4).
        assert!(update_str.contains("0 1\n0000000000 65535 f \n"));
    }

    #[test]
    fn object_zero_head_entry_written_with_no_deletions() {
        let mut delta = DeltaLayer::new(100);
        delta.modify_object(Ref::new(10, 0), PDFObject::String(b"page".to_vec()));

        let update = PDFWriter::write_incremental_update(&delta, 5000, 100, 4500).unwrap();
        let update_str = String::from_utf8_lossy(&update);

        // Head subsection alone, then the in-use subsection for object 10.
        let head = "0 1\n0000000000 65535 f \n";
        let in_use_start = update_str
            .find(head)
            .expect("free-list head must be written");
        assert!(
            update_str[in_use_start + head.len()..].starts_with("10 1\n"),
            "object 10 must start its own subsection after the head, got: {update_str}"
        );
    }

    #[test]
    fn deleted_objects_written_as_free_chain() {
        // Two freed objects form a chain: head(0) → 3 → 7 → back to head.
        let mut delta = DeltaLayer::new(100);
        delta.delete_object(Ref::new(7, 0));
        delta.delete_object(Ref::new(3, 0));

        let update = PDFWriter::write_incremental_update(&delta, 5000, 100, 4500).unwrap();
        let update_str = String::from_utf8_lossy(&update);

        // Byte-exact subsections: head points at 3; 3 points at 7; 7 closes
        // the chain. Freed generations bump to 1 (pdf.js writer.js:313-321).
        assert!(update_str.contains("0 1\n0000000003 65535 f \n"));
        assert!(update_str.contains("3 1\n0000000007 00001 f \n"));
        assert!(update_str.contains("7 1\n0000000000 00001 f \n"));
    }

    #[test]
    fn free_and_in_use_noncontiguous_subsections() {
        // Deleting object 1 merges into the head's subsection (0..=1); the
        // modified objects 7,8 form their own run.
        let mut delta = DeltaLayer::new(100);
        delta.delete_object(Ref::new(1, 0));
        delta.modify_object(Ref::new(7, 0), PDFObject::String(b"a".to_vec()));
        delta.modify_object(Ref::new(8, 0), PDFObject::String(b"b".to_vec()));

        let update = PDFWriter::write_incremental_update(&delta, 5000, 100, 4500).unwrap();
        let update_str = String::from_utf8_lossy(&update);

        // Head + freed object 1 in one subsection; 7,8 as in-use in another.
        assert!(update_str.contains("0 2\n0000000001 65535 f \n0000000000 00001 f \n"));
        assert!(update_str.contains("7 2\n"));
        // The in-use rows must be `n` rows.
        let seven = update_str.find("7 2\n").expect("subsection 7 2 must exist");
        let mut lines = update_str[seven + 4..].lines();
        let row7 = lines.next().expect("row for object 7");
        let row8 = lines.next().expect("row for object 8");
        assert!(row7.ends_with(" n "), "row 7 must be in-use: {row7}");
        assert!(row8.ends_with(" n "), "row 8 must be in-use: {row8}");
    }

    #[test]
    fn stream_length_normalized_on_write() {
        let mut dict = HashMap::new();
        dict.insert("Length".to_string(), PDFObject::Number(999.0));
        let stream = PDFObject::Stream {
            dict,
            data: b"abc".to_vec(),
        };

        let mut buffer = Vec::new();
        PDFWriter::write_object(&mut buffer, &stream).unwrap();
        let out = String::from_utf8(buffer).unwrap();

        assert!(
            out.contains("/Length 3"),
            "stale /Length must be corrected: {out}"
        );
        assert!(
            !out.contains("999"),
            "stale /Length must not survive: {out}"
        );
    }

    #[test]
    fn size_is_max_of_caller_trailer_and_entries() {
        // Original trailer claims /Size 250, caller passes 100, highest entry
        // is 10 → /Size must be 250.
        let mut delta = DeltaLayer::new(100);
        delta.modify_object(Ref::new(10, 0), PDFObject::Null);

        let mut trailer = HashMap::new();
        trailer.insert("Size".to_string(), PDFObject::Number(250.0));
        let update = PDFWriter::write_incremental_update_with_trailer(
            &delta,
            5000,
            100,
            4500,
            Some(&PDFObject::Dictionary(trailer)),
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&update).contains("/Size 250"));

        // Caller's count wins when it is the largest.
        let update = PDFWriter::write_incremental_update(&delta, 5000, 300, 4500).unwrap();
        assert!(String::from_utf8_lossy(&update).contains("/Size 300"));
    }
}
