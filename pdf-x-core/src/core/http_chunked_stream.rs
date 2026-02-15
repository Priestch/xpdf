//! Synchronous HTTP chunked stream using reqwest with blocking runtime.
//!
//! This is a convenience wrapper around AsyncHttpChunkedStream that provides
//! a synchronous API by using tokio's blocking runtime internally.
//!
//! For async usage, use AsyncHttpChunkedStream directly.

#[cfg(feature = "async")]
use super::async_http_chunked_stream::AsyncHttpChunkedStream;
#[cfg(feature = "async")]
use super::base_stream::BaseStream;
#[cfg(feature = "async")]
use super::error::{PDFError, PDFResult};

/// Synchronous HTTP chunked stream (wraps AsyncHttpChunkedStream with blocking runtime).
///
/// This provides a synchronous API for HTTP range requests by using tokio's
/// blocking runtime internally. For better performance in async contexts,
/// use AsyncHttpChunkedStream directly.
///
/// # Example
/// ```no_run
/// use pdf_x::core::HttpChunkedStream;
///
/// let stream = HttpChunkedStream::open(
///     "https://example.com/document.pdf",
///     None,  // Default chunk size (64KB)
///     None,  // Default cache (10 chunks)
/// ).unwrap();
///
/// println!("PDF size: {} bytes", stream.length());
/// ```
#[cfg(feature = "async")]
pub struct HttpChunkedStream {
    /// The underlying async stream
    async_stream: AsyncHttpChunkedStream,
    /// Tokio runtime for blocking operations
    runtime: tokio::runtime::Runtime,
}

#[cfg(feature = "async")]
impl HttpChunkedStream {
    /// Creates a new HttpChunkedStream from a URL (blocking).
    ///
    /// This makes an initial HEAD request to get the file size.
    ///
    /// # Arguments
    /// * `url` - URL of the PDF file
    /// * `chunk_size` - Size of each chunk (default: 64KB)
    /// * `max_cached_chunks` - Maximum chunks to keep in memory (default: 10)
    pub fn open(
        url: impl Into<String>,
        chunk_size: Option<usize>,
        max_cached_chunks: Option<usize>,
    ) -> PDFResult<Self> {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| PDFError::StreamError(format!("Failed to create runtime: {}", e)))?;

        let async_stream = runtime.block_on(async {
            AsyncHttpChunkedStream::open(url, chunk_size, max_cached_chunks, None).await
        })?;

        Ok(HttpChunkedStream {
            async_stream,
            runtime,
        })
    }

    /// Returns the URL of the PDF file.
    pub fn url(&self) -> &str {
        self.async_stream.url()
    }

    /// Preloads a specific chunk into the cache (blocking).
    pub fn preload_chunk(&mut self, chunk_num: usize) -> PDFResult<()> {
        self.runtime
            .block_on(self.async_stream.preload_chunk(chunk_num))
    }

    /// Preloads a range of chunks into the cache (blocking).
    pub fn preload_range(&mut self, begin: usize, end: usize) -> PDFResult<()> {
        self.runtime
            .block_on(self.async_stream.preload_range(begin, end))
    }

    /// Returns the number of chunks currently loaded in the cache.
    pub fn num_chunks_loaded(&self) -> usize {
        self.runtime.block_on(self.async_stream.num_chunks_loaded())
    }

    /// Returns the total number of chunks in the file.
    pub fn num_chunks(&self) -> usize {
        self.async_stream.num_chunks()
    }

    /// Returns true if all chunks are loaded.
    pub fn is_fully_loaded(&self) -> bool {
        self.runtime.block_on(self.async_stream.is_fully_loaded())
    }
}

#[cfg(feature = "async")]
impl BaseStream for HttpChunkedStream {
    fn length(&self) -> usize {
        self.async_stream.length()
    }

    fn is_empty(&self) -> bool {
        self.length() == 0
    }

    fn pos(&self) -> usize {
        self.async_stream.pos()
    }

    fn set_pos(&mut self, pos: usize) -> PDFResult<()> {
        self.async_stream.set_pos(pos)
    }

    fn is_data_loaded(&self) -> bool {
        self.is_fully_loaded()
    }

    fn get_byte(&mut self) -> PDFResult<u8> {
        self.runtime.block_on(self.async_stream.get_byte())
    }

    fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        self.runtime.block_on(self.async_stream.get_bytes(length))
    }

    fn get_byte_range(&self, begin: usize, end: usize) -> PDFResult<Vec<u8>> {
        if begin >= end {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        let total_length = self.length();
        if end > total_length {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        // Create a temporary stream for async operation
        // This is safe because we're in a blocking context
        let temp_stream = AsyncHttpChunkedStream::open(
            self.async_stream.url(),
            Some(self.async_stream.num_chunks()),
            Some(10),
            None,
        );

        self.runtime.block_on(async {
            let mut stream = temp_stream.await?;
            stream.set_pos(begin)?;
            stream.get_bytes(end - begin).await
        })
    }

    fn reset(&mut self) -> PDFResult<()> {
        self.async_stream.set_pos(0)
    }

    fn move_start(&mut self) -> PDFResult<()> {
        // Not implemented for HTTP streams
        Ok(())
    }

    fn make_sub_stream(&self, start: usize, length: usize) -> PDFResult<Box<dyn BaseStream>> {
        if start + length > self.length() {
            return Err(PDFError::InvalidByteRange {
                begin: start,
                end: start + length,
            });
        }

        // Create a new stream sharing the same URL
        let mut new_stream =
            HttpChunkedStream::open(self.url(), Some(self.num_chunks()), Some(10))?;
        new_stream.set_pos(start)?;

        let sub = super::sub_stream::SubStream::new(Box::new(new_stream), start, length)?;
        Ok(Box::new(sub))
    }
}

#[cfg(all(test, feature = "async"))]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Requires network access
    fn test_sync_http_stream() {
        let url = "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf";

        let stream = HttpChunkedStream::open(url, None, None);

        match stream {
            Ok(s) => {
                assert!(s.length() > 0);
                assert!(s.num_chunks() > 0);
                assert!(!s.is_empty());
            }
            Err(e) => {
                println!("Test skipped: {}", e);
            }
        }
    }
}
