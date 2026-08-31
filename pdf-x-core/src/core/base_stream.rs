use super::error::PDFResult;

/// Base trait for all PDF stream types.
///
/// This trait provides a common interface for reading data from various sources
/// (network, filesystem, memory) in a uniform way. It mirrors the architecture
/// of PDF.js's BaseStream class.
///
/// Implementations must provide core reading operations, while this trait provides
/// default implementations for common derived operations like multi-byte reads,
/// peeking, and string conversion.
///
/// The `Send` bound allows streams to be transferred between threads safely.
pub trait BaseStream: Send {
    // ============================================================================
    // Required methods (must be implemented by all stream types)
    // ============================================================================

    /// Returns the total length of the stream in bytes.
    fn length(&self) -> usize;

    /// Returns the last readable position in this stream's own coordinate
    /// system (the exclusive end of readable data).
    ///
    /// Most streams position from 0, where this equals `length()`. A
    /// windowed stream whose positions are absolute offsets into a parent
    /// buffer (e.g. `Stream` with a nonzero `start`) overrides this to
    /// `start + length`. Callers scanning with `get_byte_range`/`pos` must
    /// bound themselves with this value, NOT `length()`.
    fn end_pos(&self) -> usize {
        self.length()
    }

    /// Returns true if the stream contains no data.
    fn is_empty(&self) -> bool;

    /// Returns the current position in the stream.
    fn pos(&self) -> usize;

    /// Sets the current position in the stream.
    fn set_pos(&mut self, pos: usize) -> PDFResult<()>;

    /// Reads and returns a single byte from the stream, advancing the position.
    ///
    /// Returns an error if the end of the stream is reached or data is not available.
    fn get_byte(&mut self) -> PDFResult<u8>;

    /// Reads the specified number of bytes from the stream, advancing the position.
    ///
    /// Returns a vector containing the bytes read. May return an error if
    /// data is not available or stream bounds are exceeded.
    fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>>;

    /// Returns a range of bytes from the stream without changing the current position.
    ///
    /// # Arguments
    /// * `begin` - Starting byte offset (inclusive)
    /// * `end` - Ending byte offset (exclusive)
    fn get_byte_range(&self, begin: usize, end: usize) -> PDFResult<Vec<u8>>;

    /// Resets the stream to its initial state.
    fn reset(&mut self) -> PDFResult<()>;

    /// Moves the start position of the stream.
    fn move_start(&mut self) -> PDFResult<()>;

    /// Creates a sub-stream from this stream.
    ///
    /// # Arguments
    /// * `start` - Starting offset for the sub-stream
    /// * `length` - Length of the sub-stream
    fn make_sub_stream(&self, start: usize, length: usize) -> PDFResult<Box<dyn BaseStream>>;

    // ============================================================================
    // Provided methods with default implementations
    // ============================================================================

    /// Returns true if all data in the stream is loaded.
    ///
    /// Default implementation returns true. Override for streams that support
    /// progressive loading.
    fn is_data_loaded(&self) -> bool {
        true
    }

    /// Ensures that a specific byte range is loaded and available.
    ///
    /// For progressive/chunked streams, this triggers loading of any missing chunks
    /// needed to satisfy the range. For in-memory streams, this is a no-op.
    ///
    /// This is called by retry loops when DataMissing errors occur, following
    /// PDF.js's exception-driven loading pattern.
    ///
    /// # Arguments
    /// * `start` - Starting byte position (inclusive)
    /// * `length` - Number of bytes needed
    ///
    /// # Returns
    /// Ok(()) if the range is now available, or an error if loading fails
    fn ensure_range(&mut self, _start: usize, _length: usize) -> PDFResult<()> {
        // Default implementation: assume all data is available
        // Chunked streams override this to load missing chunks
        Ok(())
    }

    /// Reads a single byte without advancing the position.
    ///
    /// Returns an error if the end of the stream is reached or data is not available.
    fn peek_byte(&mut self) -> PDFResult<u8> {
        let current_pos = self.pos();
        let byte = self.get_byte()?;
        self.set_pos(current_pos)?;
        Ok(byte)
    }

