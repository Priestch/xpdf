use super::base_stream::BaseStream;
use super::byte_source::Loadable;
use super::decode;
use super::error::{PDFError, PDFResult};
use super::lexer::Lexer;
use super::parser::{PDFObject, Parser, Ref};
use super::stream::Stream;
use lru::LruCache;
use std::cell::RefCell;
use std::collections::HashMap; // Still needed for String keys in dictionaries
use std::num::NonZeroUsize;
use std::sync::Arc;

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
    Compressed { obj_stream_num: u32, index: u32 },
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

    /// Cache of parsed objects (full indirect reference -> PDFObject)
    /// Uses Arc to avoid expensive cloning of large objects
    /// Uses LRU cache with FxHashMap for bounded memory and fast access
    /// Default capacity: 1000 objects (~10MB for typical PDFs)
    /// Wrapped in `RefCell` so the cache can be checked with `&self` (no `&mut self`
    /// needed for a probe). Cache mutations through `RefCell` are never re-entrant, so
    /// runtime panics from overlapping borrows do not occur.
    cache:
        RefCell<LruCache<Ref, Arc<PDFObject>, std::hash::BuildHasherDefault<rustc_hash::FxHasher>>>,

    /// The trailer dictionary
    trailer: Option<PDFObject>,

    /// Stream to read PDF data from
    stream: Box<dyn BaseStream>,

    /// Whether the xref table has already been rebuilt by the full-file recovery scan.
    /// Guards lazy fetch-time recovery (hayro's `repair()` model): the first `fetch` that
    /// fails on a bad/stale offset triggers `recover_by_scan` once, then retries.
    recovered: bool,

    /// Keyed decryptor for an encrypted PDF (`Some` only after the password was verified
    /// and the file key derived). `None` = unencrypted, or encrypted-but-locked. Set via
    /// [`XRef::set_decryptor`]; decrypts streams/strings inline in [`XRef::fetch_inner`].
    decryptor: Option<super::encryption::EncryptDict>,

    /// Object number of the `/Encrypt` dictionary, if any — it is exempt from decryption
    /// (so is the cross-reference stream, detected by `/Type /XRef` on the object itself).
    encrypt_obj_num: Option<u32>,

    /// Object refs currently being fetched (in progress, below this frame on the stack).
    /// Guards against circular references in corrupt PDFs: a ref already in this set is
    /// returned as the null object instead of recursing to a stack overflow. Mirrors
    /// pdf.js's `_pendingRefs` / hayro's `obj_stream_id == id` check.
    pending_refs: std::collections::HashSet<(u32, u32)>,
}

/// `XRef` participates in the pull-based [`drive`](crate::core::byte_source::drive) loop:
/// its backing stream is the loadable data, and operations like `parse`/`fetch` may emit
/// `DataMissing` (adapted to [`Need`](crate::core::byte_source::Need) at the drive
/// boundary via [`StepErr::adapt`](crate::core::byte_source::StepErr::adapt)).
///
/// Only `ensure_range` + `len` are needed (drive never probes bytes through `get`), so
/// `XRef` implements [`Loadable`], not the full [`ByteSource`](crate::core::byte_source::ByteSource).
impl Loadable for XRef {
    fn ensure_range(&mut self, pos: u64, len: usize) -> PDFResult<()> {
        let pos = usize::try_from(pos).map_err(|_| {
            PDFError::Generic(format!("ensure_range position {pos} overflows usize"))
        })?;
        self.stream.ensure_range(pos, len)
    }

    /// Stream length in bytes.
    ///
    /// Note: `XRef` also has an inherent [`XRef::len`](struct.XRef.html#method.len) that
    /// returns the **entry count**. That inherent method shadows this trait method on
    /// direct `xref.len()` calls, so `is_empty` is overridden below to reference the
    /// trait explicitly. `drive` does not call `len`, so the dual naming is harmless for
    /// the protocol.
    fn len(&self) -> u64 {
        self.stream.length() as u64
    }

    fn is_empty(&self) -> bool {
        Loadable::len(self) == 0
    }
}

impl XRef {
    /// Creates a new XRef table.
    pub fn new(stream: Box<dyn BaseStream>) -> Self {
        // Default cache capacity: 1000 objects
        // This is enough for most PDFs while keeping memory bounded
        let capacity = NonZeroUsize::new(1000).unwrap();
        let cache = LruCache::with_hasher(
            capacity,
            std::hash::BuildHasherDefault::<rustc_hash::FxHasher>::default(),
        );

        XRef {
            entries: Vec::new(),
            cache: RefCell::new(cache),
            trailer: None,
            stream,
            recovered: false,
            decryptor: None,
            encrypt_obj_num: None,
            pending_refs: std::collections::HashSet::new(),
        }
    }

    /// Sets the stream position for parsing.
    pub fn set_stream_pos(&mut self, pos: usize) -> PDFResult<()> {
        self.stream.set_pos(pos)
    }

    /// Get mutable access to the underlying stream for progressive loading.
    ///
    /// This is used with the retry_on_data_missing! macro to implement
    /// exception-driven progressive loading where missing data is loaded
    /// on-demand and operations are retried.
    pub(crate) fn stream_mut(&mut self) -> &mut Box<dyn BaseStream> {
        &mut self.stream
    }

    /// Parses the cross-reference table starting at the current stream position.
    ///
    /// This reads either a traditional xref table or an XRef stream (PDF 1.5+).
    /// It also follows /Prev entries to handle incremental updates.
    ///
    /// Traditional xref table format:
    /// ```text
    /// xref
    /// 0 6
    /// 0000000000 65535 f
    /// 0000000015 00000 n
    /// ...
    /// trailer
    /// << /Size 6 /Root 1 0 R >>
    /// ```
    ///
    /// XRef stream format (PDF 1.5+):
    /// ```text
    /// N 0 obj
    /// << /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R >>
    /// stream
    /// ...binary data...
    /// endstream
    /// endobj
    /// ```
    pub fn parse(&mut self) -> PDFResult<()> {
        let start_pos = self.stream.pos();

        // Queue of xref positions to process (handles /Prev chain)
        let mut xref_queue = vec![start_pos];

        // Cache to prevent infinite loops from circular /Prev references
        let mut parsed_positions = std::collections::HashSet::new();

        // The first trailer we encounter (from the end of the file) is the main trailer
        let mut main_trailer: Option<PDFObject> = None;

        while let Some(pos) = xref_queue.pop() {
            // Skip if we've already parsed this position (circular reference protection)
            if !parsed_positions.insert(pos) {
                continue;
            }

            // Parse one xref section at `pos`. The root section (== start_pos) is mandatory;
            // a /Prev-chained section that fails is skipped so a malformed older xref doesn't
            // sink the whole document (the root section already covers current objects).
            let trailer = match self.parse_one_xref_section(pos) {
                Ok(t) => t,
                Err(e) => {
                    if pos == start_pos {
                        return Err(e);
                    }
                    eprintln!(
                        "Warning: skipping malformed xref section at offset {pos} (via /Prev): {e}"
                    );
                    continue;
                }
            };

            // Save the first trailer as the main trailer
            if main_trailer.is_none() {
                main_trailer = Some(trailer.clone());
            }

            // Check for /Prev entry and add to queue
            if let PDFObject::Dictionary(ref dict) = trailer {
                if let Some(prev_obj) = dict.get("Prev") {
                    match prev_obj {
                        PDFObject::Number(n) => {
                            let prev_pos = *n as usize;
                            xref_queue.push(prev_pos);
                        }
                        PDFObject::Ref(ref_obj) => {
                            // Non-compliant PDFs might use a reference for /Prev
                            // The spec says it should be a direct number
                            // We'll try to handle it anyway by using the object number as position
                            // This is a heuristic and may not always work
                            xref_queue.push(ref_obj.num as usize);
                        }
                        _ => {
                            // Invalid /Prev entry, ignore it
                        }
                    }
                }

                // Hybrid-reference files: a classic `xref` table's trailer may carry
                // `/XRefStm <offset>` pointing at a cross-reference STREAM whose entries
                // (including compressed/object-stream ones the classic table can't express)
                // must also be pulled in. Per the spec the stream entries take precedence, so
                // we queue it to be parsed too; parse_one_xref_section's set-if-None means the
                // first writer wins, so we push /XRefStm FIRST (it's popped last → applied
                // before this section's classic entries only if it was queued earlier — but
                // since classic entries are already set by the call above, we instead let the
                // stream fill only the gaps the classic table doesn't cover, which is exactly
                // the compressed-object case. This is the practical hybrid behavior.)
                if let Some(PDFObject::Number(xrefstm_pos)) = dict.get("XRefStm") {
                    let pos = *xrefstm_pos as usize;
                    if parsed_positions.insert(pos) {
                        // Best-effort: parse the stream section, tolerate failure.
                        if let Err(e) = self.parse_one_xref_section(pos) {
                            eprintln!(
                                "Warning: skipping malformed /XRefStm section at offset {pos}: {e}"
                            );
                        }
                    }
                }
            }
        }

        // Set the main trailer
        self.trailer = main_trailer;

        Ok(())
    }

