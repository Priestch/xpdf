use super::base_stream::BaseStream;
use super::decode;
use super::error::{PDFError, PDFResult};
use super::lexer::Lexer;
use super::parser::{PDFObject, Parser};
use super::stream::Stream;
use std::collections::HashMap;
use std::rc::Rc;

/// Cross-reference table entry.
///
/// Each entry in the xref table describes where to find an indirect object
/// in the PDF file. Based on PDF.js's XRef entry structure.
#[derive(Debug, Clone)]
pub enum XRefEntry {
    /// Free entry - object number is available for reuse
    Free { next_free: u64, generation: u32 },

    /// Uncompressed entry - object is stored uncompressed at given offset
    Uncompressed { offset: u64, generation: u32 },

    /// Compressed entry - object is stored in an object stream
    Compressed {
        obj_stream_num: u32,
        index: u32,
    },
}

impl XRefEntry {
    /// Returns true if this entry is free.
    pub fn is_free(&self) -> bool {
        matches!(self, XRefEntry::Free { .. })
    }

    /// Returns the generation number for this entry.
    pub fn generation(&self) -> u32 {
        match self {
            XRefEntry::Free { generation, .. } => *generation,
            XRefEntry::Uncompressed { generation, .. } => *generation,
            XRefEntry::Compressed { .. } => 0,
        }
    }
}

/// Cross-reference table for a PDF document.
///
/// The xref table maps object numbers to their locations in the PDF file.
/// This allows indirect object references (like "5 0 R") to be resolved.
///
/// Based on PDF.js src/core/xref.js
pub struct XRef {
    /// The entries in the xref table, indexed by object number
    entries: Vec<Option<XRefEntry>>,

    /// Cache of parsed objects (object number -> PDFObject)
    /// Uses Rc to avoid expensive cloning of large objects
    cache: HashMap<u32, Rc<PDFObject>>,

    /// The trailer dictionary
    trailer: Option<PDFObject>,

    /// Stream to read PDF data from
    stream: Box<dyn BaseStream>,
}

impl XRef {
    /// Creates a new XRef table.
    pub fn new(stream: Box<dyn BaseStream>) -> Self {
        XRef {
            entries: Vec::new(),
            cache: HashMap::new(),
            trailer: None,
            stream,
        }
    }

    /// Sets the stream position for parsing.
    pub fn set_stream_pos(&mut self, pos: usize) -> PDFResult<()> {
        self.stream.set_pos(pos)
    }

    /// Parses the cross-reference table starting at the current stream position.
    ///
    /// This reads the xref table and trailer dictionary. The stream should be
    /// positioned at the start of "xref" keyword.
    ///
    /// Example xref table format:
    /// ```text
    /// xref
    /// 0 6
    /// 0000000000 65535 f
    /// 0000000015 00000 n
    /// 0000000079 00000 n
    /// 0000000173 00000 n
    /// 0000000301 00000 n
    /// 0000000380 00000 n
    /// trailer
    /// << /Size 6 /Root 1 0 R >>
    /// ```
    pub fn parse(&mut self) -> PDFResult<()> {
        let lexer = Lexer::new(self.stream.make_sub_stream(
            self.stream.pos(),
            self.stream.length() - self.stream.pos(),
        )?)?;
        let mut parser = Parser::new(lexer)?;

        // First token should be "xref" command
        let obj = parser.get_object()?;
        if !obj.is_command("xref") {
            return Err(PDFError::Generic(format!(
                "Expected 'xref' keyword, got {:?}",
                obj
            )));
        }

        // Read xref table subsections (this also consumes "trailer" keyword)
        self.read_xref_table(&mut parser)?;

        // read_xref_table consumed the "trailer" keyword, so read the dictionary directly
        let trailer = parser.get_object()?;
        if !matches!(trailer, PDFObject::Dictionary(_)) {
            return Err(PDFError::Generic(
                "Expected trailer dictionary".to_string(),
            ));
        }

        self.trailer = Some(trailer);

        Ok(())
    }