    /// Reads the specified number of bytes without advancing the position.
    fn peek_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        let current_pos = self.pos();
        let bytes = self.get_bytes(length)?;
        self.set_pos(current_pos)?;
        Ok(bytes)
    }

    /// Reads a 16-bit unsigned integer (big-endian) from the stream.
    ///
    /// Returns an error if there are not enough bytes available.
    fn get_uint16(&mut self) -> PDFResult<u16> {
        let b0 = self.get_byte()?;
        let b1 = self.get_byte()?;
        Ok(((b0 as u16) << 8) | (b1 as u16))
    }

    /// Reads a 32-bit signed integer (big-endian) from the stream.
    ///
    /// Returns an error if there are not enough bytes available.
    fn get_int32(&mut self) -> PDFResult<i32> {
        let b0 = self.get_byte()? as i32;
        let b1 = self.get_byte()? as i32;
        let b2 = self.get_byte()? as i32;
        let b3 = self.get_byte()? as i32;
        Ok((b0 << 24) | (b1 << 16) | (b2 << 8) | b3)
    }

    /// Reads `length` bytes and converts them to a UTF-8 string.
    ///
    /// Invalid UTF-8 sequences are replaced with the replacement character.
    fn get_string(&mut self, length: usize) -> PDFResult<String> {
        let bytes = self.get_bytes(length)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Skips `n` bytes in the stream by advancing the position.
    ///
    /// If `n` is 0, skips 1 byte by default.
    fn skip(&mut self, n: usize) -> PDFResult<()> {
        let skip_count = if n == 0 { 1 } else { n };
        self.set_pos(self.pos() + skip_count)
    }

    /// Gets image data from the stream.
    ///
    /// NOTE: This is a synchronous version of PDF.js's async getImageData.
    /// It should only be used for image data that is guaranteed to be fully loaded.
    ///
    /// # Arguments
    /// * `length` - Number of bytes to read
    fn get_image_data(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        self.get_bytes(length)
    }

    /// Synchronous version of getting bytes (replaces async asyncGetBytes).
    ///
    /// Default implementation delegates to get_bytes.
    fn get_bytes_sync(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        self.get_bytes(length)
    }

    /// Returns the base streams if this is a composite stream.
    ///
    /// Returns `None` for simple streams.
    fn get_base_streams(&self) -> Option<Vec<Box<dyn BaseStream>>> {
        None
    }

    /// Returns the original stream if this is a wrapper/filter stream.
    ///
    /// Default implementation returns None, indicating this is the original stream.
    fn get_original_stream(&self) -> Option<&dyn BaseStream> {
        None
    }
}

/// The cursor-free half of [`BaseStream`]: everything a document's *backing data*
/// must provide, with no parse-position semantics (D.3 split).
///
/// [`XRef`] holds its byte data as `Box<dyn DocumentSource>` and never moves a
/// cursor on it — parse cursors live in parser-local sub-stream Lexers and in
/// explicit arguments. This is the contract a shell-side source implements to
/// feed the core (D.4: the native file source moves out of core behind this
/// trait): four methods instead of `BaseStream`'s positional surface.
///
/// Every [`BaseStream`] is a `DocumentSource` via the blanket impl below, so
/// existing sources (memory, chunked file, sub-streams) need no new code.
pub trait DocumentSource: Send {
    /// Total length of the data, in bytes.
    fn length(&self) -> usize;

    /// Return `[begin, end)` if the range is available locally; otherwise
    /// `DataMissing` (progressive sources) or a bounds error. Does not mutate
    /// any state.
    fn get_byte_range(&self, begin: usize, end: usize) -> PDFResult<Vec<u8>>;

    /// Make `[start, start+length)` available locally. No-op for fully-loaded
    /// sources; performs (or requests) I/O for progressive ones.
    fn ensure_range(&mut self, start: usize, length: usize) -> PDFResult<()>;

    /// A parser-local positional view of `[start, start+length)`. The returned
    /// sub-stream owns its own cursor; the source's state is unchanged.
    fn make_sub_stream(&self, start: usize, length: usize) -> PDFResult<Box<dyn BaseStream>>;
}

impl<T: BaseStream> DocumentSource for T {
    fn length(&self) -> usize {
        BaseStream::length(self)
    }

    fn get_byte_range(&self, begin: usize, end: usize) -> PDFResult<Vec<u8>> {
        BaseStream::get_byte_range(self, begin, end)
    }

    fn ensure_range(&mut self, start: usize, length: usize) -> PDFResult<()> {
        BaseStream::ensure_range(self, start, length)
    }

    fn make_sub_stream(&self, start: usize, length: usize) -> PDFResult<Box<dyn BaseStream>> {
        BaseStream::make_sub_stream(self, start, length)
    }
}