    /// Rebuild the xref table by scanning the whole file for `N G obj` patterns.
    ///
    /// Last-resort recovery for corrupt/truncated PDFs where the normal xref read fails
    /// or `startxref` is missing. Ports pdf.js's `XRef.indexObjects()`
    /// (`pdf.js/src/core/xref.js`): it reads the entire file, records every indirect
    /// object's offset, pulls compressed entries from any `/Type /XRef` streams it
    /// encounters, and selects a trailer whose `/Root → /Pages → /Count` validates.
    ///
    /// **Progressive-loading exception:** this reads the whole file
    /// (`stream.get_byte_range(0, length)`) — the documented last-resort recovery path,
    /// exactly as pdf.js does (`stream.getBytes()`). The normal path never calls this.
    pub(crate) fn recover_by_scan(&mut self) -> PDFResult<()> {
        let length = self.stream.length();
        let buf = self.stream.get_byte_range(0, length)?;
        let buf = buf.as_slice();
        let length = buf.len();

        // Reset any partial entries/cache left by the failed normal parse.
        self.entries.clear();
        self.cache.borrow_mut().clear();

        const TAB: u8 = 0x9;
        const LF: u8 = 0xa;
        const CR: u8 = 0xd;
        const SPACE: u8 = 0x20;
        const PERCENT: u8 = 0x25;

        let mut trailers: Vec<usize> = Vec::new();
        let mut xref_stms: Vec<usize> = Vec::new();
        let mut obj_stms: Vec<u32> = Vec::new();

        let mut position = 0usize;
        while position < length {
            let ch = buf[position];
            if ch == TAB || ch == LF || ch == CR || ch == SPACE {
                position += 1;
                continue;
            }
            if ch == PERCENT {
                // %-comment: skip to end of line.
                position += 1;
                while position < length && buf[position] != LF && buf[position] != CR {
                    position += 1;
                }
                continue;
            }

            // readToken: bytes from `position` until LF/CR/LT('<').
            let token_len = read_token_len(buf, position);
            let token = &buf[position..position + token_len];

            if starts_with_keyword(token, b"xref") {
                // skip to the "trailer" keyword, record it, then skip to "startxref".
                position = match skip_until(buf, position, b"trailer") {
                    Some(p) => p,
                    None => break,
                };
                trailers.push(position);
                position = skip_until(buf, position, b"startxref").unwrap_or(position);
                continue;
            }

            if let Some((num, generation)) = parse_obj_header(token) {
                let body_start = position + token_len;

                // (a) Validate before recording: only accept the object if a plausible
                // object-start byte follows the `N G obj` header. This filters the
                // false-positive headers that binary stream content produces (hayro does the
                // same by attempting a read), and — crucially — keeps us from later parsing
                // at bogus offsets where the lexer can loop on garbage.
                if !plausible_object_start(buf, body_start) {
                    position = find_endobj(buf, body_start)
                        .map(|e| e.checked_add(b"endobj".len()).unwrap_or(length))
                        .unwrap_or(body_start + token_len + 1);
                    continue;
                }

                // Record the object offset (first-writer-wins, like pdf.js).
                set_entry_if_none(
                    &mut self.entries,
                    num,
                    XRefEntry::Uncompressed {
                        offset: position as u64,
                        generation,
                    },
                );

                let body_end = find_endobj(buf, body_start).unwrap_or(length);
                let body = &buf[body_start..body_end];

                // Detect an xref-stream object: its body contains `/XRef` followed by a
                // non-letter.
                if let Some(off) = skip_until(body, 0, b"/XRef") {
                    let after = off + b"/XRef".len();
                    if after >= body.len() || body[after] < 0x40 {
                        xref_stms.push(position);
                    }
                }

                // (b) Detect an Object Stream (`/Type /ObjStm`): we'll decode it after the
                // scan and index every object it contains as a Compressed entry, so objects
                // stored only in an ObjStm become resolvable (hayro does this inline).
                if has_type_objstm(body) {
                    obj_stms.push(num);
                }

                // Advance past this object to its `endobj` so the body isn't re-scanned.
                position = body_end.checked_add(b"endobj".len()).unwrap_or(length);
                continue;
            }

            if starts_with_keyword(token, b"trailer") {
                trailers.push(position);
                position += token_len;
                continue;
            }

            // Unrecognized token: skip past it.
            position += token_len + 1;
        }

        // Pull compressed (type-2) entries from any xref-stream objects we found, reusing
        // the existing section parser (it handles `/Type /XRef` streams + /Prev). Failures
        // are tolerated — partial recovery beats none.
        for &off in &xref_stms {
            let _ = self.parse_one_xref_section(off);
        }

        // (b) Index every object inside each ObjStm we found, as Compressed entries — so
        // objects that live only in an object stream become resolvable. Best-effort.
        for &objstm_num in &obj_stms {
            self.index_object_stream(objstm_num);
        }

        // Select a trailer from the candidates (or fall back to a dict carrying /Root).
        self.trailer = self.select_recovery_trailer(&trailers, buf)?;
        self.recovered = true;
        Ok(())
    }

    /// Read the `trailer` keyword + dictionary at `offset`. Returns the dict, or `None`
    /// if it doesn't parse as `trailer << ... >>`.
    fn parse_trailer_dict_at(&mut self, offset: usize) -> Option<PDFObject> {
        let length = self.stream.length();
        let sub = self
            .stream
            .make_sub_stream(offset, length.saturating_sub(offset))
            .ok()?;
        let lexer = Lexer::new(sub).ok()?;
        let mut parser = Parser::new(lexer).ok()?;
        let kw = parser.get_object().ok()?;
        if !kw.is_command("trailer") {
            return None;
        }
        let dict = parser.get_object().ok()?;
        match dict {
            PDFObject::Dictionary(_) => Some(dict),
            // Some generators nest the dict (pdf.js handles `dict.dict`).
            _ => None,
        }
    }

    /// Validate that `dict`'s `/Root` resolves to a catalog with `/Pages` whose `/Count`
    /// is an integer — the same check pdf.js uses to pick a real trailer.
    fn root_validates(&mut self, dict: &HashMap<String, PDFObject>) -> bool {
        let root_ref = match dict.get("Root") {
            Some(r) => r.clone(),
            None => return false,
        };
        let catalog = match &root_ref {
            PDFObject::Ref(r) => match self.fetch(r.num, r.generation) {
                Ok(c) => (*c).clone(),
                Err(_) => return false,
            },
            other => other.clone(),
        };
        let catalog_dict = match catalog {
            PDFObject::Dictionary(d) => d,
            _ => return false,
        };
        let pages_ref = match catalog_dict.get("Pages") {
            Some(p) => p.clone(),
            None => return false,
        };
        let pages = match &pages_ref {
            PDFObject::Ref(r) => match self.fetch(r.num, r.generation) {
                Ok(p) => (*p).clone(),
                Err(_) => return false,
            },
            other => other.clone(),
        };
        matches!(
            pages,
            PDFObject::Dictionary(ref pd) if matches!(pd.get("Count"), Some(PDFObject::Number(_)))
        )
    }

