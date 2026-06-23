//! Pull-based data protocol for progressive, source-agnostic byte access.
//!
//! This is the **Phase 0 target abstraction** (decision A in `CLAUDE.md`). The core is
//! synchronous and runtime-free: when it needs bytes it does not have, it returns a
//! [`Need::Range`] *value* — not a thrown exception, not a future — and the **shell**
//! fulfills it and re-drives the operation.
//!
//! This module is introduced additively in Step 1. Nothing in the existing
//! `BaseStream` / `ChunkManager` / `DataMissing` path is removed or changed; the viewer
//! keeps working untouched. Later steps migrate base object access onto [`ByteSource`].
//!
//! ## How it fits the architecture
//!
//! - [`Need`] is the protocol unit the core emits (`Range` today; extensible to hint/
//!   object requests later).
//! - [`ByteSource`] is implemented by **shells/adapters**, never by core logic:
//!   [`MemoryByteSource`] (here, for tests/embedded data), plus future file/HTTP sources
//!   in shell crates.
//! - [`drive`] is the shell-side loop glue. It contains no I/O of its own — the actual
//!   loading happens in [`ByteSource::ensure_range`], whose implementation blocks (native)
//!   or is driven by an async shell loop (web/wasm). Because resolution is idempotent and
//!   cached, restarting the operation on each `Need::Range` is safe.

use super::base_stream::BaseStream;
use super::error::{PDFError, PDFResult};

/// A request from the core for bytes it does not yet have locally.
///
/// The shell fulfills the request, then re-drives the operation that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Fetch `[pos .. pos+len)` and make it available, then re-drive the operation.
    Range {
        /// Starting byte offset of the requested range.
        pos: u64,
        /// Number of bytes requested.
        len: usize,
    },
}

impl Need {
    /// Construct a range request.
    pub fn range(pos: u64, len: usize) -> Self {
        Need::Range { pos, len }
    }

    /// The starting byte offset of the request.
    pub fn pos(&self) -> u64 {
        match self {
            Need::Range { pos, .. } => *pos,
        }
    }

    /// The number of bytes requested.
    pub fn len(&self) -> usize {
        match self {
            Need::Range { len, .. } => *len,
        }
    }

    /// Whether zero bytes are requested.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Shell-provided byte access behind the pull-based protocol.
///
/// `get` is a **cheap, non-blocking probe**: it returns the range if it is already
/// available locally, or `Err(Need::Range)` to ask the shell to load it first. `get`
/// MUST NOT block. Blocking/async loading happens in [`ByteSource::ensure_range`]
/// (native: blocks; web/wasm: driven by an async shell loop around the same `op`).
///
/// Positions are `u64` to support large files on 32-bit/WASM targets where `usize`
/// would cap at 4 GiB. The Step 3 bridge casts to/from the existing `usize` positions.
pub trait ByteSource {
    /// Return `[pos, pos+len)` if the *entire* range is available locally;
    /// otherwise `Err(Need::Range { pos, len })`.
    ///
    /// Callers should bound-check against [`ByteSource::len`] before requesting:
    /// a range beyond EOF also yields `Err(Need::Range)`, which [`drive`] converts
    /// into a no-progress error rather than spinning.
    fn get(&mut self, pos: u64, len: usize) -> Result<&[u8], Need>;

    /// Make `[pos, pos+len)` available locally. Cheap/no-op for fully-loaded sources;
    /// performs I/O for progressive ones. Returns an error if loading fails or the
    /// range is beyond EOF.
    fn ensure_range(&mut self, pos: u64, len: usize) -> PDFResult<()>;

    /// Total length of the underlying data, in bytes.
    fn len(&self) -> u64;

    /// Whether the source is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// In-memory [`ByteSource`] over an owned byte buffer.
///
/// Fully available: `get` never returns `Need` for in-bounds ranges, and
/// [`ByteSource::ensure_range`] is a no-op. Used for tests and embedded data, and as
/// the "fully loaded" case that future file/HTTP sources converge on once cached.
#[derive(Debug, Clone)]
pub struct MemoryByteSource {
    data: Vec<u8>,
}

impl MemoryByteSource {
    /// Wrap an owned buffer.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Copy a slice into a new owned source.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            data: bytes.to_vec(),
        }
    }
}