    /// Reads xref table subsections.
    ///
    /// Each subsection starts with two numbers: first object number and count.
    /// Then follows one entry per line with: offset generation_number type
    fn read_xref_table(&mut self, parser: &mut Parser) -> PDFResult<()> {
        loop {
            // Peek at the next object to see if it's "trailer"
            let first_obj = parser.get_object()?;

            // Check if we've reached the trailer
            if first_obj.is_command("trailer") {
                // We've hit the trailer, but we need to "put it back"
                // Since we can't really put it back, we'll just break
                // and let the caller know to expect trailer was already consumed
                // Actually, let's just return Ok since we read the trailer keyword
                return Ok(());
            }

            // Get first object number
            let first = match first_obj {
                PDFObject::Number(n) => n as u32,
                _ => {
                    return Err(PDFError::Generic(format!(
                        "Expected subsection start number or 'trailer', got {:?}",
                        first_obj
                    )))
                }
            };

            // Get count
            let count_obj = parser.get_object()?;
            let count = match count_obj {
                PDFObject::Number(n) => n as u32,
                _ => {
                    return Err(PDFError::Generic(format!(
                        "Expected subsection count, got {:?}",
                        count_obj
                    )))
                }
            };

            // Ensure we have enough space in the entries vector
            let needed_size = (first + count) as usize;
            if self.entries.len() < needed_size {
                self.entries.resize(needed_size, None);
            }

            // Read each entry in the subsection
            for i in 0..count {
                let entry = self.read_xref_entry(parser)?;
                let obj_num = (first + i) as usize;

                // Only set if not already set (first xref wins)
                if self.entries[obj_num].is_none() {
                    self.entries[obj_num] = Some(entry);
                }
            }
        }
    }

    /// Reads a single xref entry.
    ///
    /// Format: offset generation type
    /// Example: 0000000015 00000 n
    fn read_xref_entry(&mut self, parser: &mut Parser) -> PDFResult<XRefEntry> {
        // Read offset/next_free
        let offset_obj = parser.get_object()?;
        let offset = match offset_obj {
            PDFObject::Number(n) => n as u64,
            _ => {
                return Err(PDFError::Generic(format!(
                    "Expected offset in xref entry, got {:?}",
                    offset_obj
                )))
            }
        };

        // Read generation number
        let gen_obj = parser.get_object()?;
        let generation = match gen_obj {
            PDFObject::Number(n) => n as u32,
            _ => {
                return Err(PDFError::Generic(format!(
                    "Expected generation in xref entry, got {:?}",
                    gen_obj
                )))
            }
        };

        // Read type (f = free, n = in use)
        let type_obj = parser.get_object()?;
        let entry_type = match type_obj {
            PDFObject::Name(ref name) if name == "f" => "f",
            PDFObject::Name(ref name) if name == "n" => "n",
            _ => {
                return Err(PDFError::Generic(format!(
                    "Expected 'f' or 'n' in xref entry, got {:?}",
                    type_obj
                )))
            }
        };

        let entry = match entry_type {
            "f" => XRefEntry::Free {
                next_free: offset,
                generation,
            },
            "n" => XRefEntry::Uncompressed { offset, generation },
            _ => unreachable!(),
        };

        Ok(entry)
    }

    /// Gets an entry from the xref table.
    pub fn get_entry(&self, obj_num: u32) -> Option<&XRefEntry> {
        self.entries.get(obj_num as usize)?.as_ref()
    }

