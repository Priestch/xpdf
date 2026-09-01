//! Content-stream editing infrastructure for text replacement.
//!
//! Provides byte-exact spanned editing of PDF content streams using a
//! splice-based approach: untouched operations are copied verbatim from the
//! original bytes; only modified operations are re-serialized.
//!
//! Architecture:
//! - Direct `Lexer` tokenization (no `Parser` — avoids 2-token-lookahead
//!   position corruption).
//! - Operation spans tile the decoded bytes exactly: `[prev_op_end,
//!   this_cmd_after_pos]` with no overlaps.
//! - Splice: `bytes[..span.start] + new_op_bytes + bytes[span.end..]`.

use std::ops::Range;

use super::content_stream::OpCode;
use super::error::{PDFError, PDFResult};
use super::lexer::{Lexer, Token};
use super::stream::Stream;

// ── Token record ─────────────────────────────────────────────────────

/// A token with the stream position recorded immediately after the Lexer
/// consumed it.  `after_pos` equals `Lexer::get_position()` at that point.
struct TokenRecord {
    after_pos: usize,
    token: Token,
}

// ── Spanned operation ────────────────────────────────────────────────

/// An operation with its byte span in the decoded content stream.
///
/// `span` covers everything from the end of the previous operation (or
/// byte 0 for the first) through the byte *after* the command operator's
/// trailing delimiter.  Unmodified operations can be copied verbatim;
/// only the target operation needs re-serialization.
#[derive(Debug, Clone)]
pub struct SpannedOperation {
    /// The operator.
    pub op: Option<OpCode>,
    /// Byte range `[start, end)` in `EditableContentStream::bytes`.
    pub span: Range<usize>,
    /// Index of the first token (first operand, or the command itself
    /// for zero-arg operators like `BT`, `ET`).
    pub first_token_idx: usize,
    /// Index of the command token.
    pub cmd_token_idx: usize,
    /// Which `/Contents` fragment this operation belongs to.
    pub stream_index: usize,
    /// `true` for inline image operations (`BI ... ID data EI`).
    pub is_inline_image: bool,
}

// ── Content fragment (original stream metadata) ─────────────────────

/// Metadata about one `/Contents` stream, for write-back.
#[derive(Debug, Clone)]
pub struct ContentFragment {
    /// Stream dictionary keys that affect re-encoding.
    pub filter_name: Option<String>,
    /// Whether /DecodeParms is present (v1 only supports bare FlateDecode).
    pub has_decode_parms: bool,
    /// Original (possibly compressed) data.
    pub raw: Vec<u8>,
    /// Byte offset of this fragment within the concatenated decoded bytes.
    pub decoded_offset: usize,
    /// Length of this fragment's decoded bytes.
    pub decoded_len: usize,
}

// ── Editable content stream ──────────────────────────────────────────

/// A page's `/Contents`, decoded and indexed for splice-based editing.
pub struct EditableContentStream {
    /// Concatenated decoded bytes of all `/Contents` streams.
    /// This is the working buffer that gets spliced.
    bytes: Vec<u8>,
    /// Parsed operations in stream order.
    pub ops: Vec<SpannedOperation>,
    /// Original fragment metadata (for write-back).
    pub fragments: Vec<ContentFragment>,
}

impl EditableContentStream {
    /// Build from already-decoded content stream fragments.
    ///
    /// Each `(decoded_bytes, fragment)` pair represents one entry in the
    /// page's `/Contents` array (or a single stream).  The caller is
    /// responsible for resolving references and decompressing.
    pub fn build(fragments: &[(Vec<u8>, ContentFragment)]) -> PDFResult<Self> {
        let mut bytes = Vec::new();
        let mut all_ops = Vec::new();

        for (stream_index, (decoded, frag)) in fragments.iter().enumerate() {
            let offset = bytes.len();
            bytes.extend_from_slice(decoded);

            let mut lexer = Lexer::new(Box::new(Stream::from_bytes(decoded.clone())) as _)?;

            let mut tokens: Vec<TokenRecord> = Vec::new();
            loop {
                let token = lexer.get_object()?;
                let after = lexer.get_position();
                if matches!(token, Token::EOF) {
                    break;
                }
                tokens.push(TokenRecord {
                    after_pos: after,
                    token,
                });
            }

            let mut prev_op_end = 0; // relative to this fragment's decoded bytes
            let mut i = 0;

            while i < tokens.len() {
                // Find the next Command token.
                let cmd_idx = match tokens[i..]
                    .iter()
                    .position(|t| matches!(t.token, Token::Command(_)))
                {
                    Some(j) => i + j,
                    None => break,
                };

                let op_start = prev_op_end;
                let mut op_end = tokens[cmd_idx].after_pos;
                let mut is_inline_image = false;

                // Handle inline images: BI ... key/value pairs ... ID data EI.
                if let Token::Command(ref cmd) = tokens[cmd_idx].token {
                    if cmd == "BI" {
                        is_inline_image = true;
                        // Walk forward to find EI.
                        let mut j = cmd_idx + 1;
                        while j < tokens.len() {
                            if let Token::Command(s) = &tokens[j].token {
                                if s == "EI" {
                                    op_end = tokens[j].after_pos;
                                    i = j + 1;
                                    break;
                                }
                            }
                            j += 1;
                        }
                        if j >= tokens.len() {
                            // Malformed: no EI found.
                            i = tokens.len();
                            continue;
                        }
                    }
                }

                let cmd_str = match &tokens[cmd_idx].token {
                    Token::Command(s) => s.as_str(),
                    _ => unreachable!("cmd_idx always points to a Command token"),
                };

                let op_code = OpCode::from_command(cmd_str).ok();

                all_ops.push(SpannedOperation {
                    op: op_code,
                    span: (offset + op_start)..(offset + op_end),
                    first_token_idx: i,
                    cmd_token_idx: cmd_idx,
                    stream_index,
                    is_inline_image,
                });

                prev_op_end = op_end;
                if !is_inline_image {
                    i = cmd_idx + 1;
                }
            }
        }

        Ok(EditableContentStream {
            bytes,
            ops: all_ops,
            fragments: fragments.iter().map(|(_, f)| f.clone()).collect(),
        })
    }

