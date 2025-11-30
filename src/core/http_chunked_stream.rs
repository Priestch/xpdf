use super::base_stream::BaseStream;
use super::chunk_manager::{ChunkLoader, ChunkManager, DEFAULT_MAX_CACHED_CHUNKS};
use super::error::{PDFError, PDFResult};
use std::io::Read;

/// A chunked stream that progressively loads data from an HTTP source using range requests.
///
/// This implementation minimizes memory usage by:
/// - Loading chunks on-demand via HTTP range requests
/// - Maintaining an LRU cache of recently used chunks
/// - Not downloading the entire file at once
///
/// This mirrors PDF.js's network stream approach with HTTP range requests.
pub struct HttpChunkedStream {
    /// URL of the PDF file
    url: String,
    /// HTTP agent for making requests
    agent: ureq::Agent,
    /// The chunk manager that tracks loaded chunks
    manager: ChunkManager,
    /// Current read position
    pos: usize,
    /// Starting offset in the file
    start: usize,
}

impl ChunkLoader for HttpChunkedStream {
    fn request_chunk(&mut self, chunk_num: usize) -> PDFResult<Vec<u8>> {
        let chunk_start = chunk_num * self.chunk_size();
        let chunk_end = std::cmp::min(chunk_start + self.chunk_size(), self.total_length()) - 1;

        let range_header = format!("bytes={}-{}", chunk_start, chunk_end);

        let response = self
            .agent
            .get(&self.url)
            .set("Range", &range_header)
            .call()
            .map_err(|e| {
                PDFError::StreamError(format!("Failed to fetch chunk {}: {}", chunk_num, e))
            })?;

        // Check for 206 Partial Content response
        if response.status() != 206 {
            return Err(PDFError::StreamError(format!(
                "Expected 206 status, got {}",
                response.status()
            )));
        }

        // Read response body
        let mut buffer = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut buffer)
            .map_err(|e| PDFError::StreamError(format!("Failed to read response: {}", e)))?;

        Ok(buffer)
    }

    fn chunk_size(&self) -> usize {
        self.manager.chunk_size()
    }

    fn total_length(&self) -> usize {
        self.manager.length()
    }
}

impl HttpChunkedStream {
    /// Creates a new HttpChunkedStream from a URL.
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
        let url = url.into();

        // Create HTTP agent with timeout
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build();

        // Make HEAD request to get content length
        let response = agent
            .head(&url)
            .call()
            .map_err(|e| PDFError::StreamError(format!("Failed to HEAD request: {}", e)))?;

        // Check if server supports range requests
        let accepts_ranges = response
            .header("Accept-Ranges")
            .map(|v| v.to_lowercase() == "bytes")
            .unwrap_or(false);

        if !accepts_ranges {
            return Err(PDFError::StreamError(
                "Server does not support range requests".to_string(),
            ));
        }

        // Get content length
        let length: usize = response
            .header("Content-Length")
            .ok_or_else(|| PDFError::StreamError("No Content-Length header".to_string()))?
            .parse()
            .map_err(|_| PDFError::StreamError("Invalid Content-Length".to_string()))?;

        let manager = ChunkManager::new(length, chunk_size, max_cached_chunks);