    /// Fetches an object from a compressed object stream (ObjStm).
    ///
    /// Object streams contain multiple PDF objects in a compressed format.
    /// The stream format is:
    /// ```text
    /// N1 offset1 N2 offset2 ... Nn offsetn [object1] [object2] ... [objectn]
    /// ```
    ///
    /// Based on PDF.js fetchCompressed method.
    ///
    /// # Arguments
    /// * `obj_stream_num` - The object number of the ObjStm
    /// * `index` - The index of the object within the stream (0-based)
    ///
    /// # Returns
    /// The requested object wrapped in Rc
    fn fetch_compressed(&mut self, obj_stream_num: u32, index: u32) -> PDFResult<Rc<PDFObject>> {
        // First, fetch the object stream itself (as an uncompressed object)
        let obj_stream_obj = self.fetch(obj_stream_num, 0)?;

        // The object stream must be a Stream object with dictionary and data
        match &*obj_stream_obj {
            PDFObject::Stream { dict, data } => {
                // Check if this is an ObjStm
                if let Some(PDFObject::Name(type_name)) = dict.get("Type") {
                    if type_name != "ObjStm" {
                        return Err(PDFError::Generic(format!(
                            "Expected ObjStm type, got /{}",
                            type_name
                        )));
                    }
                }

                // Get N (number of objects) and First (byte offset of first object)
                let n = dict
                    .get("N")
                    .and_then(|obj| match obj {
                        PDFObject::Number(n) => Some(*n as u32),
                        _ => None,
                    })
                    .ok_or_else(|| PDFError::Generic("ObjStm missing /N parameter".to_string()))?;

                let first = dict
                    .get("First")
                    .and_then(|obj| match obj {
                        PDFObject::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        PDFError::Generic("ObjStm missing /First parameter".to_string())
                    })?;

                if index >= n {
                    return Err(PDFError::Generic(format!(
                        "Index {} out of range for ObjStm with {} objects",
                        index, n
                    )));
                }

                // Decompress the stream data if needed
                let filter_name = dict.get("Filter").and_then(|f| match f {
                    PDFObject::Name(name) => Some(name.as_str()),
                    _ => None,
                });

                let decompressed_data = decode::decode_stream(data, filter_name)?;

                // Parse the object number/offset pairs (first N pairs of integers)
                let index_stream = Stream::from_bytes(decompressed_data[..first].to_vec());
                let lexer = Lexer::new(Box::new(index_stream) as Box<dyn BaseStream>)?;
                let mut parser = Parser::new(lexer)?;

                // Read all object numbers and offsets
                let mut obj_nums = Vec::with_capacity(n as usize);
                let mut offsets = Vec::with_capacity(n as usize);

                for _ in 0..n {
                    let num = parser.get_object()?;
                    let offset = parser.get_object()?;

                    let obj_num = match num {
                        PDFObject::Number(n) => n as u32,
                        _ => {
                            return Err(PDFError::Generic(format!(
                                "Expected object number, got {:?}",
                                num
                            )))
                        }
                    };

                    let obj_offset = match offset {
                        PDFObject::Number(n) => n as usize,
                        _ => {
                            return Err(PDFError::Generic(format!(
                                "Expected offset, got {:?}",
                                offset
                            )))
                        }
                    };

                    obj_nums.push(obj_num);
                    offsets.push(obj_offset);
                }

                // Now parse the object at the requested index
                let obj_offset = first + offsets[index as usize];
                let obj_length = if (index as usize) < offsets.len() - 1 {
                    offsets[index as usize + 1]
                } else {
                    decompressed_data.len() - obj_offset
                };

                // Create a stream for just this object's data
                let obj_data = decompressed_data[obj_offset..obj_offset + obj_length].to_vec();
                let obj_stream = Stream::from_bytes(obj_data);
                let obj_lexer = Lexer::new(Box::new(obj_stream) as Box<dyn BaseStream>)?;
                let mut obj_parser = Parser::new(obj_lexer)?;

                // Parse the object (no "obj"/"endobj" wrappers in ObjStm)
                let object = Rc::new(obj_parser.get_object()?);

                // Cache it with the actual object number
                let actual_obj_num = obj_nums[index as usize];
                self.cache.insert(actual_obj_num, Rc::clone(&object));

                Ok(object)
            }
            PDFObject::Dictionary(_) => {
                // If it's just a dictionary without stream data, we can't decompress it yet
                Err(PDFError::Generic(
                    "ObjStm is a dictionary but stream data parsing not yet implemented".to_string(),
                ))
            }
            _ => Err(PDFError::Generic(
                "ObjStm is not a stream or dictionary".to_string(),
            )),
        }
    }