    /// Returns the current working bytes (decoded, with any splices applied).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Splice `replacement` bytes over the span of `ops[op_index]`.
    ///
    /// After the splice, all spans for operations after `op_index` are
    /// shifted by `delta = replacement.len() as isize - old_span.len()`.
    /// The caller must NOT hold references to previous span values after
    /// calling this.
    pub fn replace_op_bytes(&mut self, op_index: usize, replacement: Vec<u8>) {
        let old_span = self.ops[op_index].span.clone();
        let old_bytes_len = self.bytes.len();
        let delta = replacement.len() as isize - old_span.len() as isize;

        // Splice the byte buffer.
        self.bytes.splice(old_span.clone(), replacement);

        // Update the replaced op's span end: start stays, end = start + new_len.
        let new_len = (old_span.end - old_span.start) as isize + delta;
        self.ops[op_index].span.end = old_span.start + new_len as usize;

        // Shift all subsequent operation spans.
        if delta != 0 {
            for op in &mut self.ops[op_index + 1..] {
                op.span.start = (op.span.start as isize + delta) as usize;
                op.span.end = (op.span.end as isize + delta) as usize;
            }
        }

        // Update fragment boundaries to match the new byte buffer.
        self.recalc_fragment_bounds(op_index, old_bytes_len, self.bytes.len());
    }

    /// Recalculate fragment `decoded_offset`/`decoded_len` after a splice.
    ///
    /// `old_bytes_len` is the byte-buffer length *before* the splice.
    /// `new_bytes_len` is the length *after*.  The affected fragment's
    /// length and all subsequent fragments' offsets shift by the difference.
    fn recalc_fragment_bounds(&mut self, op_index: usize, old_bytes_len: usize, new_bytes_len: usize) {
        let delta = new_bytes_len as isize - old_bytes_len as isize;
        if delta == 0 {
            return;
        }
        let stream_index = self.ops[op_index].stream_index;
        // Adjust this fragment's length.
        self.fragments[stream_index].decoded_len =
            (self.fragments[stream_index].decoded_len as isize + delta) as usize;
        // Shift subsequent fragments' offsets.
        for frag in &mut self.fragments[stream_index + 1..] {
            frag.decoded_offset = (frag.decoded_offset as isize + delta) as usize;
        }
    }

    /// Serialize each fragment back to (data, dict_entries) pairs.
    ///
    /// For unmodified fragments: returns the original raw data.
    /// For modified fragments: returns the spliced decoded bytes with
    /// `/Length` updated and re-compression applied if needed.
    pub fn serialize_fragments(
        &self,
    ) -> PDFResult<Vec<(Vec<u8>, Option<String>)>> {
        let mut results = Vec::with_capacity(self.fragments.len());

        for frag in &self.fragments {
            let decoded = &self.bytes[frag.decoded_offset..frag.decoded_offset + frag.decoded_len];

            // Check if this fragment was modified by comparing to original.
            // For now, always re-encode from the working bytes.
            // TODO: track dirty flag per fragment for efficiency.

            let (data, filter) = match &frag.filter_name {
                None => {
                    // No filter — store raw decoded bytes.
                    (decoded.to_vec(), None)
                }
                Some(f) if f == "FlateDecode" && !frag.has_decode_parms => {
                    // Re-compress with flate.
                    use flate2::write::DeflateEncoder;
                    use flate2::Compression;
                    use std::io::Write;
                    let mut buf = Vec::new();
                    {
                        let mut encoder =
                            DeflateEncoder::new(&mut buf, Compression::fast());
                        encoder
                            .write_all(decoded)
                            .map_err(|e| PDFError::Generic(format!("flate compress: {e}")))?;
                        let _ = encoder.finish();
                    }
                    (buf, Some("FlateDecode".to_string()))
                }
                Some(other) => {
                    return Err(PDFError::Generic(format!(
                        "unsupported content stream filter: {other}"
                    )));
                }
            };

            results.push((data, filter));
        }

        Ok(results)
    }
}

// ── Text run locator ─────────────────────────────────────────────

use rustc_hash::FxHashMap;

/// Which kind of text-showing operator produced this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextShowKind {
    /// `Tj` — show text string
    Tj,
    /// `TJ` — show text with individual glyph positioning
    TJ,
    /// `'` — move to next line and show text
    Quote,
    /// `"` — set spacing, move to next line, show text
    DoubleQuote,
}