    /// Pick the recovery trailer: prefer a validated candidate with `/ID`, else the last
    /// Decode an Object Stream (`/Type /ObjStm`) and record every object it contains as a
    /// `Compressed` entry, so objects that live only inside an ObjStm become resolvable.
    /// Used by `recover_by_scan` (mirrors hayro's `fallback_xref_map_inner` ObjStm handling).
    /// Best-effort: any decode/parse failure just aborts indexing that stream.
    fn index_object_stream(&mut self, obj_stream_num: u32) {
        let obj = match self.fetch(obj_stream_num, 0) {
            Ok(o) => o,
            Err(_) => return,
        };
        let (dict, data) = match &*obj {
            PDFObject::Stream { dict, data } => (dict.clone(), data.clone()),
            _ => return,
        };
        if !matches!(dict.get("Type"), Some(PDFObject::Name(n)) if n == "ObjStm") {
            return;
        }
        let n = match dict.get("N") {
            Some(PDFObject::Number(n)) => *n as u32,
            _ => return,
        };
        let first = match dict.get("First") {
            Some(PDFObject::Number(n)) => *n as usize,
            _ => return,
        };
        // apply_filters handles Name and Array filters (e.g. [/FlateDecode]).
        // decode_stream only accepts a bare FlateDecode name and returns raw
        // bytes for arrays — ObjStm then parses zlib as objects ("xÚ…").
        let mut decoded = match dict.get("Filter") {
            Some(filter) => match decode::apply_filters(&data, filter) {
                Ok(d) => d,
                Err(_) => return,
            },
            None => data.clone(),
        };
        // PNG predictor (mirror fetch_compressed).
        if let Some(PDFObject::Dictionary(parms)) = dict.get("DecodeParms") {
            if let Some(PDFObject::Number(p)) = parms.get("Predictor") {
                let pred = *p as i32;
                if (10..=14).contains(&pred) {
                    let columns = num_in(parms, "Columns").unwrap_or(1);
                    let colors = num_in(parms, "Colors").unwrap_or(1);
                    let bpc = num_in(parms, "BitsPerComponent").unwrap_or(8);
                    match decode::decode_png_predictor(&decoded, colors, bpc, columns) {
                        Ok(d) => decoded = d,
                        Err(_) => return,
                    }
                }
            }
        }

        // Parse the `N` (obj_num, offset) header pairs that precede the objects.
        if first > decoded.len() {
            return;
        }
        let header_stream = Stream::from_bytes(decoded[..first].to_vec());
        let lexer = match Lexer::new(Box::new(header_stream) as Box<dyn BaseStream>) {
            Ok(l) => l,
            Err(_) => return,
        };
        let mut parser = match Parser::new(lexer) {
            Ok(p) => p,
            Err(_) => return,
        };
        for i in 0..n {
            let num_obj = parser.get_object();
            let off_obj = parser.get_object();
            match (num_obj, off_obj) {
                (Ok(PDFObject::Number(obj_num)), Ok(PDFObject::Number(_))) => {
                    set_entry_if_none(
                        &mut self.entries,
                        obj_num as u32,
                        XRefEntry::Compressed {
                            obj_stream_num,
                            index: i,
                        },
                    );
                }
                _ => return,
            }
        }
    }