impl ByteSource for MemoryByteSource {
    fn get(&mut self, pos: u64, len: usize) -> Result<&[u8], Need> {
        let start = usize::try_from(pos).map_err(|_| Need::range(pos, len))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| Need::range(pos, len))?;
        if end > self.data.len() {
            return Err(Need::range(pos, len));
        }
        Ok(&self.data[start..end])
    }

    fn ensure_range(&mut self, _pos: u64, _len: usize) -> PDFResult<()> {
        // All in-bounds data is already present.
        Ok(())
    }

    fn len(&self) -> u64 {
        self.data.len() as u64
    }
}

/// `ByteSource` adapter over an existing [`BaseStream`].
///
/// This bridges the legacy exception-driven sources (e.g. `FileChunkedStream`) onto the
/// pull-based protocol: a `DataMissing` from the underlying stream becomes a [`Need`],
/// and [`ByteSource::ensure_range`] delegates to [`BaseStream::ensure_range`]. It is the
/// migration seam that lets existing chunked sources be driven through [`drive`] while
/// the deeper `XRef`-reads-through-`ByteSource` migration (Step 3b) is completed.
pub struct BaseStreamSource<'a> {
    stream: &'a mut dyn BaseStream,
    /// Buffer holding the most recently probed range, so `get` can return `&[u8]`.
    buf: Vec<u8>,
}

impl<'a> BaseStreamSource<'a> {
    /// Wrap a mutable base stream as a [`ByteSource`].
    pub fn new(stream: &'a mut dyn BaseStream) -> Self {
        Self {
            stream,
            buf: Vec::new(),
        }
    }
}

impl ByteSource for BaseStreamSource<'_> {
    fn get(&mut self, pos: u64, len: usize) -> Result<&[u8], Need> {
        let start = usize::try_from(pos).map_err(|_| Need::range(pos, len))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| Need::range(pos, len))?;
        match self.stream.get_byte_range(start, end) {
            Ok(bytes) => {
                self.buf = bytes;
                Ok(&self.buf)
            }
            // Not yet loaded → ask the shell to load it. Other errors (e.g. out of
            // bounds) also surface as Need here, since `get`'s type can't carry a fatal
            // error; `drive`'s no-progress guard converts an unsatisfiable Need into an
            // error rather than spinning.
            Err(_) => Err(Need::range(pos, len)),
        }
    }

    fn ensure_range(&mut self, pos: u64, len: usize) -> PDFResult<()> {
        let pos = usize::try_from(pos).map_err(|_| {
            PDFError::Generic(format!("ensure_range position {pos} overflows usize"))
        })?;
        self.stream.ensure_range(pos, len)
    }

    fn len(&self) -> u64 {
        self.stream.length() as u64
    }
}

/// Error from a single [`drive`] step: either the core needs more data ([`Need`]) or it
/// hit a fatal error ([`PDFError`]).
///
/// Carrying both lets a real resolve step use `?` naturally — a missing range yields
/// `Need`, anything else yields `Fatal` — instead of conflating "need data" with "broke".
#[derive(Debug)]
pub enum StepErr {
    /// The core needs this byte range loaded before it can proceed; the shell fulfills
    /// it and [`drive`] retries.
    Need(Need),
    /// A real error (parse failure, corrupt data, …). [`drive`] propagates it.
    Fatal(PDFError),
}

impl StepErr {
    /// Adapt a throwing op result into a drive step: `DataMissing → Need`, all other
    /// errors → `Fatal`. This is the boundary that lets legacy `PDFResult`-returning
    /// operations participate in the pull-based protocol.
    pub fn adapt<T>(res: PDFResult<T>) -> Result<T, Self> {
        match res {
            Ok(v) => Ok(v),
            Err(PDFError::DataMissing { position, length }) => {
                Err(Self::Need(Need::range(position as u64, length)))
            }
            Err(e) => Err(Self::Fatal(e)),
        }
    }
}

impl From<Need> for StepErr {
    fn from(need: Need) -> Self {
        Self::Need(need)
    }
}