/// A located text-showing operation with decoded content and text state.
#[derive(Debug, Clone)]
pub struct TextRunOp {
    /// Index into `EditableContentStream::ops`.
    pub op_index: usize,
    /// Which /Contents fragment.
    pub stream_index: usize,
    /// Byte span in `EditableContentStream::bytes`.
    pub span: Range<usize>,
    /// The operator kind.
    pub kind: TextShowKind,
    /// Active font name at this point (from the most recent `Tf`).
    pub font_name: Option<String>,
    /// Font size from the most recent `Tf`.
    pub font_size: f64,
    /// Text matrix at this point.
    pub text_matrix: [f64; 6],
    /// Decoded Unicode text.
    pub decoded: String,
    /// Original char codes (parallel to `decoded`).
    pub codes: Vec<u16>,
    /// Whether the font is a CID font.
    pub is_cid: bool,
}

/// Options for `find_text`.
#[derive(Debug, Clone, Default)]
pub struct FindOptions {
    /// 0-based occurrence index. Default 0 (first match).
    pub occurrence: usize,
}

/// A text match result — which operation(s) contain the needle and where.
#[derive(Debug, Clone)]
pub struct TextMatch {
    /// Index of the SpannedOperation that contains the match.
    pub op_index: usize,
    /// Byte range of the matched substring within `decoded`.
    pub char_range: Range<usize>,
    /// The matched text.
    pub matched_text: String,
}

/// Walks `SpannedOperation`s and locates text-showing operations,
/// tracking text state (Tm, Tlm, font, size) without merging runs.
pub struct TextRunLocator<'a> {
    ecs: &'a EditableContentStream,
    fonts: &'a FxHashMap<String, super::font::Font>,
}

impl<'a> TextRunLocator<'a> {
    pub fn new(
        ecs: &'a EditableContentStream,
        fonts: &'a FxHashMap<String, super::font::Font>,
    ) -> Self {
        Self { ecs, fonts }
    }

    /// Walk all operations and collect text-showing runs.
    pub fn collect_runs(&self) -> Vec<TextRunOp> {
        let mut runs = Vec::new();
        let mut text_matrix: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mut text_line_matrix: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mut current_font_name: Option<String> = None;
        let mut current_font_size: f64 = 12.0;
        let mut in_text_object = false;
        let mut leading: f64 = 0.0;

        // We need the raw token-level data from the Lexer to get
        // string arg bytes.  For now, extract them from the span bytes
        // by re-tokenizing each text op's span.
        for (i, op) in self.ecs.ops.iter().enumerate() {
            let Some(op_code) = op.op else { continue };

            match op_code {
                OpCode::BeginText => {
                    text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                    text_line_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                    in_text_object = true;
                }
                OpCode::EndText => {
                    in_text_object = false;
                }
                OpCode::SetFont => {
                    // Extract font name and size from the op's raw bytes.
                    // Parse the span to find the two operands.
                    let span_bytes = &self.ecs.bytes[op.span.clone()];
                    if let Some((name, size)) = parse_tf_args(span_bytes) {
                        current_font_name = Some(name);
                        current_font_size = size;
                    }
                }
                OpCode::SetTextMatrix => {
                    // Tm — 6 numbers.
                    let span_bytes = &self.ecs.bytes[op.span.clone()];
                    if let Some(m) = parse_matrix_args(span_bytes) {
                        text_matrix = m;
                        text_line_matrix = m;
                    }
                }
                OpCode::MoveText => {
                    let span_bytes = &self.ecs.bytes[op.span.clone()];
                    if let Some((tx, ty)) = parse_two_numbers(span_bytes) {
                        text_line_matrix[4] += tx;
                        text_line_matrix[5] += ty;
                        text_matrix = text_line_matrix;
                    }
                }
                OpCode::SetLeadingMoveText => {
                    let span_bytes = &self.ecs.bytes[op.span.clone()];
                    if let Some((tx, ty)) = parse_two_numbers(span_bytes) {
                        leading = -ty;
                        text_line_matrix[4] += tx;
                        text_line_matrix[5] += ty;
                        text_matrix = text_line_matrix;
                    }
                }
                OpCode::NextLine => {
                    text_line_matrix[4] = 0.0;
                    text_line_matrix[5] -= leading;
                    text_matrix = text_line_matrix;
                }
                OpCode::ShowText if in_text_object => {
                    let (decoded, codes) = self.decode_tj_op(op, i);
                    let (is_cid, font_name) = current_font_name
                        .as_ref()
                        .map(|n| {
                            self.fonts
                                .get(n)
                                .map(|f| (f.font_type().is_cid_font(), Some(n.clone())))
                                .unwrap_or((false, Some(n.clone())))
                        })
                        .unwrap_or((false, None));

                    runs.push(TextRunOp {
                        op_index: i,
                        stream_index: op.stream_index,
                        span: op.span.clone(),
                        kind: TextShowKind::Tj,
                        font_name,
                        font_size: current_font_size,
                        text_matrix,
                        decoded,
                        codes,
                        is_cid,
                    });
                }
                OpCode::ShowSpacedText if in_text_object => {
                    let (decoded, codes) = self.decode_tj_array_op(op, i);
                    let (is_cid, font_name) = current_font_name
                        .as_ref()
                        .map(|n| {
                            self.fonts
                                .get(n)
                                .map(|f| (f.font_type().is_cid_font(), Some(n.clone())))
                                .unwrap_or((false, Some(n.clone())))
                        })
                        .unwrap_or((false, None));

                    runs.push(TextRunOp {
                        op_index: i,
                        stream_index: op.stream_index,
                        span: op.span.clone(),
                        kind: TextShowKind::TJ,
                        font_name,
                        font_size: current_font_size,
                        text_matrix,
                        decoded,
                        codes,
                        is_cid,
                    });
                }
                _ => {}
            }
        }

        runs
    }