    /// Pick the recovery trailer: prefer a validated candidate with `/ID`, else the last
    /// validated candidate, else (pdf.js final fallback) the first dict carrying `/Root`.
    /// `buf` is the whole-file buffer (already read by `recover_by_scan`) reused for the
    /// byte-scan fallback.
    fn select_recovery_trailer(
        &mut self,
        candidates: &[usize],
        buf: &[u8],
    ) -> PDFResult<Option<PDFObject>> {
        let mut with_id: Option<PDFObject> = None;
        let mut validated_last: Option<PDFObject> = None;

        for &off in candidates {
            let dict_obj = match self.parse_trailer_dict_at(off) {
                Some(d) => d,
                None => continue,
            };
            let dict = match &dict_obj {
                PDFObject::Dictionary(d) => d.clone(),
                _ => continue,
            };
            let has_id = dict.get("ID").is_some();
            if self.root_validates(&dict) {
                if has_id && with_id.is_none() {
                    with_id = Some(dict_obj.clone());
                }
                validated_last = Some(dict_obj);
            }
        }

        if let Some(t) = with_id.or(validated_last) {
            return Ok(Some(t));
        }

        // Final fallback: no `trailer` candidate validated — find the first recorded object
        // whose body carries a `/Root <num> <generation> R` reference and synthesize a trailer.
        //
        // This is a BYTE-SCAN, deliberately not a parser: recovery can record false-positive
        // objects from binary content, and parsing at a bogus offset can send the lexer into
        // a non-advancing loop on garbage (a separate parser-robustness issue). A bounded
        // byte scan cannot loop. (Mirrors pdf.js's "first dict with /Root" fallback intent.)
        let nums: Vec<u32> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|_| i as u32))
            .collect();
        for num in nums {
            let Some(XRefEntry::Uncompressed { offset, .. }) = self.get_entry(num) else {
                continue;
            };
            if let Some((root_num, root_gen)) = extract_root_ref(buf, *offset as usize) {
                let mut dict = HashMap::new();
                dict.insert(
                    "Root".to_string(),
                    PDFObject::Ref(super::parser::Ref::new(root_num, root_gen)),
                );
                return Ok(Some(PDFObject::Dictionary(dict)));
            }
        }

        Err(PDFError::Generic(
            "recovery scan found no usable trailer/Root".to_string(),
        ))
    }

    /// Parse a single xref section (traditional table or XRef stream) at `pos`.
    ///
    /// Returns the section's trailer dictionary on success. Extracted from `parse()` so the
    /// caller can tolerate failures on `/Prev`-chained sections.
    fn parse_one_xref_section(&mut self, pos: usize) -> PDFResult<PDFObject> {
        self.stream.set_pos(pos)?;

        let lexer = Lexer::new(
            self.stream
                .make_sub_stream(pos, self.stream.length() - pos)?,
        )?;
        let mut parser = Parser::new(lexer)?;

        let obj = parser.get_object()?;

        let trailer = match obj {
            obj if obj.is_command("xref") => {
                self.read_xref_table(&mut parser)?;
                let trailer = parser.get_object()?;
                if !matches!(trailer, PDFObject::Dictionary(_)) {
                    return Err(PDFError::Generic("Expected trailer dictionary".to_string()));
                }
                trailer
            }
            PDFObject::Number(_obj_num) => {
                let generation = parser.get_object()?;
                let obj_keyword = parser.get_object()?;
                if !matches!(generation, PDFObject::Number(0.0)) {
                    return Err(PDFError::Generic(
                        "XRef stream must have generation 0".to_string(),
                    ));
                }
                if !obj_keyword.is_command("obj") {
                    return Err(PDFError::Generic(format!(
                        "Expected 'obj' keyword, got {:?}",
                        obj_keyword
                    )));
                }
                let xref_obj = parser.get_object()?;
                match xref_obj {
                    PDFObject::Stream { dict, data } => {
                        if let Some(PDFObject::Name(type_name)) = dict.get("Type") {
                            if type_name != "XRef" {
                                return Err(PDFError::Generic(format!(
                                    "Expected /Type /XRef, got /Type /{}",
                                    type_name
                                )));
                            }
                        } else {
                            return Err(PDFError::Generic(
                                "XRef stream missing /Type entry".to_string(),
                            ));
                        }
                        self.parse_xref_stream(&dict, &data)?;
                        PDFObject::Dictionary(dict)
                    }
                    _ => {
                        return Err(PDFError::Generic("Expected XRef stream object".to_string()));
                    }
                }
            }
            _ => {
                return Err(PDFError::Generic(format!(
                    "Expected 'xref' keyword or object number, got {:?}",
                    obj
                )));
            }
        };

        Ok(trailer)
    }
    ///
    /// XRef streams encode the cross-reference table as binary data in a stream.
    /// Dictionary keys:
    /// - /W [w1 w2 w3]: byte widths for type, offset/obj_stream_num, generation/index
    /// - /Index [first1 n1 first2 n2 ...]: ranges of object numbers (default: [0, Size])
    /// - /Size: total number of entries
    ///
    /// Entry types:
    /// - Type 0: Free entry (offset = next free obj, generation = generation)
    /// - Type 1: Uncompressed entry (offset = byte offset, generation = generation)
    /// - Type 2: Compressed entry (offset = obj stream num, generation = index in stream)
    ///
    /// Based on PDF.js processXRefStream()
    fn parse_xref_stream(
        &mut self,
        dict: &HashMap<String, PDFObject>,
        data: &[u8],
    ) -> PDFResult<()> {
        // Get W array (byte widths)
        let w_array = dict
            .get("W")
            .ok_or_else(|| PDFError::Generic("XRef stream missing /W entry".to_string()))?;

        let widths = match w_array {
            PDFObject::Array(arr) => {
                if arr.len() != 3 {
                    return Err(PDFError::Generic(format!(
                        "XRef stream /W must have 3 elements, got {}",
                        arr.len()
                    )));
                }
                let w1 = match &*arr[0] {
                    PDFObject::Number(n) => *n as usize,
                    _ => return Err(PDFError::Generic("/W[0] must be a number".to_string())),
                };
                let w2 = match &*arr[1] {
                    PDFObject::Number(n) => *n as usize,
                    _ => return Err(PDFError::Generic("/W[1] must be a number".to_string())),
                };
                let w3 = match &*arr[2] {
                    PDFObject::Number(n) => *n as usize,
                    _ => return Err(PDFError::Generic("/W[2] must be a number".to_string())),
                };
                (w1, w2, w3)
            }
            _ => return Err(PDFError::Generic("/W must be an array".to_string())),
        };

        // Get Index array (ranges) - default is [0, Size]
        let index_array = if let Some(index) = dict.get("Index") {
            match index {
                PDFObject::Array(arr) => arr.clone(),
                _ => return Err(PDFError::Generic("/Index must be an array".to_string())),
            }
        } else {
            // Default: [0, Size]
            let size = dict
                .get("Size")
                .ok_or_else(|| PDFError::Generic("XRef stream missing /Size".to_string()))?;
            match size {
                PDFObject::Number(n) => {
                    use smallvec::smallvec;
                    smallvec![
                        Box::new(PDFObject::Number(0.0)),
                        Box::new(PDFObject::Number(*n))
                    ]
                }
                _ => return Err(PDFError::Generic("/Size must be a number".to_string())),
            }
        };

        // Decompress: apply_filters handles Name and [/FlateDecode] arrays.
        let mut decompressed_data = match dict.get("Filter") {
            Some(filter) => decode::apply_filters(data, filter)
                .map_err(|e| PDFError::Generic(format!("XRef stream decode error: {}", e)))?,
            None => data.to_vec(),
        };

        // Apply PNG predictor if specified in DecodeParms
        if let Some(decode_parms) = dict.get("DecodeParms") {
            if let PDFObject::Dictionary(parms) = decode_parms {
                // Check for Predictor
                if let Some(PDFObject::Number(predictor)) = parms.get("Predictor") {
                    let pred = *predictor as i32;
                    // PNG predictor values are 10-14 (10 = None, 11 = Sub, 12 = Up, 13 = Average, 14 = Paeth)
                    if pred >= 10 && pred <= 14 {
                        // Get Columns parameter (required for PNG predictor)
                        let columns = parms
                            .get("Columns")
                            .and_then(|obj| match obj {
                                PDFObject::Number(n) => Some(*n as usize),
                                _ => None,
                            })
                            .unwrap_or(1);

                        // Get Colors parameter (default 1)
                        let colors = parms
                            .get("Colors")
                            .and_then(|obj| match obj {
                                PDFObject::Number(n) => Some(*n as usize),
                                _ => None,
                            })
                            .unwrap_or(1);

                        // Get BitsPerComponent (default 8)
                        let bits_per_component = parms
                            .get("BitsPerComponent")
                            .and_then(|obj| match obj {
                                PDFObject::Number(n) => Some(*n as usize),
                                _ => None,
                            })
                            .unwrap_or(8);

                        // Apply PNG predictor
                        decompressed_data = decode::decode_png_predictor(
                            &decompressed_data,
                            colors,
                            bits_per_component,
                            columns,
                        )
                        .map_err(|e| {
                            PDFError::Generic(format!("PNG predictor decode error: {}", e))
                        })?;
                    }
                }
            }
        }

        // Parse entries from the decompressed data
        let (w1, w2, w3) = widths;
        let entry_size = w1 + w2 + w3;
        let mut pos = 0;

        // Process each range in the Index array
        let mut i = 0;
        while i < index_array.len() {
            let first = match &*index_array[i] {
                PDFObject::Number(n) => *n as u32,
                _ => {
                    return Err(PDFError::Generic(
                        "Index entry must be a number".to_string(),
                    ));
                }
            };

            let count = match &*index_array[i + 1] {
                PDFObject::Number(n) => *n as usize,
                _ => {
                    return Err(PDFError::Generic(
                        "Index entry must be a number".to_string(),
                    ));
                }
            };

            // Read 'count' entries starting from 'first'
            for j in 0..count {
                if pos + entry_size > decompressed_data.len() {
                    return Err(PDFError::Generic("XRef stream data truncated".to_string()));
                }

                // Read type field (w1 bytes)
                let entry_type = if w1 > 0 {
                    read_big_endian(&decompressed_data[pos..pos + w1])
                } else {
                    1 // Default type is 1 if w1 == 0
                };
                pos += w1;

                // Read second field (w2 bytes) - offset or obj stream num
                let field2 = if w2 > 0 {
                    read_big_endian(&decompressed_data[pos..pos + w2])
                } else {
                    0
                };
                pos += w2;

                // Read third field (w3 bytes) - generation or index
                let field3 = if w3 > 0 {
                    read_big_endian(&decompressed_data[pos..pos + w3])
                } else {
                    0
                };
                pos += w3;

                // Create entry based on type
                let obj_num = first + j as u32;
                let entry = match entry_type {
                    0 => XRefEntry::Free {
                        next_free: field2,
                        generation: field3 as u32,
                    },
                    1 => XRefEntry::Uncompressed {
                        offset: field2,
                        generation: field3 as u32,
                    },
                    2 => XRefEntry::Compressed {
                        obj_stream_num: field2 as u32,
                        index: field3 as u32,
                    },
                    _ => {
                        return Err(PDFError::xref_error(format!(
                            "Invalid XRef entry type: {} at object {}",
                            entry_type, obj_num
                        )));
                    }
                };

                // Ensure entries vector is large enough
                while self.entries.len() <= obj_num as usize {
                    self.entries.push(None);
                }

                // Only set if not already set (first entry wins)
                if self.entries[obj_num as usize].is_none() {
                    self.entries[obj_num as usize] = Some(entry);
                }
            }

            i += 2;
        }

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
                    )));
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
                    )));
                }
            };

            // Ensure we have enough space in the entries vector
            // Use checked arithmetic to prevent overflow on corrupt PDFs
            let needed_size = first.checked_add(count).ok_or_else(|| {
                PDFError::corrupted_pdf(format!(
                    "XRef table overflow: first={}, count={}",
                    first, count
                ))
            })? as usize;

            // Sanity check: prevent extremely large allocations
            if needed_size > 10_000_000 {
                // 10 million objects is unreasonable
                return Err(PDFError::corrupted_pdf(format!(
                    "XRef table size {} exceeds reasonable limit",
                    needed_size
                )));
            }

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
                )));
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
                )));
            }
        };

        // Read type (f = free, n = in use)
        let type_obj = parser.get_object()?;
        let entry_type = match type_obj {
            obj if obj.is_command("f") => "f",
            obj if obj.is_command("n") => "n",
            _ => {
                return Err(PDFError::Generic(format!(
                    "Expected 'f' or 'n' in xref entry, got {:?}",
                    type_obj
                )));
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
    /// The requested object wrapped in Arc
    fn fetch_compressed(&mut self, obj_stream_num: u32, index: u32) -> PDFResult<Arc<PDFObject>> {
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

                // Decompress: apply_filters handles Name and Array (e.g. [/FlateDecode]).
                // Using decode_stream with only Name caused silent no-op when Filter
                // is an array — zlib bytes were then parsed as objects
                // ("Expected object number, got Command(\"xÚ…\")") on Antenna
                // House / Foxit books under ~/Books.
                let mut decompressed_data = match dict.get("Filter") {
                    Some(filter) => decode::apply_filters(data, filter)
                        .map_err(|e| PDFError::Generic(format!("ObjStm decode error: {}", e)))?,
                    None => data.clone(),
                };

                // Apply PNG predictor if specified in DecodeParms
                if let Some(decode_parms) = dict.get("DecodeParms") {
                    if let PDFObject::Dictionary(parms) = decode_parms {
                        // Check for Predictor
                        if let Some(PDFObject::Number(predictor)) = parms.get("Predictor") {
                            let pred = *predictor as i32;
                            // PNG predictor values are 10-14
                            if pred >= 10 && pred <= 14 {
                                let columns = parms
                                    .get("Columns")
                                    .and_then(|obj| match obj {
                                        PDFObject::Number(n) => Some(*n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or(1);

                                let colors = parms
                                    .get("Colors")
                                    .and_then(|obj| match obj {
                                        PDFObject::Number(n) => Some(*n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or(1);

                                let bits_per_component = parms
                                    .get("BitsPerComponent")
                                    .and_then(|obj| match obj {
                                        PDFObject::Number(n) => Some(*n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or(8);

                                decompressed_data = decode::decode_png_predictor(
                                    &decompressed_data,
                                    colors,
                                    bits_per_component,
                                    columns,
                                )
                                .map_err(|e| {
                                    PDFError::Generic(format!("PNG predictor decode error: {}", e))
                                })?;
                            }
                        }
                    }
                }

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
                            )));
                        }
                    };

                    let obj_offset = match offset {
                        PDFObject::Number(n) => n as usize,
                        _ => {
                            return Err(PDFError::Generic(format!(
                                "Expected offset, got {:?}",
                                offset
                            )));
                        }
                    };

                    obj_nums.push(obj_num);
                    offsets.push(obj_offset);
                }

                // Now parse the object at the requested index
                let obj_offset = first + offsets[index as usize];

                // Validate offset is within bounds
                if obj_offset >= decompressed_data.len() {
                    return Err(PDFError::corrupted_pdf(format!(
                        "ObjStm: object offset {} exceeds stream length {}",
                        obj_offset,
                        decompressed_data.len()
                    )));
                }

                let obj_length = if (index as usize) < offsets.len() - 1 {
                    // Length is the difference between consecutive offsets
                    offsets[index as usize + 1] - offsets[index as usize]
                } else {
                    // Last object extends to end of data
                    decompressed_data.len() - obj_offset
                };

                // Validate the calculated range is within bounds
                let obj_end = obj_offset + obj_length;
                if obj_end > decompressed_data.len() {
                    return Err(PDFError::corrupted_pdf(format!(
                        "ObjStm: object range {}..{} exceeds stream length {}",
                        obj_offset,
                        obj_end,
                        decompressed_data.len()
                    )));
                }

                // Create a stream for just this object's data
                let obj_data = decompressed_data[obj_offset..obj_end].to_vec();
                let obj_stream = Stream::from_bytes(obj_data);
                let obj_lexer = Lexer::new(Box::new(obj_stream) as Box<dyn BaseStream>)?;
                let mut obj_parser = Parser::new(obj_lexer)?;

                // Parse the object (no "obj"/"endobj" wrappers in ObjStm)
                let object = Arc::new(obj_parser.get_object()?);

                // Objects stored in an object stream always have generation 0.
                let actual_obj_num = obj_nums[index as usize];
                self.cache
                    .borrow_mut()
                    .put(Ref::new(actual_obj_num, 0), Arc::clone(&object));

                Ok(object)
            }
            PDFObject::Dictionary(_) => {
                // If it's just a dictionary without stream data, we can't decompress it yet
                Err(PDFError::Generic(
                    "ObjStm is a dictionary but stream data parsing not yet implemented"
                        .to_string(),
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
    /// The object is cached after being parsed. Returns an Arc to avoid expensive cloning.
    ///
    /// **Lazy recovery:** if the fetch fails on a bad/stale offset *or* a missing xref
    /// entry (common in corrupt / linearized PDFs whose first-page xref parses but omits
    /// later objects — e.g. a broken `/Prev` was skipped), the first such failure triggers
    /// a full-file `recover_by_scan` once and retries — mirroring hayro's `get_with` →
    /// `repair()` model. Subsequent failures pass through.
    pub fn fetch(&mut self, obj_num: u32, generation: u32) -> PDFResult<Arc<PDFObject>> {
        // Circular-reference guard: a ref already being resolved on the stack (e.g. an
        // object whose indirect /Length refers back into its own dependency cycle) would
        // recurse unboundedly and overflow the stack. pdf.js's `_pendingRefs` returns a
        // CIRCULAR_REF; we return the null object.
        let key = (obj_num, generation);
        if self.pending_refs.contains(&key) {
            eprintln!("Warning: ignoring circular object reference {obj_num} {generation} R");
            return Ok(Arc::new(PDFObject::Null));
        }
        self.pending_refs.insert(key);
        let result = self.fetch_inner(obj_num, generation);
        self.pending_refs.remove(&key);

        match result {
            Ok(obj) => Ok(obj),
            // Lazy recovery (hayro `get_with` → `repair()`):
            // - BadXRefOffset: entry exists but offset is stale/wrong.
            // - ObjectNotInXRef: entry missing entirely — common for linearized first-page
            //   xrefs whose `/Prev` points into the middle of a classic table and was
            //   skipped (issue53.pdf: `/Prev 49380` lands on the free-entry `f` flag).
            // Rebuild once from a full-file object scan, then retry the fetch.
            Err(e)
                if !self.recovered
                    && matches!(
                        e,
                        PDFError::BadXRefOffset { .. } | PDFError::ObjectNotInXRef { .. }
                    ) =>
            {
                self.recovered = true;
                let _ = self.recover_by_scan();
                self.fetch_inner(obj_num, generation).map_err(|_| e)
            }
            Err(e) => Err(e),
        }
    }

    fn fetch_inner(&mut self, obj_num: u32, generation: u32) -> PDFResult<Arc<PDFObject>> {
        let cache_key = Ref::new(obj_num, generation);

        // Check cache first - Arc::clone is cheap (just increments refcount)
        if let Some(cached) = self.cache.borrow_mut().get(&cache_key) {
            return Ok(Arc::clone(cached));
        }

        // Get xref entry
        let entry = self
            .get_entry(obj_num)
            .ok_or(PDFError::ObjectNotInXRef { num: obj_num })?;

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
                let stream_length = self.stream.length();

                // Validate offset is within stream bounds
                if offset_value as usize >= stream_length {
                    return Err(PDFError::bad_xref_offset(
                        obj_num,
                        format!("offset {offset_value} exceeds stream length {stream_length}"),
                    ));
                }

                // Create a sub-stream starting at the object's position
                // No need to manipulate parent stream position - sub-stream is independent
                let sub_stream = self.stream.make_sub_stream(
                    offset_value as usize,
                    stream_length - offset_value as usize,
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
                        return Err(PDFError::bad_xref_offset(
                            obj_num,
                            format!("expected object number, got {num_obj:?}"),
                        ));
                    }
                };

                if parsed_num != obj_num {
                    return Err(PDFError::bad_xref_offset(
                        obj_num,
                        format!("object number mismatch: expected {obj_num}, got {parsed_num}"),
                    ));
                }

                // Read generation number
                let gen_obj = parser.get_object()?;
                let parsed_gen = match gen_obj {
                    PDFObject::Number(n) => n as u32,
                    _ => {
                        return Err(PDFError::bad_xref_offset(
                            obj_num,
                            format!("expected generation number, got {gen_obj:?}"),
                        ));
                    }
                };

                if parsed_gen != generation {
                    return Err(PDFError::bad_xref_offset(
                        obj_num,
                        format!("generation mismatch: expected {generation}, got {parsed_gen}"),
                    ));
                }

                // Read "obj" keyword
                let obj_keyword = parser.get_object()?;
                if !obj_keyword.is_command("obj") {
                    return Err(PDFError::bad_xref_offset(
                        obj_num,
                        format!("expected 'obj' keyword, got {obj_keyword:?}"),
                    ));
                }

                // Read the actual object
                let object = parser.get_object()?;

                // Decrypt streams/strings inline if a keyed decryptor is installed and
                // the object is not exempt (the /Encrypt dict itself is never encrypted).
                let object = if let Some(ref decryptor) = self.decryptor {
                    if self.encrypt_obj_num == Some(obj_num) {
                        object
                    } else {
                        Self::decrypt_object(decryptor, object, obj_num, generation)?
                    }
                } else {
                    object
                };
                let object_arc = Arc::new(object);

                // Cache the Arc - cheap clone
                self.cache
                    .borrow_mut()
                    .put(cache_key, Arc::clone(&object_arc));

                Ok(object_arc)
            }

            XRefEntry::Compressed {
                obj_stream_num,
                index,
            } => {
                if generation != 0 {
                    return Err(PDFError::Generic(format!(
                        "Generation mismatch for compressed object {}: expected 0, got {}",
                        obj_num, generation
                    )));
                }

                // Fetch from compressed object stream
                self.fetch_compressed(*obj_stream_num, *index)
            }
        }
    }

    /// Check the cache for an object without any I/O or stream access.
    /// Returns `None` if the object isn't cached yet (caller should fall back to
    /// [`fetch`](Self::fetch) which reads from the stream on a cache miss).
    ///
    /// This is the `&self`-compatible cache probe — part of the fetch split. Callers
    /// that only need to check whether an object is already resolved (e.g. the
    /// parser's /Length-resolver closure) use this instead of `fetch`, avoiding
    /// the need for `&mut self`.
    pub fn fetch_cached(&self, obj_num: u32, generation: u32) -> Option<Arc<PDFObject>> {
        self.cache
            .borrow_mut()
            .get(&Ref::new(obj_num, generation))
            .map(Arc::clone)
    }

    /// Fetches an object if it's a reference, otherwise returns the object as-is.
    ///
    /// Returns an Arc<PDFObject> — cheap clone (refcount inc) when fetching from cache.
    /// For non-Ref objects, wraps in a new Arc.
    pub fn fetch_if_ref(&mut self, obj: &PDFObject) -> PDFResult<Arc<PDFObject>> {
        match obj {
            PDFObject::Ref(ref_obj) => self.fetch(ref_obj.num, ref_obj.generation),
            _ => Ok(Arc::new(obj.clone())),
        }
    }

    /// Returns the trailer dictionary.
    pub fn trailer(&self) -> Option<&PDFObject> {
        self.trailer.as_ref()
    }

    /// Returns the length of the underlying stream.
    ///
    /// This is useful for determining the original PDF file size.
    pub fn stream_length(&self) -> usize {
        self.stream.length()
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
                _ => return Err(PDFError::Generic("Trailer is not a dictionary".to_string())),
            };

            trailer_dict
                .get("Root")
                .ok_or_else(|| PDFError::Generic("No Root entry in trailer".to_string()))?
                .clone()
        };

        // Fetch and dereference the Arc
        let rc_catalog = match &root_ref {
            PDFObject::Ref(ref_obj) => self.fetch(ref_obj.num, ref_obj.generation)?,
            _ => return Ok(root_ref),
        };

        Ok((*rc_catalog).clone())
    }

    /// Gets the file ID from the trailer dictionary.
    ///
    /// The file ID is used in encryption key derivation for PDF versions V=1,2,4.
    ///
    /// # Returns
    /// The file ID as a byte array, or an error if not found
    pub fn file_id(&self) -> PDFResult<Vec<u8>> {
        let trailer = self
            .trailer
            .as_ref()
            .ok_or_else(|| PDFError::Generic("No trailer dictionary".to_string()))?;

        let trailer_dict = match trailer {
            PDFObject::Dictionary(dict) => dict,
            _ => return Err(PDFError::Generic("Trailer is not a dictionary".to_string())),
        };

        // Get the ID entry - it's an array of two byte strings
        let id_obj = trailer_dict
            .get("ID")
            .ok_or_else(|| PDFError::Generic("No ID entry in trailer".to_string()))?;

        match id_obj {
            PDFObject::Array(arr) if !arr.is_empty() => {
                // The first element of the ID array is the file ID
                match &*arr[0] {
                    PDFObject::String(bytes) => Ok(bytes.to_vec()),
                    // `HexString` is already decoded to raw bytes by the parser (a `<...>`
                    // token), so the file ID is its bytes directly.
                    PDFObject::HexString(bytes) => Ok(bytes.to_vec()),
                    _ => Err(PDFError::Generic(
                        "ID array element is not a string".to_string(),
                    )),
                }
            }
            _ => Err(PDFError::Generic("ID is not an array".to_string())),
        }
    }

    /// Gets the /Encrypt dictionary from the trailer (if PDF is encrypted).
    ///
    /// # Returns
    /// The Encrypt dictionary as an indirect reference, or None if not encrypted
    pub fn get_encrypt_dict_ref(&self) -> Option<PDFObject> {
        let trailer = self.trailer.as_ref()?;

        let trailer_dict = match trailer {
            PDFObject::Dictionary(dict) => dict,
            _ => return None,
        };

        trailer_dict.get("Encrypt").cloned()
    }

    /// Install the keyed decryptor (file key already derived) so subsequent `fetch`es
    /// decrypt streams/strings inline. `encrypt_obj_num` is the `/Encrypt` object number,
    /// which is exempt from decryption. Pass `None` to disable decryption (locked doc).
    pub fn set_decryptor(
        &mut self,
        decryptor: Option<super::encryption::EncryptDict>,
        encrypt_obj_num: Option<u32>,
    ) {
        self.decryptor = decryptor;
        self.encrypt_obj_num = encrypt_obj_num;
        // Objects fetched while locked (or under a previous key) may sit in the
        // cache as ciphertext / garbage. Drop them so the next fetch re-reads
        // and decrypts with the newly installed key. Mirrors pdf.js building a
        // fresh CipherTransform and not reusing pre-auth object snapshots.
        self.cache.borrow_mut().clear();
    }

    /// Decrypt an object's encrypted streams/strings in place, per the installed
    /// decryptor. Mirrors pdf.js applying a `CipherTransform` during object fetch.
    /// No-op for non-encrypted content. `obj_num`/`generation` key the per-object cipher.
    fn decrypt_object(
        decryptor: &super::encryption::EncryptDict,
        object: PDFObject,
        obj_num: u32,
        generation: u32,
    ) -> PDFResult<PDFObject> {
        Ok(match object {
            PDFObject::Stream { mut dict, data } => {
                // Cross-reference streams are never encrypted.
                let is_xref = matches!(dict.get("Type"), Some(PDFObject::Name(n)) if n == "XRef");
                if is_xref {
                    PDFObject::Stream { dict, data }
                } else {
                    let decrypted = decryptor.decrypt_stream(&data, obj_num, generation)?;
                    PDFObject::Stream {
                        dict: std::mem::take(&mut dict),
                        data: decrypted,
                    }
                }
            }
            PDFObject::String(bytes) => {
                PDFObject::String(decryptor.decrypt_string(&bytes, obj_num, generation)?)
            }
            PDFObject::HexString(bytes) => {
                PDFObject::HexString(decryptor.decrypt_string(&bytes, obj_num, generation)?)
            }
            other => other,
        })
    }

    /// Returns the number of entries in the xref table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the xref table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Gets the current stream position.
    pub fn stream_pos(&self) -> usize {
        self.stream.pos()
    }

    /// Gets bytes from the stream at a specific position without changing current position.
    pub(crate) fn get_bytes(&mut self, pos: usize, length: usize) -> PDFResult<Vec<u8>> {
        // Save current position
        let current_pos = self.stream.pos();

        // Move to requested position
        self.stream.set_pos(pos)?;

        // Read bytes
        let mut bytes = vec![0u8; length];
        for i in 0..length {
            bytes[i] = self.stream.get_byte()?;
        }

        // Restore position
        self.stream.set_pos(current_pos)?;

        Ok(bytes)
    }
}

/// Helper function to read big-endian integer from bytes.
///
/// Used for reading XRef stream entry fields.
fn read_big_endian(bytes: &[u8]) -> u64 {
    let mut result = 0u64;
    for &byte in bytes {
        result = (result << 8) | (byte as u64);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::parser::Ref; // Import Ref for test code
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
    fn fetch_cache_is_scoped_by_generation() {
        let data = b"1 0 obj\n42\nendobj\n";
        let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);
        xref.entries.push(None);
        xref.entries.push(Some(XRefEntry::Uncompressed {
            offset: 0,
            generation: 0,
        }));

        let object = xref.fetch(1, 0).expect("generation 0 object should parse");
        assert_eq!(object.as_ref(), &PDFObject::Number(42.0));
        assert!(xref.fetch_cached(1, 0).is_some());
        assert!(xref.fetch_cached(1, 1).is_none());

        let error = xref
            .fetch(1, 1)
            .expect_err("a cached generation 0 object must not satisfy generation 1");
        assert!(
            error.to_string().contains("Generation mismatch"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn set_decryptor_drops_cached_objects() {
        // Objects fetched while locked (or under a previous key) may sit in the
        // cache as ciphertext/garbage; installing a decryptor must drop them so
        // the next fetch re-reads and decrypts. Mirrors pdf.js building a fresh
        // CipherTransform instead of reusing pre-auth snapshots.
        let data = b"1 0 obj\n42\nendobj\n";
        let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);
        xref.entries.push(None);
        xref.entries.push(Some(XRefEntry::Uncompressed {
            offset: 0,
            generation: 0,
        }));

        xref.fetch(1, 0).expect("object should parse while unencrypted");
        assert!(
            xref.fetch_cached(1, 0).is_some(),
            "fetch must populate the cache"
        );

        // Authenticate: installing a decryptor (even absent) must invalidate
        // pre-auth cache entries.
        xref.set_decryptor(None, None);
        assert!(
            xref.fetch_cached(1, 0).is_none(),
            "set_decryptor must drop cached pre-auth objects"
        );

        let refetched = xref.fetch(1, 0).expect("object should re-fetch after cache drop");
        assert_eq!(refetched.as_ref(), &PDFObject::Number(42.0));
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
        let ref_obj = PDFObject::Ref(Ref::new(1, 0));
        let result = xref.fetch_if_ref(&ref_obj).unwrap();
        assert_eq!(*result, PDFObject::Number(42.0));

        // Test with a direct object
        let direct_obj = PDFObject::Number(100.0);
        let result = xref.fetch_if_ref(&direct_obj).unwrap();
        assert_eq!(*result, PDFObject::Number(100.0));
    }

    #[test]
    #[ignore] // TODO: Fix test - stream data needs to be properly positioned in complete PDF
    fn test_parse_xref_stream() {
        // Create a minimal XRef stream
        // This tests parsing of PDF 1.5+ XRef streams (compressed cross-reference tables)
        //
        // XRef stream format:
        // N 0 obj
        // << /Type /XRef /Size 3 /W [1 2 1] >>
        // stream
        // <binary data>
        // endstream
        // endobj
        //
        // /W [1 2 1] means:
        // - 1 byte for type (0=free, 1=uncompressed, 2=compressed)
        // - 2 bytes for offset/obj_stream_num
        // - 1 byte for generation/index
        //
        // We'll create entries for objects 0-2:
        // Entry 0: Free (type=0, next_free=0, generation=255)
        // Entry 1: Uncompressed (type=1, offset=15, generation=0)
        // Entry 2: Uncompressed (type=1, offset=79, generation=0)

        // Build the PDF data manually
        let mut data = Vec::new();

        // Binary XRef stream data (12 bytes total: 3 entries * 4 bytes each)
        let xref_data = vec![
            // Entry 0: type=0, next_free=0 (0x0000), generation=255 (0xFF)
            0x00, 0x00, 0x00, 0xFF, // Entry 1: type=1, offset=15 (0x000F), generation=0
            0x01, 0x00, 0x0F, 0x00, // Entry 2: type=1, offset=79 (0x004F), generation=0
            0x01, 0x00, 0x4F, 0x00,
        ];

        // Object header
        data.extend_from_slice(b"1 0 obj\n");
        data.extend_from_slice(b"<< /Type /XRef /Size 3 /W [1 2 1] /Length 12 >>\n");
        data.extend_from_slice(b"stream\n");
        data.extend_from_slice(&xref_data);
        data.extend_from_slice(b"endstream\nendobj\n");

        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);

        xref.parse().unwrap();

        // Verify we have 3 entries
        assert_eq!(xref.len(), 3);

        // Check entry 0 (free)
        let entry0 = xref.get_entry(0).unwrap();
        assert!(entry0.is_free());
        assert_eq!(entry0.generation(), 255); // 0xFF = 255

        // Check entry 1 (uncompressed at offset 15)
        let entry1 = xref.get_entry(1).unwrap();
        assert!(!entry1.is_free());
        if let XRefEntry::Uncompressed { offset, generation } = entry1 {
            assert_eq!(*offset, 15);
            assert_eq!(*generation, 0);
        } else {
            panic!("Expected uncompressed entry, got {:?}", entry1);
        }

        // Check entry 2 (uncompressed at offset 79)
        let entry2 = xref.get_entry(2).unwrap();
        if let XRefEntry::Uncompressed { offset, generation } = entry2 {
            assert_eq!(*offset, 79);
            assert_eq!(*generation, 0);
        } else {
            panic!("Expected uncompressed entry, got {:?}", entry2);
        }

        // Verify trailer dictionary contains XRef stream properties
        let trailer = xref.trailer().unwrap();
        if let PDFObject::Dictionary(dict) = trailer {
            // Check /Type is /XRef
            if let Some(PDFObject::Name(type_name)) = dict.get("Type") {
                assert_eq!(type_name, "XRef");
            } else {
                panic!("Expected /Type /XRef in trailer");
            }

            // Check /Size is 3
            if let Some(PDFObject::Number(size)) = dict.get("Size") {
                assert_eq!(*size, 3.0);
            } else {
                panic!("Expected /Size 3 in trailer");
            }
        } else {
            panic!("Expected dictionary trailer");
        }
    }

    #[test]
    #[ignore] // TODO: Fix test - stream data needs to be properly positioned in complete PDF
    fn test_parse_xref_stream_with_compressed_entries() {
        // Test XRef stream with type 2 (compressed) entries
        // This represents objects stored in ObjStm (object streams)
        //
        // Entry 0: Free (type=0)
        // Entry 1: Compressed in stream 5, index 0 (type=2, obj_stream=5, index=0)
        // Entry 2: Compressed in stream 5, index 1 (type=2, obj_stream=5, index=1)

        let mut data = Vec::new();

        // Binary XRef stream data (12 bytes total: 3 entries * 4 bytes each)
        let xref_data = vec![
            // Entry 0: free
            0x00, 0x00, 0x00, 0xFF, // Entry 1: compressed in stream 5, index 0
            0x02, 0x00, 0x05, 0x00, // Entry 2: compressed in stream 5, index 1
            0x02, 0x00, 0x05, 0x01,
        ];

        // Object header
        data.extend_from_slice(b"1 0 obj\n");
        data.extend_from_slice(b"<< /Type /XRef /Size 3 /W [1 2 1] /Length 12 >>\n");
        data.extend_from_slice(b"stream\n");
        data.extend_from_slice(&xref_data);
        data.extend_from_slice(b"endstream\nendobj\n");

        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);

        xref.parse().unwrap();

        assert_eq!(xref.len(), 3);

        // Check entry 0 (free)
        let entry0 = xref.get_entry(0).unwrap();
        assert!(entry0.is_free());

        // Check entry 1 (compressed)
        let entry1 = xref.get_entry(1).unwrap();
        if let XRefEntry::Compressed {
            obj_stream_num,
            index,
        } = entry1
        {
            assert_eq!(*obj_stream_num, 5);
            assert_eq!(*index, 0);
        } else {
            panic!("Expected compressed entry, got {:?}", entry1);
        }

        // Check entry 2 (compressed)
        let entry2 = xref.get_entry(2).unwrap();
        if let XRefEntry::Compressed {
            obj_stream_num,
            index,
        } = entry2
        {
            assert_eq!(*obj_stream_num, 5);
            assert_eq!(*index, 1);
        } else {
            panic!("Expected compressed entry, got {:?}", entry2);
        }
    }

    /// Lazy recovery: a missing xref entry (not just a bad offset) must trigger
    /// `recover_by_scan` once so linearized PDFs with a broken `/Prev` still resolve
    /// early objects. Regression for hayro-tests `issue53.pdf` (Object 1 not found).
    #[test]
    fn fetch_missing_entry_triggers_recover_by_scan() {
        let mut data = Vec::new();
        data.extend_from_slice(b"%PDF-1.4\n");
        // Real objects live in the file body but are deliberately ABSENT from the xref.
        data.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        data.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
        // A partial first-page-style xref that only indexes object 2 — object 1 is missing,
        // mirroring issue53's linearized section (objects 4..17 only) after /Prev is skipped.
        let xref_off = data.len();
        data.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n2 1\n");
        // offset of obj 2: find it
        let obj2_off = data
            .windows(b"2 0 obj".len())
            .position(|w| w == b"2 0 obj")
            .unwrap();
        data.extend_from_slice(format!("{obj2_off:010} 00000 n \n").as_bytes());
        data.extend_from_slice(b"trailer\n<< /Size 3 /Root 1 0 R >>\n");
        data.extend_from_slice(format!("startxref\n{xref_off}\n%%EOF\n").as_bytes());

        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);
        // Install the partial table only (skip parse of startxref path).
        xref.entries.clear();
        xref.entries.push(Some(XRefEntry::Free {
            next_free: 0,
            generation: 65535,
        })); // obj 0
        xref.entries.push(None); // obj 1 MISSING
        xref.entries.push(Some(XRefEntry::Uncompressed {
            offset: obj2_off as u64,
            generation: 0,
        }));
        let mut trailer_dict = std::collections::HashMap::new();
        trailer_dict.insert("Root".to_string(), PDFObject::Ref(Ref::new(1, 0)));
        trailer_dict.insert("Size".to_string(), PDFObject::Number(3.0));
        xref.trailer = Some(PDFObject::Dictionary(trailer_dict));
        xref.recovered = false;

        assert!(!xref.recovered, "precondition: recovery has not run yet");
        let obj = xref
            .fetch(1, 0)
            .expect("missing entry must recover via full-file scan and fetch obj 1");
        assert!(
            xref.recovered,
            "fetch of missing entry must set recovered flag"
        );
        match obj.as_ref() {
            PDFObject::Dictionary(dict) => {
                assert!(
                    dict.get("Type").is_some() || dict.get("Pages").is_some(),
                    "recovered obj 1 should be the catalog dict, got {dict:?}"
                );
            }
            other => panic!("expected dictionary catalog, got {other:?}"),
        }
    }

    /// `recover_by_scan` rebuilds the xref table from object offsets when the file has
    /// no xref table / no startxref (truncated tail). Ports pdf.js `indexObjects`.
    #[test]
    fn test_recover_by_scan_rebuilds_entries_and_trailer() {
        let mut data = Vec::new();
        data.extend_from_slice(b"%PDF-1.4\n");
        data.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        data.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
        data.extend_from_slice(b"trailer\n<< /Size 3 /Root 1 0 R >>\n");
        // Intentionally NO xref table and NO startxref/%%EOF.

        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);

        xref.recover_by_scan()
            .expect("recovery scan rebuilds xref from object offsets");

        // Entries for objects 1 and 2 recorded as uncompressed, generation 0.
        let e1 = xref.get_entry(1).expect("obj 1 entry recovered");
        let e2 = xref.get_entry(2).expect("obj 2 entry recovered");
        assert!(
            matches!(e1, XRefEntry::Uncompressed { generation: 0, .. }),
            "obj 1 should be uncompressed generation 0, got {e1:?}"
        );
        assert!(
            matches!(e2, XRefEntry::Uncompressed { generation: 0, .. }),
            "obj 2 should be uncompressed generation 0, got {e2:?}"
        );

        // A trailer was selected and /Root resolves to a catalog.
        assert!(xref.trailer().is_some(), "recovery selected a trailer");
        xref.catalog()
            .expect("catalog resolves via recovered trailer");
    }

    /// A circular object reference (A's /Length is `B R`, B's /Length is `A R`) must NOT
    /// recurse to a stack overflow — `fetch` returns the null object for the re-entered ref.
    #[test]
    fn fetch_circular_reference_returns_null() {
        // Two dicts that reference each other via indirect /Length: fetch(1) resolves its
        // /Length 2 0 R via fetch(2), whose /Length 1 0 R re-enters fetch(1) → cycle.
        let data = b"1 0 obj\n<< /Length 2 0 R >>\nendobj\n2 0 obj\n<< /Length 1 0 R >>\nendobj\n";
        let stream = Box::new(Stream::from_bytes(data.to_vec())) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);
        // Place entries directly at the real offsets (bypass full recovery, which needs a
        // trailer — the cycle behavior doesn't depend on the trailer).
        let p1 = data
            .windows(b"1 0 obj".len())
            .position(|w| w == b"1 0 obj")
            .unwrap();
        let p2 = data
            .windows(b"2 0 obj".len())
            .position(|w| w == b"2 0 obj")
            .unwrap();
        xref.entries.push(None); // obj 0
        xref.entries.push(Some(XRefEntry::Uncompressed {
            offset: p1 as u64,
            generation: 0,
        }));
        xref.entries.push(Some(XRefEntry::Uncompressed {
            offset: p2 as u64,
            generation: 0,
        }));
        // Must terminate (no stack overflow) — the re-entered fetch returns Null.
        let _ = xref
            .fetch(1, 0)
            .expect("circular fetch terminates, not stack-overflows");
    }
}