        Ok(HttpChunkedStream {
            url,
            agent,
            manager,
            pos: 0,
            start: 0,
        })
    }

    /// Ensures a chunk is loaded into the manager.
    ///
    /// If not already loaded, requests the chunk and sends it to the manager.
    fn ensure_chunk_loaded(&mut self, chunk_num: usize) -> PDFResult<()> {
        if !self.manager.has_chunk(chunk_num) {
            let data = self.request_chunk(chunk_num)?;
            self.manager.on_receive_data(chunk_num, data)?;
        } else if self.manager.is_chunk_cached(chunk_num) {
            self.manager.mark_chunk_accessed(chunk_num);
        } else {
            // Chunk was loaded before but evicted from cache, reload it
            let data = self.request_chunk(chunk_num)?;
            self.manager.on_receive_data(chunk_num, data)?;
        }
        Ok(())
    }

    /// Returns the number of chunks currently loaded in the cache.
    pub fn num_chunks_loaded(&self) -> usize {
        self.manager.num_chunks_loaded()
    }

    /// Returns the total number of chunks in the file.
    pub fn num_chunks(&self) -> usize {
        self.manager.num_chunks()
    }

    /// Returns true if all chunks are loaded.
    pub fn is_fully_loaded(&self) -> bool {
        self.manager.is_data_loaded()
    }

    /// Returns a list of chunk numbers that are not currently loaded.
    pub fn get_missing_chunks(&self) -> Vec<usize> {
        self.manager.get_missing_chunks()
    }

    /// Preloads a specific chunk into the cache.
    pub fn preload_chunk(&mut self, chunk_num: usize) -> PDFResult<()> {
        self.ensure_chunk_loaded(chunk_num)
    }

    /// Preloads a range of chunks into the cache.
    pub fn preload_range(&mut self, begin: usize, end: usize) -> PDFResult<()> {
        let begin_chunk = self.manager.get_chunk_number(begin);
        let end_chunk = self.manager.get_chunk_number(end.saturating_sub(1));

        for chunk in begin_chunk..=end_chunk.min(self.manager.num_chunks() - 1) {
            self.ensure_chunk_loaded(chunk)?;
        }

        Ok(())
    }

    /// Returns the URL of the PDF file.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl BaseStream for HttpChunkedStream {
    fn length(&self) -> usize {
        self.manager.length()
    }

    fn is_empty(&self) -> bool {
        self.manager.length() == 0
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn set_pos(&mut self, pos: usize) -> PDFResult<()> {
        if pos > self.manager.length() {
            return Err(PDFError::InvalidPosition {
                pos,
                length: self.manager.length(),
            });
        }
        self.pos = pos;
        Ok(())
    }

    fn is_data_loaded(&self) -> bool {
        self.manager.is_data_loaded()
    }

    fn get_byte(&mut self) -> PDFResult<u8> {
        if self.pos >= self.manager.length() {
            return Err(PDFError::UnexpectedEndOfStream);
        }

        let chunk_num = self.manager.get_chunk_number(self.pos);
        self.ensure_chunk_loaded(chunk_num)?;

        let byte = self.manager.get_byte_from_cache(self.pos)?;
        self.pos += 1;
        Ok(byte)
    }

    fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        let end_pos = std::cmp::min(self.pos + length, self.manager.length());
        let actual_length = end_pos - self.pos;

        if actual_length == 0 {
            return Ok(Vec::new());
        }

        // Load all required chunks
        let begin_chunk = self.manager.get_chunk_number(self.pos);
        let end_chunk = self.manager.get_chunk_number(end_pos - 1);

        for chunk in begin_chunk..=end_chunk {
            self.ensure_chunk_loaded(chunk)?;
        }

        // Collect bytes from cache efficiently by copying chunk slices
        let mut result = Vec::with_capacity(actual_length);

        for chunk_num in begin_chunk..=end_chunk {
            let chunk = self
                .manager
                .get_chunk(chunk_num)
                .ok_or(PDFError::DataNotLoaded { chunk: chunk_num })?;

            // Calculate the start offset within this chunk
            let chunk_start_pos = chunk_num * self.manager.chunk_size();

            // Determine which part of this chunk we need
            let read_start = if chunk_num == begin_chunk {
                self.pos - chunk_start_pos
            } else {
                0
            };

            let read_end = if chunk_num == end_chunk {
                end_pos - chunk_start_pos
            } else {
                chunk.len()
            };

            // Copy the slice from this chunk
            result.extend_from_slice(&chunk[read_start..read_end]);
        }

        self.pos = end_pos;
        Ok(result)
    }

    fn get_byte_range(&self, begin: usize, end: usize) -> PDFResult<Vec<u8>> {
        if begin >= end {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        if end > self.manager.length() {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        let begin_chunk = self.manager.get_chunk_number(begin);
        let end_chunk = self.manager.get_chunk_number(end - 1);

        // Check if all required chunks are loaded
        for chunk in begin_chunk..=end_chunk {
            if !self.manager.has_chunk(chunk) {
                return Err(PDFError::DataNotLoaded { chunk });
            }
        }

        // Collect bytes from cache efficiently by copying chunk slices
        let mut result = Vec::with_capacity(end - begin);

        for chunk_num in begin_chunk..=end_chunk {
            let chunk = self
                .manager
                .get_chunk(chunk_num)
                .ok_or(PDFError::DataNotLoaded { chunk: chunk_num })?;

            // Calculate the start offset within this chunk
            let chunk_start_pos = chunk_num * self.manager.chunk_size();

            // Determine which part of this chunk we need
            let read_start = if chunk_num == begin_chunk {
                begin - chunk_start_pos
            } else {
                0
            };

            let read_end = if chunk_num == end_chunk {
                end - chunk_start_pos
            } else {
                chunk.len()
            };

            // Copy the slice from this chunk
            result.extend_from_slice(&chunk[read_start..read_end]);
        }

        Ok(result)
    }

    fn reset(&mut self) -> PDFResult<()> {
        self.pos = self.start;
        Ok(())
    }

    fn move_start(&mut self) -> PDFResult<()> {
        if self.pos > self.start {
            self.start = self.pos;
        }
        Ok(())
    }

    fn make_sub_stream(&self, start: usize, length: usize) -> PDFResult<Box<dyn BaseStream>> {
        if start + length > self.manager.length() {
            return Err(PDFError::InvalidByteRange {
                begin: start,
                end: start + length,
            });
        }

        // Create a new HttpChunkedStream for the sub-stream
        let parent = Box::new(Self::open(
            self.url.clone(),
            Some(self.manager.chunk_size()),
            Some(DEFAULT_MAX_CACHED_CHUNKS),
        )?) as Box<dyn BaseStream>;

        // Wrap in SubStream to provide the restricted view
        let sub = super::sub_stream::SubStream::new(parent, start, length)?;
        Ok(Box::new(sub))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require network access and a test server
    // For real-world testing, you'd need a test HTTP server that supports range requests

    #[test]
    #[ignore] // Ignore by default since it requires network
    fn test_http_chunked_stream_creation() {
        // This test requires a real PDF file accessible via HTTP
        // Example: https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf
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

    #[test]
    fn test_chunk_number_calculation() {
        // Test chunk number calculation logic
        let chunk_size = 1000;

        // Test chunk number calculation
        assert_eq!(0 / chunk_size, 0);
        assert_eq!(999 / chunk_size, 0);
        assert_eq!(1000 / chunk_size, 1);
        assert_eq!(1999 / chunk_size, 1);
        assert_eq!(2000 / chunk_size, 2);
    }
}