    /// Find `needle` as an exact Unicode substring within a single Tj/TJ op.
    /// Returns the first match (or the N-th if `opts.occurrence` is set).
    pub fn find_text(&self, needle: &str, opts: &FindOptions) -> Option<TextMatch> {
        let runs = self.collect_runs();
        let mut occurrence = 0;

        for run in &runs {
            if let Some(idx) = run.decoded.find(needle) {
                if occurrence == opts.occurrence {
                    return Some(TextMatch {
                        op_index: run.op_index,
                        char_range: idx..idx + needle.chars().count(),
                        matched_text: needle.to_string(),
                    });
                }
                occurrence += 1;
            }
        }

        None
    }

    /// Decode a Tj operation's string argument from its raw span bytes.
    fn decode_tj_op(&self, op: &SpannedOperation, op_index: usize) -> (String, Vec<u16>) {
        let span_bytes = &self.ecs.bytes[op.span.clone()];
        // Extract the string bytes from the span: last token before the command.
        // For Tj, the operand is a (string) or <hex>.
        if let Some(text_bytes) = extract_single_string_arg(span_bytes) {
            let codes: Vec<u16> = if self.is_cid_font(op_index) {
                // 2-byte codes
                text_bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect()
            } else {
                text_bytes.iter().map(|&b| b as u16).collect()
            };
            let decoded = self.decode_bytes(op_index, &text_bytes);
            (decoded, codes)
        } else {
            (String::new(), Vec::new())
        }
    }

    /// Decode a TJ operation's array argument from its raw span bytes.
    fn decode_tj_array_op(&self, op: &SpannedOperation, op_index: usize) -> (String, Vec<u16>) {
        let span_bytes = &self.ecs.bytes[op.span.clone()];
        let items = extract_tj_array_items(span_bytes);

        let is_cid = self.is_cid_font(op_index);
        let mut decoded = String::new();
        let mut all_codes = Vec::new();

        for item in &items {
            match item {
                TjRawItem::Text(text_bytes) => {
                    let codes: Vec<u16> = if is_cid {
                        text_bytes
                            .chunks_exact(2)
                            .map(|c| u16::from_be_bytes([c[0], c[1]]))
                            .collect()
                    } else {
                        text_bytes.iter().map(|&b| b as u16).collect()
                    };
                    let text = self.decode_bytes(op_index, text_bytes);
                    decoded.push_str(&text);
                    all_codes.extend_from_slice(&codes);
                }
                TjRawItem::Spacing(spacing) => {
                    // Word-space heuristic matching extract_text thresholds.
                    // Source: pdf.js evaluator.js.
                    if *spacing < -600.0 {
                        // Large gap — line break. Insert nothing (per-op, not merging).
                    } else if *spacing < -102.0 {
                        decoded.push(' ');
                    }
                    // Kerning (>= -102) — no space.
                }
            }
        }

        (decoded, all_codes)
    }

    fn is_cid_font(&self, op_index: usize) -> bool {
        // We need the current font name. This is approximate —
        // the real font name comes from the text state tracked in collect_runs.
        // For standalone decode, we look at the preceding Tf ops.
        for prev_op in self.ecs.ops[..op_index].iter().rev() {
            if let Some(OpCode::SetFont) = prev_op.op {
                let span_bytes = &self.ecs.bytes[prev_op.span.clone()];
                if let Some((name, _)) = parse_tf_args(span_bytes) {
                    return self
                        .fonts
                        .get(&name)
                        .map(|f| f.font_type().is_cid_font())
                        .unwrap_or(false);
                }
                break;
            }
        }
        false
    }

    fn decode_bytes(&self, op_index: usize, text_bytes: &[u8]) -> String {
        // Find the font for this op by looking at the most recent preceding Tf.
        for prev_op in self.ecs.ops[..op_index].iter().rev() {
            if let Some(OpCode::SetFont) = prev_op.op {
                let span_bytes = &self.ecs.bytes[prev_op.span.clone()];
                if let Some((name, _)) = parse_tf_args(span_bytes) {
                    if let Some(font) = self.fonts.get(&name) {
                        return super::content_stream::decode_text_with_font(
                            font, text_bytes,
                        );
                    }
                }
                break;
            }
        }
        // No font found — use UTF-8 lossy fallback.
        String::from_utf8_lossy(text_bytes).into_owned()
    }
}

// ── Raw TJ array items (for parsing) ──────────────────────────────────

enum TjRawItem {
    Text(Vec<u8>),
    Spacing(f64),
}

/// Extract the single string arg bytes from a Tj op's raw span.
/// E.g. `(Hello) Tj\n` → Some(b"Hello"), `<48656c6c6f> Tj\n` → Some(b"48656c6c6f").
fn extract_single_string_arg(span_bytes: &[u8]) -> Option<Vec<u8>> {
    // Strip the trailing command (" Tj\n").
    // Find the last whitespace before the command.
    let span_str = std::str::from_utf8(span_bytes).ok()?;
    // The last token before the command is the string operand.
    // It's either (...) or <...>.
    if let Some(parens) = span_str.rfind('(') {
        // Find matching close paren.
        let open = parens;
        let mut depth = 1u32;
        let mut i = open + 1;
        while i < span_str.len() && depth > 0 {
            match span_str.as_bytes()[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        // Return the raw bytes between parens (unescaped).
                        // For decode_text_with_font we need the raw char codes,
                        // not the literal string bytes. But for literal strings,
                        // the bytes ARE the char codes (after escape processing).
                        // TODO: handle \n, \r, \\n, \NNN octal escapes.
                        // For now, do minimal escape processing.
                        return Some(unescape_pdf_string(
                            &span_str.as_bytes()[open + 1..i],
                        ));
                    }
                }
                b'\\' => {
                    i += 1; // skip next char
                }
                _ => {}
            }
            i += 1;
        }
    }

    if let Some(angled) = span_str.rfind('<') {
        // Hex string: <48656c6c6f>.
        let end = span_str[angled..].find('>').unwrap_or(span_str.len());
        let hex_str = &span_str[angled + 1..angled + end];
        let bytes = hex_decode(hex_str);
        if !bytes.is_empty() {
            return Some(bytes);
        }
    }

    None
}