// ---------------------------------------------------------------------------
// Recovery-scan byte helpers (used by `XRef::recover_by_scan`). Hand-rolled to
// avoid pulling in a `regex` dependency; mirrors pdf.js's `indexObjects` scan.
// ---------------------------------------------------------------------------

/// Length of the token starting at `off`: bytes until LF/CR/LT('<'). (pdf.js `readToken`.)
fn read_token_len(buf: &[u8], mut off: usize) -> usize {
    let start = off;
    while off < buf.len() && buf[off] != 0xa && buf[off] != 0xd && buf[off] != b'<' {
        off += 1;
    }
    off - start
}

/// True if `token` starts with `kw` and is followed by whitespace or end-of-token
/// (i.e. `kw` is a whole keyword, not a prefix of a longer word).
fn starts_with_keyword(token: &[u8], kw: &[u8]) -> bool {
    token.starts_with(kw) && (token.len() == kw.len() || token[kw.len()].is_ascii_whitespace())
}

/// Offset of the first occurrence of `needle` at/after `off`, or `None`.
/// (pdf.js `skipUntil`, but returns the absolute offset of the match.)
fn skip_until(buf: &[u8], off: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || off >= buf.len() {
        return None;
    }
    buf[off..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| off + p)
}

/// Parse a leading `(\d+)[ \t\r\n]+(\d+)[ \t\r\n]+obj\b` object header from `token`.
/// Returns `(object_number, generation)` on match. (pdf.js `objRegExp`.)
fn parse_obj_header(token: &[u8]) -> Option<(u32, u32)> {
    let mut i = 0;
    let num_start = i;
    while i < token.len() && token[i].is_ascii_digit() {
        i += 1;
    }
    if i == num_start {
        return None;
    }
    let num: u32 = std::str::from_utf8(&token[num_start..i])
        .ok()?
        .parse()
        .ok()?;
    // required whitespace
    if i >= token.len() || !token[i].is_ascii_whitespace() {
        return None;
    }
    while i < token.len()
        && (token[i] == b' ' || token[i] == b'\t' || token[i] == b'\r' || token[i] == b'\n')
    {
        i += 1;
    }
    let gen_start = i;
    while i < token.len() && token[i].is_ascii_digit() {
        i += 1;
    }
    if i == gen_start {
        return None;
    }
    let generation: u32 = std::str::from_utf8(&token[gen_start..i])
        .ok()?
        .parse()
        .ok()?;
    if i >= token.len() || !token[i].is_ascii_whitespace() {
        return None;
    }
    while i < token.len()
        && (token[i] == b' ' || token[i] == b'\t' || token[i] == b'\r' || token[i] == b'\n')
    {
        i += 1;
    }
    // "obj" + word boundary
    if token[i..].starts_with(b"obj") {
        let after = i + 3;
        if after == token.len() || !token[after].is_ascii_alphanumeric() {
            return Some((num, generation));
        }
    }
    None
}

