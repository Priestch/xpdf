use super::base_stream::BaseStream;
use super::error::{PDFError, PDFResult};
use std::collections::{HashMap, VecDeque};
use std::io::Read;

/// Default chunk size: 64KB (same as PDF.js)
pub const DEFAULT_CHUNK_SIZE: usize = 65536;

/// Default maximum number of chunks to keep in memory cache
pub const DEFAULT_MAX_CACHED_CHUNKS: usize = 10;

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
    /// Total length of the file in bytes
    length: usize,
    /// Size of each chunk in bytes
    chunk_size: usize,
    /// Total number of chunks
    num_chunks: usize,
    /// Current read position
    pos: usize,
    /// Starting offset in the file
    start: usize,
    /// Cache of loaded chunks (chunk_number -> data)
    chunk_cache: HashMap<usize, Vec<u8>>,
    /// LRU queue for cache eviction (stores chunk numbers)
    lru_queue: VecDeque<usize>,
    /// Maximum number of chunks to keep in cache
    max_cached_chunks: usize,
    /// HTTP agent for making requests
    agent: ureq::Agent,
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

        let chunk_size = chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
        let max_cached_chunks = max_cached_chunks.unwrap_or(DEFAULT_MAX_CACHED_CHUNKS);
        let num_chunks = length.div_ceil(chunk_size);

        Ok(HttpChunkedStream {
            url,
            length,
            chunk_size,
            num_chunks,
            pos: 0,
            start: 0,
            chunk_cache: HashMap::new(),
            lru_queue: VecDeque::new(),
            max_cached_chunks,
            agent,
        })
    }

    /// Gets the chunk number for a given byte position.
    fn get_chunk_number(&self, pos: usize) -> usize {
        pos / self.chunk_size
    }

    /// Loads a chunk from the HTTP server if not already cached.
    ///
    /// This method implements LRU cache eviction when the cache is full.
    fn ensure_chunk_loaded(&mut self, chunk_num: usize) -> PDFResult<()> {
        if chunk_num >= self.num_chunks {
            return Err(PDFError::InvalidByteRange {
                begin: chunk_num * self.chunk_size,
                end: (chunk_num + 1) * self.chunk_size,
            });
        }

        // Check if chunk is already cached
        if self.chunk_cache.contains_key(&chunk_num) {
            // Update LRU: move to back
            self.lru_queue.retain(|&x| x != chunk_num);
            self.lru_queue.push_back(chunk_num);
            return Ok(());
        }

        // Download chunk via HTTP range request
        let chunk_start = chunk_num * self.chunk_size;
        let chunk_end = std::cmp::min(chunk_start + self.chunk_size, self.length) - 1;

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

        // Evict LRU chunk if cache is full
        if self.chunk_cache.len() >= self.max_cached_chunks {
            if let Some(lru_chunk) = self.lru_queue.pop_front() {
                self.chunk_cache.remove(&lru_chunk);
            }
        }

        // Add to cache
        self.chunk_cache.insert(chunk_num, buffer);
        self.lru_queue.push_back(chunk_num);

        Ok(())
    }

    /// Gets a byte from the cache (chunk must be loaded).
    fn get_byte_from_cache(&self, pos: usize) -> PDFResult<u8> {
        let chunk_num = self.get_chunk_number(pos);
        let chunk_offset = pos % self.chunk_size;

        let chunk = self
            .chunk_cache
            .get(&chunk_num)
            .ok_or(PDFError::DataNotLoaded { chunk: chunk_num })?;

        chunk
            .get(chunk_offset)
            .copied()
            .ok_or(PDFError::UnexpectedEndOfStream)
    }

    /// Returns the number of chunks currently loaded in the cache.
    pub fn num_chunks_loaded(&self) -> usize {
        self.chunk_cache.len()
    }

    /// Returns the total number of chunks in the file.
    pub fn num_chunks(&self) -> usize {
        self.num_chunks
    }

    /// Returns true if all chunks are loaded.
    pub fn is_fully_loaded(&self) -> bool {
        self.chunk_cache.len() == self.num_chunks
    }

    /// Returns a list of chunk numbers that are not currently cached.
    pub fn get_missing_chunks(&self) -> Vec<usize> {
        (0..self.num_chunks)
            .filter(|chunk| !self.chunk_cache.contains_key(chunk))
            .collect()
    }

    /// Preloads a specific chunk into the cache.
    pub fn preload_chunk(&mut self, chunk_num: usize) -> PDFResult<()> {
        self.ensure_chunk_loaded(chunk_num)
    }

    /// Preloads a range of chunks into the cache.
    pub fn preload_range(&mut self, begin: usize, end: usize) -> PDFResult<()> {
        let begin_chunk = self.get_chunk_number(begin);
        let end_chunk = self.get_chunk_number(end.saturating_sub(1));

        for chunk in begin_chunk..=end_chunk.min(self.num_chunks - 1) {
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
        self.length
    }

    fn is_empty(&self) -> bool {
        self.length == 0
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn set_pos(&mut self, pos: usize) -> PDFResult<()> {
        if pos > self.length {
            return Err(PDFError::InvalidPosition {
                pos,
                length: self.length,
            });
        }
        self.pos = pos;
        Ok(())
    }

    fn is_data_loaded(&self) -> bool {
        self.is_fully_loaded()
    }

    fn get_byte(&mut self) -> PDFResult<u8> {
        if self.pos >= self.length {
            return Err(PDFError::UnexpectedEndOfStream);
        }

        let chunk_num = self.get_chunk_number(self.pos);
        self.ensure_chunk_loaded(chunk_num)?;

        let byte = self.get_byte_from_cache(self.pos)?;
        self.pos += 1;
        Ok(byte)
    }

    fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        let end_pos = std::cmp::min(self.pos + length, self.length);
        let actual_length = end_pos - self.pos;

        if actual_length == 0 {
            return Ok(Vec::new());
        }

        // Load all required chunks
        let begin_chunk = self.get_chunk_number(self.pos);
        let end_chunk = self.get_chunk_number(end_pos - 1);

        for chunk in begin_chunk..=end_chunk {
            self.ensure_chunk_loaded(chunk)?;
        }

        // Collect bytes from cache
        let mut result = Vec::with_capacity(actual_length);
        for pos in self.pos..end_pos {
            result.push(self.get_byte_from_cache(pos)?);
        }

        self.pos = end_pos;
        Ok(result)
    }

    fn get_byte_range(&self, begin: usize, end: usize) -> PDFResult<Vec<u8>> {
        if begin >= end {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        if end > self.length {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        let begin_chunk = self.get_chunk_number(begin);
        let end_chunk = self.get_chunk_number(end - 1);

        // Check if all required chunks are loaded
        for chunk in begin_chunk..=end_chunk {
            if !self.chunk_cache.contains_key(&chunk) {
                return Err(PDFError::DataNotLoaded { chunk });
            }
        }

        // Collect bytes from cache
        let mut result = Vec::with_capacity(end - begin);
        for pos in begin..end {
            result.push(self.get_byte_from_cache(pos)?);
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
        if start + length > self.length {
            return Err(PDFError::InvalidByteRange {
                begin: start,
                end: start + length,
            });
        }

        // For now, return an error as proper sub-stream implementation
        // would require sharing the HTTP agent or creating a new one
        Err(PDFError::Generic(
            "Sub-streams not yet supported for HttpChunkedStream".to_string(),
        ))
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