/// Extract TJ array items: `[(Hello) -120 (World)] TJ\n`.
fn extract_tj_array_items(span_bytes: &[u8]) -> Vec<TjRawItem> {
    let mut items = Vec::new();
    let span_str = match std::str::from_utf8(span_bytes) {
        Ok(s) => s,
        Err(_) => return items,
    };

    // Find the array brackets.
    let arr_start = match span_str.find('[') {
        Some(i) => i + 1,
        None => return items,
    };
    let arr_end = match span_str[arr_start..].find(']') {
        Some(i) => arr_start + i,
        None => return items,
    };

    let arr_content = &span_str[arr_start..arr_end];
    let mut i = 0;
    let bytes = arr_content.as_bytes();

    while i < bytes.len() {
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        if bytes[i] == b'(' {
            // Literal string.
            let open = i;
            let mut depth = 1u32;
            i += 1;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            let text_bytes = unescape_pdf_string(&bytes[open + 1..i]);
                            items.push(TjRawItem::Text(text_bytes));
                        }
                    }
                    b'\\' => i += 1,
                    _ => {}
                }
                i += 1;
            }
        } else if bytes[i] == b'<' {
            // Hex string.
            let open = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            let hex_str = &arr_content[open + 1..i];
            let decoded = hex_decode(hex_str);
            if !decoded.is_empty() {
                items.push(TjRawItem::Text(decoded));
            }
        } else if bytes[i].is_ascii_digit()
            || bytes[i] == b'-'
            || bytes[i] == b'+'
            || bytes[i] == b'.'
        {
            // Number (spacing).
            let num_start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || bytes[i] == b'.'
                    || bytes[i] == b'-'
                    || bytes[i] == b'+')
            {
                i += 1;
            }
            let num_str = &arr_content[num_start..i];
            if let Ok(n) = num_str.parse::<f64>() {
                items.push(TjRawItem::Spacing(n));
            }
        } else {
            i += 1;
        }
    }

    items
}

/// Parse `/Name size` from a Tf op's raw bytes (e.g. `/F1 12 Tf\n`).
pub fn parse_tf_args(span_bytes: &[u8]) -> Option<(String, f64)> {
    let s = std::str::from_utf8(span_bytes).ok()?;
    // Format: "/Name size Tf" or "/Name size Tf\n"
    let trimmed = s.trim_end();
    // Remove trailing " Tf".
    let without_cmd = trimmed.strip_suffix(" Tf")?;
    let without_cmd = without_cmd.trim_end();
    // Split on whitespace: first token is /Name, last is size.
    let mut parts = without_cmd.split_whitespace();
    let name = parts.next()?.strip_prefix('/')?.to_string();
    let size: f64 = parts.last()?.parse().ok()?;
    Some((name, size))
}

/// Parse 6 numbers for Tm/cm from raw op bytes.
fn parse_matrix_args(span_bytes: &[u8]) -> Option<[f64; 6]> {
    let s = std::str::from_utf8(span_bytes).ok()?;
    let trimmed = s.trim();
    // Remove trailing command (" Tm\n" or " cm\n").
    let without_cmd = trimmed
        .rsplit_once(char::is_whitespace)
        .map(|(rest, _)| rest)
        .unwrap_or(trimmed);
    let nums: Vec<f64> = without_cmd
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    if nums.len() >= 6 {
        Some([nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]])
    } else {
        None
    }
}

/// Parse 2 numbers for Td/TD from raw op bytes.
fn parse_two_numbers(span_bytes: &[u8]) -> Option<(f64, f64)> {
    let s = std::str::from_utf8(span_bytes).ok()?;
    let trimmed = s.trim();
    let without_cmd = trimmed
        .rsplit_once(char::is_whitespace)
        .map(|(rest, _)| rest)
        .unwrap_or(trimmed);
    let nums: Vec<f64> = without_cmd
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    if nums.len() >= 2 {
        Some((nums[0], nums[1]))
    } else {
        None
    }
}