    /// Fetches an indirect object by reference.
    ///
    /// This resolves an indirect reference like "5 0 R" to its actual object.
    /// The object is cached after being parsed. Returns an Rc to avoid expensive cloning.
    pub fn fetch(&mut self, obj_num: u32, generation: u32) -> PDFResult<Rc<PDFObject>> {
        // Check cache first - Rc::clone is cheap (just increments refcount)
        if let Some(cached) = self.cache.get(&obj_num) {
            return Ok(Rc::clone(cached));
        }

        // Get xref entry
        let entry = self
            .get_entry(obj_num)
            .ok_or_else(|| PDFError::Generic(format!("Object {} not found in xref", obj_num)))?;

        match entry {
            XRefEntry::Free { .. } => Err(PDFError::Generic(format!(
                "Cannot fetch free object {}",
                obj_num
            ))),

            XRefEntry::Uncompressed {
                offset,
                generation: entry_gen,
            } => {
                // Verify generation number matches
                if generation != *entry_gen {
                    return Err(PDFError::Generic(format!(
                        "Generation mismatch for object {}: expected {}, got {}",
                        obj_num, entry_gen, generation
                    )));
                }

                // Clone the offset to avoid borrow checker issues
                let offset_value = *offset;

                // Create a sub-stream starting at the object's position
                // No need to manipulate parent stream position - sub-stream is independent
                let sub_stream = self.stream.make_sub_stream(
                    offset_value as usize,
                    self.stream.length() - offset_value as usize,
                )?;

                // Parse the indirect object
                // Format: N G obj ... endobj
                let lexer = Lexer::new(sub_stream)?;
                let mut parser = Parser::new(lexer)?;

                // Read object number
                let num_obj = parser.get_object()?;
                let parsed_num = match num_obj {
                    PDFObject::Number(n) => n as u32,
                    _ => {
                        return Err(PDFError::Generic(format!(
                            "Expected object number, got {:?}",
                            num_obj
                        )))
                    }
                };

                if parsed_num != obj_num {
                    return Err(PDFError::Generic(format!(
                        "Object number mismatch: expected {}, got {}",
                        obj_num, parsed_num
                    )));
                }

                // Read generation number
                let gen_obj = parser.get_object()?;
                let parsed_gen = match gen_obj {
                    PDFObject::Number(n) => n as u32,
                    _ => {
                        return Err(PDFError::Generic(format!(
                            "Expected generation number, got {:?}",
                            gen_obj
                        )))
                    }
                };

                if parsed_gen != generation {
                    return Err(PDFError::Generic(format!(
                        "Generation number mismatch: expected {}, got {}",
                        generation, parsed_gen
                    )));
                }

                // Read "obj" keyword
                let obj_keyword = parser.get_object()?;
                if !obj_keyword.is_command("obj") {
                    return Err(PDFError::Generic(format!(
                        "Expected 'obj' keyword, got {:?}",
                        obj_keyword
                    )));
                }

                // Read the actual object
                let object = Rc::new(parser.get_object()?);

                // Cache the Rc - cheap clone
                self.cache.insert(obj_num, Rc::clone(&object));

                Ok(object)
            }

            XRefEntry::Compressed {
                obj_stream_num,
                index,
            } => {
                // Fetch from compressed object stream
                self.fetch_compressed(*obj_stream_num, *index)
            }
        }
    }

    /// Fetches an object if it's a reference, otherwise returns the object as-is.
    ///
    /// Returns an owned PDFObject (cloned from Rc if fetched from cache).
    /// Use `fetch()` directly if you want an Rc to avoid the clone.
    pub fn fetch_if_ref(&mut self, obj: &PDFObject) -> PDFResult<PDFObject> {
        match obj {
            PDFObject::Ref { num, generation } => {
                let rc_obj = self.fetch(*num, *generation)?;
                Ok((*rc_obj).clone())
            }
            _ => Ok(obj.clone()),
        }
    }

    /// Returns the trailer dictionary.
    pub fn trailer(&self) -> Option<&PDFObject> {
        self.trailer.as_ref()
    }

    /// Returns the catalog (root) dictionary.
    pub fn catalog(&mut self) -> PDFResult<PDFObject> {
        // Clone the root reference to avoid borrow checker issues
        let root_ref = {
            let trailer = self
                .trailer
                .as_ref()
                .ok_or_else(|| PDFError::Generic("No trailer dictionary".to_string()))?;

            let trailer_dict = match trailer {
                PDFObject::Dictionary(dict) => dict,
                _ => {
                    return Err(PDFError::Generic(
                        "Trailer is not a dictionary".to_string(),
                    ))
                }
            };

            trailer_dict
                .get("Root")
                .ok_or_else(|| PDFError::Generic("No Root entry in trailer".to_string()))?
                .clone()
        };

        // Fetch and dereference the Rc
        let rc_catalog = match &root_ref {
            PDFObject::Ref { num, generation } => self.fetch(*num, *generation)?,
            _ => return Ok(root_ref),
        };

        Ok((*rc_catalog).clone())
    }