/// Offset of the next `endobj` at/after `off` (bounds an object's body for the
/// `/XRef`-stream check and for advancing past the object).
fn find_endobj(buf: &[u8], off: usize) -> Option<usize> {
    skip_until(buf, off, b"endobj")
}

/// Byte-scan a small window at `off` for `/Root <num> <generation> R` and return the ref.
/// Used by the recovery trailer fallback to avoid invoking the parser (which can loop
/// on garbage at bogus recovery offsets). Bounded → cannot loop.
fn extract_root_ref(buf: &[u8], off: usize) -> Option<(u32, u32)> {
    let win = 512.min(buf.len().saturating_sub(off));
    let region = &buf[off..off + win];
    let i = region.windows(b"/Root".len()).position(|w| w == b"/Root")?;
    let mut p = i + b"/Root".len();
    let skip_ws = |p: &mut usize| {
        while *p < region.len() && region[*p].is_ascii_whitespace() {
            *p += 1;
        }
    };
    let read_num = |p: &mut usize| -> Option<u32> {
        let s = *p;
        while *p < region.len() && region[*p].is_ascii_digit() {
            *p += 1;
        }
        std::str::from_utf8(&region[s..*p]).ok()?.parse().ok()
    };
    skip_ws(&mut p);
    let num = read_num(&mut p)?;
    skip_ws(&mut p);
    let generation = read_num(&mut p)?;
    skip_ws(&mut p);
    if p < region.len() && region[p] == b'R' {
        Some((num, generation))
    } else {
        None
    }
}