/// Minimal PDF string unescape: handles `\n`, `\r`, `\\`, `\(`, `\)`, `\NNN` (octal).
fn unescape_pdf_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                }
                b'(' | b')' | b'\\' => {
                    out.push(bytes[i + 1]);
                    i += 2;
                }
                b'0'..=b'7' => {
                    // Octal: up to 3 digits.
                    let mut val = 0u32;
                    for _ in 0..3 {
                        if i + 1 < bytes.len() && (b'0'..=b'7').contains(&bytes[i + 1]) {
                            val = val * 8 + (bytes[i + 1] - b'0') as u32;
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    out.push(val as u8);
                    i += 1;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Decode a hex string (without angle brackets).
fn hex_decode(hex: &str) -> Vec<u8> {
    let hex: Vec<u8> = hex
        .as_bytes()
        .iter()
        .filter(|b| !b.is_ascii_whitespace())
        .copied()
        .collect();
    let hex = if hex.len() % 2 != 0 {
        // Odd-length hex strings are padded with a leading 0 (PDF spec 7.3.4.1).
        let mut padded = Vec::with_capacity(hex.len() + 1);
        padded.push(b'0');
        padded.extend_from_slice(&hex);
        padded
    } else {
        hex
    };
    let mut out = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.chunks(2) {
        let hi = hex_val(chunk[0]);
        let lo = hex_val(chunk[1]);
        out.push(hi << 4 | lo);
    }
    out
}

// ── Operand serializers ──────────────────────────────────────────────

/// Serialize a byte sequence as a PDF hex string: `<48656c6c6f>`.
///
/// Hex strings are lexer-round-trip-safe (no escaping needed) and
/// `decode_text` handles them identically to literal strings.
pub fn serialize_hex_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + bytes.len() * 2 + 1);
    out.push(b'<');
    for &b in bytes {
        out.push(hex_nibble(b >> 4));
        out.push(hex_nibble(b & 0x0F));
    }
    out.push(b'>');
    out
}

/// Serialize a number as PDF bytes.
///
/// Integers use the shortest decimal form; floats use fixed-point.
pub fn serialize_number(n: f64) -> Vec<u8> {
    // Check if it's an integer value.
    if n.fract() == 0.0 && n.abs() < 2_147_483_648.0 {
        // Format as integer.
        format!("{}", n as i64).into_bytes()
    } else {
        format!("{}", n).into_bytes()
    }
}

/// Items in a TJ array operand.
#[derive(Debug, Clone)]
pub enum TjItem {
    /// A text string (will be serialized as hex string).
    Text(Vec<u8>),
    /// A spacing adjustment in 1/1000ths of a text space unit.
    Spacing(f64),
}

/// Serialize a TJ array: `[<hex> -120 <hex> 50 <hex>]`
pub fn serialize_tj_array(items: &[TjItem]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + items.len() * 10);
    out.push(b'[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(b' ');
        }
        match item {
            TjItem::Text(bytes) => out.extend_from_slice(&serialize_hex_string(bytes)),
            TjItem::Spacing(n) => {
                let s = serialize_number(*n);
                out.extend_from_slice(&s);
            }
        }
    }
    out.push(b']');
    out
}

/// Serialize a complete operation: `operand_bytes SPACE command\n`.
pub fn serialize_operation(op: OpCode, operand_bytes: &[u8]) -> Vec<u8> {
    let cmd = op.to_command();
    let mut out = Vec::with_capacity(operand_bytes.len() + 1 + cmd.len() + 1);
    out.extend_from_slice(operand_bytes);
    out.push(b' ');
    out.extend_from_slice(cmd.as_bytes());
    out.push(b'\n');
    out
}

/// Convert a hex character (ASCII) to its 4-bit value.
fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