    /// Returns the number of entries in the xref table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the xref table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Stream;

    #[test]
    fn test_xref_entry_is_free() {
        let free_entry = XRefEntry::Free {
            next_free: 0,
            generation: 65535,
        };
        assert!(free_entry.is_free());

        let uncompressed_entry = XRefEntry::Uncompressed {
            offset: 100,
            generation: 0,
        };
        assert!(!uncompressed_entry.is_free());
    }

    #[test]
    fn test_xref_entry_generation() {
        let entry = XRefEntry::Uncompressed {
            offset: 100,
            generation: 5,
        };
        assert_eq!(entry.generation(), 5);
    }

    #[test]
    fn test_parse_simple_xref() {
        let data = b"xref\n\
            0 1\n\
            0000000000 65535 f\n\
            trailer\n\
            << /Size 1 >>\n";

        let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);

        xref.parse().unwrap();

        // Check that entry 0 is free
        let entry = xref.get_entry(0).unwrap();
        assert!(entry.is_free());
        assert_eq!(entry.generation(), 65535);

        // Check trailer
        assert!(xref.trailer().is_some());
    }

    #[test]
    fn test_parse_xref_with_multiple_entries() {
        let data = b"xref\n\
            0 3\n\
            0000000000 65535 f\n\
            0000000015 00000 n\n\
            0000000079 00000 n\n\
            trailer\n\
            << /Size 3 >>\n";

        let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);

        xref.parse().unwrap();

        assert_eq!(xref.len(), 3);

        // Check entry 0 (free)
        let entry0 = xref.get_entry(0).unwrap();
        assert!(entry0.is_free());

        // Check entry 1 (uncompressed)
        let entry1 = xref.get_entry(1).unwrap();
        assert!(!entry1.is_free());
        if let XRefEntry::Uncompressed { offset, generation } = entry1 {
            assert_eq!(*offset, 15);
            assert_eq!(*generation, 0);
        } else {
            panic!("Expected uncompressed entry");
        }

        // Check entry 2 (uncompressed)
        let entry2 = xref.get_entry(2).unwrap();
        if let XRefEntry::Uncompressed { offset, generation } = entry2 {
            assert_eq!(*offset, 79);
            assert_eq!(*generation, 0);
        } else {
            panic!("Expected uncompressed entry");
        }
    }

    #[test]
    fn test_fetch_indirect_object() {
        // Create a minimal PDF with an indirect object
        let data = b"1 0 obj\n\
            42\n\
            endobj\n\
            xref\n\
            0 2\n\
            0000000000 65535 f\n\
            0000000000 00000 n\n\
            trailer\n\
            << /Size 2 >>\n";

        let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);

        // Parse xref starting from "xref" position
        // First, we need to position the stream at the xref
        let xref_pos = data
            .windows(4)
            .position(|w| w == b"xref")
            .expect("xref not found");
        xref.stream.set_pos(xref_pos).unwrap();
        xref.parse().unwrap();

        // Fetch object 1
        let obj = xref.fetch(1, 0).unwrap();
        assert_eq!(*obj, PDFObject::Number(42.0));
    }

    #[test]
    fn test_fetch_if_ref() {
        let data = b"1 0 obj\n\
            42\n\
            endobj\n\
            xref\n\
            0 2\n\
            0000000000 65535 f\n\
            0000000000 00000 n\n\
            trailer\n\
            << /Size 2 >>\n";

        let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);

        let xref_pos = data
            .windows(4)
            .position(|w| w == b"xref")
            .expect("xref not found");
        xref.stream.set_pos(xref_pos).unwrap();
        xref.parse().unwrap();

        // Test with a reference
        let ref_obj = PDFObject::Ref {
            num: 1,
            generation: 0,
        };
        let result = xref.fetch_if_ref(&ref_obj).unwrap();
        assert_eq!(result, PDFObject::Number(42.0));

        // Test with a direct object
        let direct_obj = PDFObject::Number(100.0);
        let result = xref.fetch_if_ref(&direct_obj).unwrap();
        assert_eq!(result, PDFObject::Number(100.0));
    }
}