/// Ensure `entries` is large enough for `num`, then set the entry only if unset
/// (first-writer-wins, matching pdf.js's `if (!this.entries[num])`).
fn set_entry_if_none(entries: &mut Vec<Option<XRefEntry>>, num: u32, entry: XRefEntry) {
    while entries.len() <= num as usize {
        entries.push(None);
    }
    if entries[num as usize].is_none() {
        entries[num as usize] = Some(entry);
    }
}

/// Skip whitespace from `p`, then return true if the next byte is a plausible start of a
/// PDF object (`<`, `(`, `[`, `/`, a number, or `t`/`f`/`n` for true/false/null). Used by
/// the recovery scan to reject false-positive `N G obj` headers in binary content WITHOUT
/// invoking the parser (which can loop on garbage). (hayro validates by attempting a read.)
fn plausible_object_start(buf: &[u8], mut p: usize) -> bool {
    while p < buf.len() && matches!(buf[p], b' ' | b'\t' | b'\r' | b'\n') {
        p += 1;
    }
    if p >= buf.len() {
        return false;
    }
    let c = buf[p];
    matches!(c, b'<' | b'(' | b'[' | b'/' | b'.' | b'+' | b'-')
        || c.is_ascii_digit()
        || matches!(c, b't' | b'f' | b'n')
}

/// True if an object body contains a `/ObjStm` name (followed by a non-letter), i.e. the
/// object is an Object Stream whose contained objects the recovery scan should index.
fn has_type_objstm(body: &[u8]) -> bool {
    match skip_until(body, 0, b"/ObjStm") {
        Some(off) => {
            let after = off + b"/ObjStm".len();
            after >= body.len() || !body[after].is_ascii_alphabetic()
        }
        None => false,
    }
}

/// Read a numeric dict entry as `usize` (helper for ObjStm DecodeParms parsing).
fn num_in(dict: &HashMap<String, PDFObject>, key: &str) -> Option<usize> {
    match dict.get(key)? {
        PDFObject::Number(n) => Some(*n as usize),
        _ => None,
    }
}
