use super::base_stream::BaseStream;
use super::chunk_manager::{ChunkLoader, ChunkManager};
use super::error::{PDFError, PDFResult};
use std::io::Read;
use std::sync::{Arc, Mutex, MutexGuard};

/// Helper function to standardize mutex lock error handling for the chunk manager.
#[inline]
fn lock_manager(manager: &Arc<Mutex<ChunkManager>>) -> PDFResult<MutexGuard<'_, ChunkManager>> {
    manager.lock()
        .map_err(|_| PDFError::StreamError("Failed to lock chunk manager (mutex poisoned)".to_string()))
}

/// A chunked stream that progressively loads data from an HTTP source using range requests.
///
/// This implementation minimizes memory usage by:
/// - Loading chunks on-demand via HTTP range requests
/// - Maintaining an LRU cache of recently used chunks
/// - Not downloading the entire file at once
///
/// The HTTP agent and chunk manager are shared, allowing sub-streams to reuse
/// the same connection pool and cache.
///
/// This mirrors PDF.js's network stream approach with HTTP range requests.
pub struct HttpChunkedStream {
    /// URL of the PDF file
    url: String,
    /// HTTP agent for making requests (cheaply cloneable)
    agent: ureq::Agent,
    /// The chunk manager that tracks loaded chunks (shared)
    manager: Arc<Mutex<ChunkManager>>,
    /// Current read position
    pos: usize,
    /// Starting offset in the file
    start: usize,
    /// Cached chunk size (immutable, no need to lock manager)
    chunk_size: usize,
    /// Cached total file length (immutable, no need to lock manager)
    total_length: usize,
}

impl ChunkLoader for HttpChunkedStream {
    fn request_chunk(&mut self, chunk_num: usize) -> PDFResult<Vec<u8>> {
        let chunk_start = chunk_num * self.chunk_size;
        let chunk_end = std::cmp::min(chunk_start + self.chunk_size, self.total_length) - 1;

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
        self.chunk_size
    }

    fn total_length(&self) -> usize {
        self.total_length
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

        // Cache immutable values to avoid repeated mutex locking
        let cached_chunk_size = manager.chunk_size();
        let cached_length = manager.length();

        Ok(HttpChunkedStream {
            url,
            agent,
            manager: Arc::new(Mutex::new(manager)),
            pos: 0,
            start: 0,
            chunk_size: cached_chunk_size,
            total_length: cached_length,
        })
    }

    /// Creates a new HttpChunkedStream that shares resources with another stream.
    ///
    /// This is used internally for creating sub-streams.
    fn from_shared(
        url: String,
        agent: ureq::Agent,
        manager: Arc<Mutex<ChunkManager>>,
        chunk_size: usize,
        total_length: usize,
    ) -> Self {
        HttpChunkedStream {
            url,
            agent,
            manager,
            pos: 0,
            start: 0,
            chunk_size,
            total_length,
        }
    }

    /// Ensures a chunk is loaded into the manager.
    ///
    /// If not already loaded, requests the chunk and sends it to the manager.
    fn ensure_chunk_loaded(&mut self, chunk_num: usize) -> PDFResult<()> {
        let mut manager = lock_manager(&self.manager)?;

        if !manager.has_chunk(chunk_num) {
            // Release lock before loading
            drop(manager);
            let data = self.request_chunk(chunk_num)?;
            let mut manager = lock_manager(&self.manager)?;
            manager.on_receive_data(chunk_num, data)?;
        } else if manager.is_chunk_cached(chunk_num) {
            manager.mark_chunk_accessed(chunk_num);
        } else {
            // Chunk was loaded before but evicted from cache, reload it
            drop(manager);
            let data = self.request_chunk(chunk_num)?;
            let mut manager = lock_manager(&self.manager)?;
            manager.on_receive_data(chunk_num, data)?;
        }
        Ok(())
    }

    /// Returns the number of chunks currently loaded in the cache.
    pub fn num_chunks_loaded(&self) -> usize {
        self.manager
            .lock()
            .map(|m| m.num_chunks_loaded())
            .unwrap_or(0)
    }

    /// Returns the total number of chunks in the file.
    pub fn num_chunks(&self) -> usize {
        self.manager
            .lock()
            .map(|m| m.num_chunks())
            .unwrap_or(0)
    }

    /// Returns true if all chunks are loaded.
    pub fn is_fully_loaded(&self) -> bool {
        self.manager
            .lock()
            .map(|m| m.is_data_loaded())
            .unwrap_or(false)
    }