impl From<PDFError> for StepErr {
    fn from(err: PDFError) -> Self {
        Self::Fatal(err)
    }
}

/// Drive a restartable core operation to completion.
///
/// `op` is retried until it returns `Ok`; after each [`StepErr::Need`], the requested
/// range is loaded via [`ByteSource::ensure_range`] and `op` is retried. A
/// [`StepErr::Fatal`] propagates immediately. `op` MUST be idempotent/restartable — it is
/// re-invoked from its start on every retry — which is safe because resolution is cached
/// (re-walking only re-fetches uncached objects).
///
/// A **no-progress guard** prevents infinite loops: if the *same* `Need` is requested
/// twice in a row (meaning `ensure_range` did not make the range available), `drive`
/// returns an error instead of spinning.
///
/// This is the sync loop used by native shells and tests. Web/wasm shells write an
/// equivalent async loop around the same `op` (the core contains no runtime).
pub fn drive<S, T, O>(source: &mut S, mut op: O) -> PDFResult<T>
where
    S: ByteSource,
    O: FnMut(&mut S) -> Result<T, StepErr>,
{
    let mut last: Option<Need> = None;
    loop {
        match op(source) {
            Ok(value) => return Ok(value),
            Err(StepErr::Fatal(err)) => return Err(err),
            Err(StepErr::Need(need)) => {
                if last == Some(need) {
                    return Err(PDFError::Generic(format!(
                        "drive: no progress loading {need:?} (ensure_range did not satisfy the range)"
                    )));
                }
                source.ensure_range(need.pos(), need.len())?;
                last = Some(need);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny, structurally-flavored PDF byte buffer for protocol tests.
    fn sample_pdf() -> Vec<u8> {
        let mut v = b"%PDF-1.7\n".to_vec();
        v.extend_from_slice(b"0 0 obj\n<< /Type /Catalog >>\nendobj\n");
        v.extend_from_slice(b"startxref\n0\n%%EOF");
        v
    }

    #[test]
    fn memory_source_reads_in_bounds() {
        let data = sample_pdf();
        let mut src = MemoryByteSource::from_bytes(&data);
        assert_eq!(src.len() as usize, data.len());
        assert!(!src.is_empty());

        let head = src.get(0, 5).expect("header in bounds");
        assert_eq!(head, b"%PDF-");

        let tail_start = (data.len() - 5) as u64;
        let tail = src.get(tail_start, 5).expect("tail in bounds");
        assert_eq!(tail, b"%%EOF");
    }

    #[test]
    fn memory_source_needs_out_of_bounds() {
        let mut src = MemoryByteSource::from_bytes(b"abc");
        // 3-byte buffer, request 10 → Need (drive would turn this into a no-progress error).
        assert!(matches!(src.get(0, 10), Err(Need::Range { pos: 0, len: 10 })));
        // beyond EOF at a non-zero offset
        assert!(matches!(src.get(2, 5), Err(Need::Range { pos: 2, len: 5 })));
    }

    #[test]
    fn drive_completes_immediately_when_data_available() {
        let mut src = MemoryByteSource::from_bytes(&sample_pdf());
        // op validates the header; with full data it succeeds on the first call.
        let validate_header = |s: &mut MemoryByteSource| -> Result<(), StepErr> {
            let head = s.get(0, 5)?;
            assert_eq!(head, b"%PDF-");
            Ok(())
        };
        drive(&mut src, validate_header).expect("header validates with full data");
    }

    /// Test-only source that simulates progressive loading: no range is available until
    /// `ensure_range` marks it loaded. Mirrors how a real file/http source behaves before
    /// its chunks are fetched.
    struct LazySource {
        data: Vec<u8>,
        loaded: Vec<bool>,
    }

    impl LazySource {
        fn new(data: Vec<u8>) -> Self {
            let n = data.len();
            Self {
                data,
                loaded: vec![false; n],
            }
        }

        fn loaded_byte_count(&self) -> usize {
            self.loaded.iter().filter(|&&b| b).count()
        }
    }

    impl ByteSource for LazySource {
        fn get(&mut self, pos: u64, len: usize) -> Result<&[u8], Need> {
            let start = usize::try_from(pos).map_err(|_| Need::range(pos, len))?;
            let end = start
                .checked_add(len)
                .ok_or_else(|| Need::range(pos, len))?;
            if end > self.data.len() {
                return Err(Need::range(pos, len));
            }
            if !(start..end).all(|i| self.loaded[i]) {
                return Err(Need::range(pos, len));
            }
            Ok(&self.data[start..end])
        }

        fn ensure_range(&mut self, pos: u64, len: usize) -> PDFResult<()> {
            let start = usize::try_from(pos).map_err(|_| {
                PDFError::Generic(format!("ensure_range position {pos} overflows usize"))
            })?;
            let end = start
                .checked_add(len)
                .ok_or_else(|| PDFError::Generic(format!("ensure_range length {len} overflows")))?;
            if end > self.data.len() {
                return Err(PDFError::InvalidByteRange {
                    begin: start,
                    end,
                });
            }
            for i in start..end {
                self.loaded[i] = true;
            }
            Ok(())
        }

        fn len(&self) -> u64 {
            self.data.len() as u64
        }
    }

    #[test]
    fn drive_loads_only_requested_ranges_then_completes() {
        let data = sample_pdf();
        let mut src = LazySource::new(data.clone());

        // A miniature "resolve": validate the header (head range) and confirm `%%EOF`
        // appears in the last 5 bytes (tail range). Both reads return Need until loaded.
        let resolve = |s: &mut LazySource| -> Result<(), StepErr> {
            let head = s.get(0, 5)?;
            if head != b"%PDF-" {
                return Err(Need::range(0, 5).into());
            }
            let tail_start = s.len() - 5;
            let tail = s.get(tail_start, 5)?;
            if tail != b"%%EOF" {
                return Err(Need::range(tail_start, 5).into());
            }
            Ok(())
        };

        drive(&mut src, resolve).expect("drive resolves head and tail");

        // Progressive-minimality property: only the two requested ranges were loaded,
        // not the whole file. This is the differentiator the architecture is built around.
        assert_eq!(src.loaded_byte_count(), 5 + 5);
    }

    #[test]
    fn drive_aborts_on_unsatisfiable_range() {
        // Request a range beyond EOF: ensure_range fails, so the same Need repeats and
        // drive's no-progress guard converts it into an error (no infinite loop).
        let mut src = LazySource::new(b"abc".to_vec());
        let op = |s: &mut LazySource| -> Result<(), StepErr> { s.get(0, 99).map(drop).map_err(StepErr::from) };
        let err = drive(&mut src, op).expect_err("out-of-EOF drive must error");
        let _ = err; // no infinite loop; test passes by terminating
    }

    /// A `ByteSource` wrapper that records every `get` range, so tests can assert exactly
    /// which byte ranges a driven operation read (the minimality property).
    struct RecordingSource<S: ByteSource> {
        inner: S,
        requested: Vec<(u64, usize)>,
    }

    impl<S: ByteSource> RecordingSource<S> {
        fn new(inner: S) -> Self {
            Self {
                inner,
                requested: Vec::new(),
            }
        }
    }

    impl<S: ByteSource> ByteSource for RecordingSource<S> {
        fn get(&mut self, pos: u64, len: usize) -> Result<&[u8], Need> {
            self.requested.push((pos, len));
            self.inner.get(pos, len)
        }
        fn ensure_range(&mut self, pos: u64, len: usize) -> PDFResult<()> {
            self.inner.ensure_range(pos, len)
        }
        fn len(&self) -> u64 {
            self.inner.len()
        }
    }

    /// A minimal but valid classic-xref PDF large enough that "header + tail" is a strict
    /// subset of the file (so minimality is meaningfully assertable). Offsets are computed
    /// at runtime, guaranteeing a well-formed file the parser accepts.
    fn classic_pdf() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n");
        // A valid PDF comment padding the body past 1 KiB, so a standard 1 KiB tail read
        // is a strict subset of the file and the minimality property is meaningfully
        // assertable. Offsets below are computed from `buf.len()`, so they stay correct.
        let mut pad = Vec::with_capacity(1202);
        pad.push(b'%');
        pad.resize(1201, b'.');
        pad.push(b'\n');
        buf.extend_from_slice(&pad);
        let o1 = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let o2 = buf.len();
        buf.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        let o3 = buf.len();
        buf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
        );
        let xref_offset = buf.len();
        buf.extend_from_slice(b"xref\n0 4\n");
        buf.extend_from_slice(format!("{:010} 65535 f \r\n", 0).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o1).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o2).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o3).as_bytes());
        buf.extend_from_slice(b"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n");
        buf.extend_from_slice(format!("{}\n", xref_offset).as_bytes());
        buf.extend_from_slice(b"%%EOF");
        buf
    }

    /// Byte-level PDF reader driven purely through [`ByteSource`]: validate the header and
    /// locate the `startxref` offset from the tail. This is the front of resolving any page,
    /// done with range reads instead of loading the whole file.
    fn locate_xref_offset<S: ByteSource>(s: &mut S) -> Result<u64, StepErr> {
        let total = s.len();
        let header: Vec<u8> = s.get(0, 5)?.to_vec();
        if header != b"%PDF-" {
            return Err(PDFError::corrupted_pdf("missing %PDF- header").into());
        }
        let tail_start = total.saturating_sub(1024);
        let tail_len = (total - tail_start) as usize;
        let tail: Vec<u8> = s.get(tail_start, tail_len)?.to_vec();

        let needle = b"startxref";
        let idx = match tail.windows(needle.len()).rposition(|w| w == needle) {
            Some(i) => i,
            None => return Err(PDFError::corrupted_pdf("no startxref marker").into()),
        };
        let bytes = &tail[idx + needle.len()..];
        let mut p = 0;
        while p < bytes.len() && !bytes[p].is_ascii_digit() {
            p += 1;
        }
        let mut q = p;
        while q < bytes.len() && bytes[q].is_ascii_digit() {
            q += 1;
        }
        let digits = match std::str::from_utf8(&bytes[p..q]) {
            Ok(s) => s,
            Err(_) => return Err(PDFError::corrupted_pdf("startxref offset not utf-8").into()),
        };
        let offset: u64 = match digits.parse() {
            Ok(n) => n,
            Err(_) => return Err(PDFError::corrupted_pdf("startxref offset not a number").into()),
        };
        Ok(offset)
    }

    #[test]
    fn drive_locates_xref_with_minimal_ranges_on_real_pdf() {
        let data = classic_pdf();
        let total = data.len() as u64;
        let mut src = RecordingSource::new(MemoryByteSource::from_bytes(&data));

        let offset = drive(&mut src, locate_xref_offset).expect("xref offset should be located");

        // Correctness: the located offset must actually point at "xref" in the file.
        assert_eq!(&data[offset as usize..offset as usize + 4], b"xref");

        // Minimality / no-whole-file-read: the operation read only the header (5 bytes)
        // and the tail (≤ 1024 bytes), never the middle. For a file this size that is a
        // strict subset — proof the protocol fetches only what it needs.
        assert_eq!(src.requested.len(), 2, "exactly two ranges read (header + tail)");
        assert_eq!(src.requested[0], (0, 5), "first read is the header");
        let (tp, tl) = src.requested[1];
        assert_eq!(tp, total.saturating_sub(1024), "second read is the tail");
        assert_eq!(tp as usize + tl, total as usize, "tail reaches end of file");
        let bytes_read: usize = src.requested.iter().map(|(_, l)| l).sum();
        assert!(
            bytes_read < total as usize,
            "read {bytes_read} of {total} bytes — must be a strict subset (no whole-file read)"
        );
    }

    #[test]
    fn base_stream_source_bridges_a_base_stream() {
        use crate::core::stream::Stream;
        let data = classic_pdf();
        let mut stream = Stream::from_bytes(data.clone());
        let mut src = BaseStreamSource::new(&mut stream);

        // Header read through the bridge.
        let header = src.get(0, 5).expect("bridge reads in-bounds header");
        assert_eq!(header, b"%PDF-");
        assert_eq!(src.len() as usize, data.len());
        // In-memory base stream is fully loaded, so ensure_range is a no-op.
        src.ensure_range(0, 5).expect("ensure_range is a no-op on a loaded base stream");
    }
}