fn hex_nibble(n: u8) -> u8 {
    if n < 10 {
        b'0' + n
    } else {
        b'a' + n - 10
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fragment(decoded: &str) -> (Vec<u8>, ContentFragment) {
        let bytes = decoded.as_bytes().to_vec();
        let len = bytes.len();
        (
            bytes,
            ContentFragment {
                filter_name: None,
                has_decode_parms: false,
                raw: Vec::new(),
                decoded_offset: 0,
                decoded_len: len,
            },
        )
    }

    fn make_fragment_with_offset(
        decoded: &str,
        offset: usize,
    ) -> (Vec<u8>, ContentFragment) {
        let bytes = decoded.as_bytes().to_vec();
        let len = bytes.len();
        (
            bytes,
            ContentFragment {
                filter_name: None,
                has_decode_parms: false,
                raw: Vec::new(),
                decoded_offset: offset,
                decoded_len: len,
            },
        )
    }

    #[test]
    fn identity_simple() {
        let content = "BT /F1 12 Tf 100 700 Td (Hello) Tj ET";
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        assert_eq!(ecs.bytes(), content.as_bytes());
    }

    #[test]
    fn identity_with_qq() {
        let content = "q 1 0 0 1 50 50 cm BT /F1 12 Tf (Hello) Tj ET Q";
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        assert_eq!(ecs.bytes(), content.as_bytes());
    }

    #[test]
    fn identity_multiline() {
        let content = "BT\n/F1 12 Tf\n100 700 Td\n(Hello World) Tj\nET";
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        assert_eq!(ecs.bytes(), content.as_bytes());
    }

    #[test]
    fn identity_hex_string() {
        let content = "BT /F1 12 Tf <48656c6c6f> Tj ET";
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        assert_eq!(ecs.bytes(), content.as_bytes());
    }

    #[test]
    fn identity_tj_array() {
        let content = "BT /F1 12 Tf [(H) -120 (ello)] TJ ET";
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        assert_eq!(ecs.bytes(), content.as_bytes());
    }

    #[test]
    fn identity_colors_and_paths() {
        let content = "0.5 0.5 0.5 rg 100 200 m 150 250 l S";
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        assert_eq!(ecs.bytes(), content.as_bytes());
    }

    #[test]
    fn identity_empty() {
        let content = "";
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        assert_eq!(ecs.bytes(), content.as_bytes());
        assert!(ecs.ops.is_empty());
    }

    #[test]
    fn op_count_matches_evaluator() {
        let content = "BT /F1 12 Tf 100 700 Td (Hello) Tj ET q Q";
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        // BT, Tf, Td, Tj, ET, q, Q = 7 operations
        assert_eq!(ecs.ops.len(), 7);
        assert_eq!(ecs.ops[0].op, Some(OpCode::BeginText));
        assert_eq!(ecs.ops[1].op, Some(OpCode::SetFont));
        assert_eq!(ecs.ops[2].op, Some(OpCode::MoveText));
        assert_eq!(ecs.ops[3].op, Some(OpCode::ShowText));
        assert_eq!(ecs.ops[4].op, Some(OpCode::EndText));
        assert_eq!(ecs.ops[5].op, Some(OpCode::Save));
        assert_eq!(ecs.ops[6].op, Some(OpCode::Restore));
    }

    #[test]
    fn splice_preserves_neighbors() {
        let content = "BT /F1 12 Tf (Hello) Tj (World) Tj ET";
        let (bytes, frag) = make_fragment(content);
        let mut ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();

        // Replace the first Tj operation (index 2, "(Hello) Tj") with a hex string.
        let replacement = serialize_operation(OpCode::ShowText, &serialize_hex_string(b"Hi"));
        ecs.replace_op_bytes(2, replacement);

        // Before: BT /F1 12 Tf (Hello) Tj (World) Tj ET
        // After:  BT /F1 12 Tf <4869> Tj (World) Tj ET
        let result = String::from_utf8_lossy(ecs.bytes()).to_string();
        assert!(result.contains("<4869> Tj"));
        assert!(result.contains("(World) Tj"));
        assert!(result.starts_with("BT"));
        assert!(result.ends_with("ET"));
    }

    #[test]
    fn splice_shifts_subsequent_spans() {
        let content = "(A) Tj (B) Tj (C) Tj";
        let (bytes, frag) = make_fragment(content);
        let mut ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        assert_eq!(ecs.ops.len(), 3);

        // Replace middle Tj with a longer string.
        let replacement =
            serialize_operation(OpCode::ShowText, &serialize_hex_string(b"LongerText"));
        let old_third_span = ecs.ops[2].span.clone();
        ecs.replace_op_bytes(1, replacement);

        // Third op should have shifted.
        assert_ne!(ecs.ops[2].span, old_third_span);
        // But it should still reference the correct bytes.
        let third_bytes = &ecs.bytes()[ecs.ops[2].span.clone()];
        let third_str = String::from_utf8_lossy(third_bytes).to_string();
        assert!(third_str.contains("(C) Tj"));

        // Fragment bounds should match the (now longer) byte buffer.
        let frag = &ecs.fragments[0];
        assert_eq!(frag.decoded_offset, 0);
        assert_eq!(frag.decoded_len, ecs.bytes().len());
    }

    #[test]
    fn serialize_hex_string_roundtrip() {
        let input = b"Hello";
        let hex = serialize_hex_string(input);
        assert_eq!(&hex, b"<48656c6c6f>");
    }

    #[test]
    fn serialize_number_integer() {
        assert_eq!(serialize_number(12.0), b"12".to_vec());
        assert_eq!(serialize_number(0.0), b"0".to_vec());
        assert_eq!(serialize_number(-5.0), b"-5".to_vec());
    }

    #[test]
    fn serialize_number_float() {
        let result = serialize_number(1.5);
        assert!(result.starts_with(b"1.5"));
    }

    #[test]
    fn serialize_tj_array_basic() {
        let items = vec![
            TjItem::Text(b"Hello".to_vec()),
            TjItem::Spacing(-120.0),
            TjItem::Text(b"World".to_vec()),
        ];
        let result = serialize_tj_array(&items);
        let s = String::from_utf8_lossy(&result).to_string();
        assert!(s.starts_with('['));
        assert!(s.ends_with(']'));
        assert!(s.contains("<48656c6c6f>"));
        assert!(s.contains("-120"));
        assert!(s.contains("<576f726c64>"));
    }

    #[test]
    fn serialize_operation_basic() {
        let result = serialize_operation(OpCode::ShowText, &serialize_hex_string(b"Hi"));
        let s = String::from_utf8_lossy(&result).to_string();
        assert_eq!(s, "<4869> Tj\n");
    }

    #[test]
    fn multi_fragment_identity() {
        let f1 = make_fragment_with_offset("q 1 0 0 1 0 0 cm ", 0);
        let f2 = make_fragment_with_offset("BT /F1 12 Tf (X) Tj ET ", 20);
        let f3 = make_fragment_with_offset("Q", 44);
        let ecs = EditableContentStream::build(&[f1, f2, f3]).unwrap();

        // Should have ops from all fragments.
        assert!(ecs.ops.len() > 3);
        // First op should be in fragment 0, text ops in fragment 1.
        assert_eq!(ecs.ops[0].stream_index, 0);
        // Find the Tj op — should be in fragment 1.
        let tj = ecs.ops.iter().find(|o| o.op == Some(OpCode::ShowText)).unwrap();
        assert_eq!(tj.stream_index, 1);
    }

    // ── TextRunLocator tests ──────────────────────────────────────────

    use rustc_hash::FxHashMap;

    fn with_locator(content: &str, f: impl FnOnce(&TextRunLocator)) {
        let (bytes, frag) = make_fragment(content);
        let ecs = EditableContentStream::build(&[(bytes, frag)]).unwrap();
        let fonts: FxHashMap<String, crate::core::font::Font> = FxHashMap::default();
        let locator = TextRunLocator::new(&ecs, &fonts);
        f(&locator);
    }

    #[test]
    fn locator_finds_tj_text() {
        with_locator("BT /F1 12 Tf (Hello) Tj ET", |locator| {
            let runs = locator.collect_runs();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].decoded, "Hello");
            assert_eq!(runs[0].kind, TextShowKind::Tj);
            assert_eq!(runs[0].font_name.as_deref(), Some("F1"));
            assert_eq!(runs[0].font_size, 12.0);
        });
    }

    #[test]
    fn locator_finds_hex_string_tj() {
        with_locator("BT /F1 12 Tf <48656c6c6f> Tj ET", |locator| {
            let runs = locator.collect_runs();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].decoded, "Hello"); // hex decoded
        });
    }

    #[test]
    fn locator_finds_tj_array() {
        with_locator("BT /F1 12 Tf [(H) -120 (ello)] TJ ET", |locator| {
            let runs = locator.collect_runs();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].decoded, "H ello"); // spacing -120 → space after H
            assert_eq!(runs[0].kind, TextShowKind::TJ);
        });
    }

    #[test]
    fn locator_tracks_text_matrix() {
        with_locator(
            "BT /F1 12 Tf 100 700 Td (A) Tj 200 700 Td (B) Tj ET",
            |locator| {
                let runs = locator.collect_runs();
                assert_eq!(runs.len(), 2);
                // First Tj at Td(100,700): Tx=100, Ty=700.
                assert_eq!(runs[0].text_matrix[4], 100.0);
                assert_eq!(runs[0].text_matrix[5], 700.0);
                // Second Tj: Td(200,700) is relative, so Tx=100+200=300, Ty=700+700=1400.
                assert_eq!(runs[1].text_matrix[4], 300.0);
                assert_eq!(runs[1].text_matrix[5], 1400.0);
            },
        );
    }

    #[test]
    fn find_text_simple() {
        with_locator("BT /F1 12 Tf (Hello World) Tj ET", |locator| {
            let m = locator.find_text("World", &FindOptions::default()).unwrap();
            assert_eq!(m.op_index, 2); // Tj is op 2
            assert_eq!(m.matched_text, "World");
        });
    }

    #[test]
    fn find_text_no_match() {
        with_locator("BT /F1 12 Tf (Hello) Tj ET", |locator| {
            assert!(locator.find_text("XYZ", &FindOptions::default()).is_none());
        });
    }

    #[test]
    fn find_text_hex_string() {
        with_locator("BT /F1 12 Tf <48656c6c6f> Tj ET", |locator| {
            let m = locator.find_text("Hello", &FindOptions::default()).unwrap();
            assert_eq!(m.matched_text, "Hello");
        });
    }

    #[test]
    fn find_text_second_occurrence() {
        with_locator("BT /F1 12 Tf (Hello) Tj (Hello) Tj ET", |locator| {
            let m = locator
                .find_text(
                    "Hello",
                    &FindOptions {
                        occurrence: 1,
                        ..Default::default()
                    },
                )
                .unwrap();
            // Second Hello is in the second Tj op (index 3).
            assert_eq!(m.op_index, 3);
        });
    }

    // ── Hex/unescape tests ────────────────────────────────────────────

    #[test]
    fn hex_decode_basic() {
        assert_eq!(hex_decode("48656c6c6f"), b"Hello");
    }

    #[test]
    fn hex_decode_odd_length() {
        assert_eq!(hex_decode("A"), b"\x0A"); // padded with 0
    }

    #[test]
    fn unescape_pdf_string_basic() {
        assert_eq!(unescape_pdf_string(b"Hello"), b"Hello");
    }

    #[test]
    fn unescape_pdf_string_escapes() {
        assert_eq!(unescape_pdf_string(b"Hello\\World"), b"Hello\\World");
        assert_eq!(unescape_pdf_string(b"Hello\\nWorld"), b"Hello\nWorld");
        assert_eq!(unescape_pdf_string(b"Hello\\rWorld"), b"Hello\rWorld");
        assert_eq!(unescape_pdf_string(b"(parens)"), b"(parens)");
        assert_eq!(unescape_pdf_string(b"(nested (inner))"), b"(nested (inner))");
    }

    #[test]
    fn unescape_pdf_string_octal() {
        assert_eq!(unescape_pdf_string(b"\\101"), b"A");
        assert_eq!(unescape_pdf_string(b"\\012"), b"\n");
        assert_eq!(unescape_pdf_string(b"\\377"), b"\xff");
    }

    #[test]
    fn parse_tf_args_basic() {
        assert_eq!(
            parse_tf_args(b"/F1 12 Tf\n"),
            Some(("F1".to_string(), 12.0))
        );
    }

    #[test]
    fn parse_tf_args_decimal() {
        assert_eq!(
            parse_tf_args(b"/F1 10.5 Tf\n"),
            Some(("F1".to_string(), 10.5))
        );
    }

    #[test]
    fn parse_matrix_args_tm() {
        let m = parse_matrix_args(b"1 0 0 1 100 700 Tm\n").unwrap();
        assert_eq!(m[4], 100.0);
        assert_eq!(m[5], 700.0);
    }

    #[test]
    fn parse_two_numbers_td() {
        assert_eq!(parse_two_numbers(b"100 700 Td\n"), Some((100.0, 700.0)));
    }
}