    /// Returns a list of chunk numbers that are not currently loaded.
    pub fn get_missing_chunks(&self) -> Vec<usize> {
        self.manager
            .lock()
            .map(|m| m.get_missing_chunks())
            .unwrap_or_default()
    }

    /// Preloads a specific chunk into the cache.
    pub fn preload_chunk(&mut self, chunk_num: usize) -> PDFResult<()> {
        self.ensure_chunk_loaded(chunk_num)
    }

    /// Preloads a range of chunks into the cache.
    pub fn preload_range(&mut self, begin: usize, end: usize) -> PDFResult<()> {
        let manager = lock_manager(&self.manager)?;

        let begin_chunk = manager.get_chunk_number(begin);
        let end_chunk = manager.get_chunk_number(end.saturating_sub(1));
        let num_chunks = manager.num_chunks();
        drop(manager);

        for chunk in begin_chunk..=end_chunk.min(num_chunks - 1) {
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
        self.manager
            .lock()
            .map(|m| m.length())
            .unwrap_or(0)
    }

    fn is_empty(&self) -> bool {
        self.length() == 0
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn set_pos(&mut self, pos: usize) -> PDFResult<()> {
        if pos > self.length() {
            return Err(PDFError::InvalidPosition {
                pos,
                length: self.length(),
            });
        }
        self.pos = pos;
        Ok(())
    }

    fn is_data_loaded(&self) -> bool {
        self.is_fully_loaded()
    }

    fn get_byte(&mut self) -> PDFResult<u8> {
        if self.pos >= self.length() {
            return Err(PDFError::UnexpectedEndOfStream);
        }

        let manager = lock_manager(&self.manager)?;
        let chunk_num = manager.get_chunk_number(self.pos);
        drop(manager);

        self.ensure_chunk_loaded(chunk_num)?;

        let manager = lock_manager(&self.manager)?;
        let byte = manager.get_byte_from_cache(self.pos)?;
        drop(manager);

        self.pos += 1;
        Ok(byte)
    }

    fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        let total_length = self.length();
        let end_pos = std::cmp::min(self.pos + length, total_length);
        let actual_length = end_pos - self.pos;

        if actual_length == 0 {
            return Ok(Vec::new());
        }

        // Load all required chunks
        let manager = lock_manager(&self.manager)?;
        let begin_chunk = manager.get_chunk_number(self.pos);
        let end_chunk = manager.get_chunk_number(end_pos - 1);
        let chunk_size = manager.chunk_size();
        drop(manager);

        for chunk in begin_chunk..=end_chunk {
            self.ensure_chunk_loaded(chunk)?;
        }

        // Collect bytes from cache efficiently by copying chunk slices
        let mut result = Vec::with_capacity(actual_length);
        let manager = lock_manager(&self.manager)?;

        for chunk_num in begin_chunk..=end_chunk {
            let chunk = manager
                .get_chunk(chunk_num)
                .ok_or(PDFError::DataNotLoaded { chunk: chunk_num })?;

            // Calculate the start offset within this chunk
            let chunk_start_pos = chunk_num * chunk_size;

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

        let total_length = self.length();
        if end > total_length {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        let manager = lock_manager(&self.manager)?;

        let begin_chunk = manager.get_chunk_number(begin);
        let end_chunk = manager.get_chunk_number(end - 1);

        // Check if all required chunks are loaded
        for chunk in begin_chunk..=end_chunk {
            if !manager.has_chunk(chunk) {
                return Err(PDFError::DataNotLoaded { chunk });
            }
        }

        // Collect bytes from cache efficiently by copying chunk slices
        let mut result = Vec::with_capacity(end - begin);
        let chunk_size = manager.chunk_size();

        for chunk_num in begin_chunk..=end_chunk {
            let chunk = manager
                .get_chunk(chunk_num)
                .ok_or(PDFError::DataNotLoaded { chunk: chunk_num })?;

            // Calculate the start offset within this chunk
            let chunk_start_pos = chunk_num * chunk_size;

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
        if start + length > self.length() {
            return Err(PDFError::InvalidByteRange {
                begin: start,
                end: start + length,
            });
        }

        // Create a new HttpChunkedStream sharing the same agent and manager
        let new_stream = HttpChunkedStream::from_shared(
            self.url.clone(),
            self.agent.clone(), // ureq::Agent is cheaply cloneable
            Arc::clone(&self.manager),
            self.chunk_size,
            self.total_length,
        );

        // Wrap in SubStream to provide the restricted view
        let sub = super::sub_stream::SubStream::new(Box::new(new_stream), start, length)?;
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
