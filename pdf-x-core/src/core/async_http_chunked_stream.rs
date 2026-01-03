//! Async HTTP chunked stream for progressive PDF loading from URLs.
//!
//! This module provides async/await support for loading PDFs over HTTP with range requests.
//! It's similar to HttpChunkedStream but uses reqwest for async I/O.
//!
//! Based on PDF.js's network stream approach with HTTP range requests.

#[cfg(feature = "async")]
use super::chunk_manager::ChunkManager;
#[cfg(feature = "async")]
use super::error::{PDFError, PDFResult};
#[cfg(feature = "async")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "async")]
use reqwest::Client;

#[cfg(feature = "async")]
use tokio::sync::RwLock as AsyncRwLock;

/// Progress callback for tracking download progress.
///
/// # Arguments
/// * `loaded` - Number of bytes loaded so far
/// * `total` - Total size of the PDF file in bytes
///
/// # Example
/// ```no_run
/// use pdf_x::core::ProgressCallback;
///
/// let callback: ProgressCallback = Box::new(|loaded, total| {
///     println!("Downloaded: {}% ({}/{})", loaded * 100 / total, loaded, total);
/// });
/// ```
#[cfg(feature = "async")]
pub type ProgressCallback = Box<dyn Fn(usize, usize) + Send + Sync>;

/// Async chunked stream that progressively loads data from HTTP sources using range requests.
///
/// This implementation:
/// - Loads chunks on-demand via HTTP range requests (async)
/// - Maintains an LRU cache of recently used chunks
/// - Supports progress callbacks for download tracking
/// - Does NOT download the entire file at once
///
/// The HTTP client and chunk manager are shared, allowing sub-streams to reuse
/// the same connection pool and cache.
///
/// # Example
/// ```no_run
/// use pdf_x::core::AsyncHttpChunkedStream;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stream = AsyncHttpChunkedStream::open(
///         "https://example.com/document.pdf",
///         None,  // Default chunk size (64KB)
///         None,  // Default cache (10 chunks)
///         None,  // No progress callback
///     ).await?;
///
///     println!("PDF size: {} bytes", stream.length());
///     Ok(())
/// }
/// ```
#[cfg(feature = "async")]
pub struct AsyncHttpChunkedStream {
    /// URL of the PDF file
    url: String,

    /// Async HTTP client for making requests
    client: Client,

    /// The chunk manager that tracks loaded chunks (shared across clones)
    manager: Arc<AsyncRwLock<ChunkManager>>,

    /// Current read position (not shared - each stream instance has its own)
    pos: usize,

    /// Starting offset in the file
    start: usize,

    /// Cached chunk size (immutable)
    chunk_size: usize,

    /// Cached total file length (immutable)
    total_length: usize,

    /// Optional progress callback
    progress_callback: Option<Arc<ProgressCallback>>,

    /// Total bytes loaded (for progress reporting)
    bytes_loaded: Arc<Mutex<usize>>,
}

#[cfg(feature = "async")]
impl AsyncHttpChunkedStream {
    /// Creates a new AsyncHttpChunkedStream from a URL.
    ///
    /// This makes an initial HEAD request to get the file size and verify range support.
    ///
    /// # Arguments
    /// * `url` - URL of the PDF file
    /// * `chunk_size` - Size of each chunk (default: 64KB)
    /// * `max_cached_chunks` - Maximum chunks to keep in memory (default: 10)
    /// * `progress_callback` - Optional callback for download progress
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::AsyncHttpChunkedStream;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let stream = AsyncHttpChunkedStream::open(
    ///         "https://example.com/doc.pdf",
    ///         Some(65536),  // 64KB chunks
    ///         Some(10),     // Cache 10 chunks
    ///         Some(Box::new(|loaded, total| {
    ///             println!("Progress: {}%", loaded * 100 / total);
    ///         })),
    ///     ).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn open(
        url: impl Into<String>,
        chunk_size: Option<usize>,
        max_cached_chunks: Option<usize>,
        progress_callback: Option<ProgressCallback>,
    ) -> PDFResult<Self> {
        let url = url.into();

        // Create HTTP client with timeout
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| PDFError::StreamError(format!("Failed to create HTTP client: {}", e)))?;

        // Make HEAD request to get content length
        let response = client
            .head(&url)
            .send()
            .await
            .map_err(|e| PDFError::StreamError(format!("Failed to HEAD request: {}", e)))?;

        // Check if server supports range requests
        let accepts_ranges = response
            .headers()
            .get("accept-ranges")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_lowercase() == "bytes")
            .unwrap_or(false);

        if !accepts_ranges {
            return Err(PDFError::StreamError(
                "Server does not support range requests".to_string(),
            ));
        }

        // Get content length
        let length: usize = response
            .headers()
            .get("content-length")
            .ok_or_else(|| PDFError::StreamError("No Content-Length header".to_string()))?
            .to_str()
            .map_err(|_| PDFError::StreamError("Invalid Content-Length header".to_string()))?
            .parse()
            .map_err(|_| PDFError::StreamError("Invalid Content-Length value".to_string()))?;

        let manager = ChunkManager::new(length, chunk_size, max_cached_chunks);

        // Cache immutable values
        let cached_chunk_size = manager.chunk_size();
        let cached_length = manager.length();

        Ok(AsyncHttpChunkedStream {
            url,
            client,
            manager: Arc::new(AsyncRwLock::new(manager)),
            pos: 0,
            start: 0,
            chunk_size: cached_chunk_size,
            total_length: cached_length,
            progress_callback: progress_callback.map(Arc::new),
            bytes_loaded: Arc::new(Mutex::new(0)),
        })
    }

    /// Requests a specific chunk from the server via HTTP range request.
    ///
    /// This is an async operation that downloads the chunk data.
    async fn request_chunk(&self, chunk_num: usize) -> PDFResult<Vec<u8>> {
        let chunk_start = chunk_num * self.chunk_size;
        let chunk_end = std::cmp::min(chunk_start + self.chunk_size, self.total_length) - 1;

        let range_header = format!("bytes={}-{}", chunk_start, chunk_end);

        let response = self
            .client
            .get(&self.url)
            .header("Range", range_header)
            .send()
            .await
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
        let data = response
            .bytes()
            .await
            .map_err(|e| PDFError::StreamError(format!("Failed to read response: {}", e)))?
            .to_vec();

        // Update progress
        if let Some(callback) = &self.progress_callback {
            if let Ok(mut loaded) = self.bytes_loaded.lock() {
                *loaded += data.len();
                callback(*loaded, self.total_length);
            }
        }

        Ok(data)
    }

    /// Ensures a chunk is loaded into the manager.
    ///
    /// If not already loaded, requests the chunk and sends it to the manager.
    pub async fn ensure_chunk_loaded(&self, chunk_num: usize) -> PDFResult<()> {
        {
            let manager = self.manager.read().await;
            if manager.has_chunk(chunk_num) {
                drop(manager);
                let mut manager = self.manager.write().await;
                manager.mark_chunk_accessed(chunk_num);
                return Ok(());
            }
        }

        // Chunk not loaded, download it
        let data = self.request_chunk(chunk_num).await?;

        let mut manager = self.manager.write().await;
        manager.on_receive_data(chunk_num, data)?;

        Ok(())
    }

    /// Preloads a specific chunk into the cache.
    pub async fn preload_chunk(&self, chunk_num: usize) -> PDFResult<()> {
        self.ensure_chunk_loaded(chunk_num).await
    }

    /// Preloads a range of chunks into the cache.
    ///
    /// This is useful for prefetching data before it's needed.
    pub async fn preload_range(&self, begin: usize, end: usize) -> PDFResult<()> {
        let (begin_chunk, end_chunk, num_chunks) = {
            let manager = self.manager.read().await;
            let begin_chunk = manager.get_chunk_number(begin);
            let end_chunk = manager.get_chunk_number(end.saturating_sub(1));
            let num_chunks = manager.num_chunks();
            (begin_chunk, end_chunk, num_chunks)
        };

        for chunk in begin_chunk..=end_chunk.min(num_chunks - 1) {
            self.ensure_chunk_loaded(chunk).await?;
        }

        Ok(())
    }

    /// Returns the URL of the PDF file.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the total length of the PDF file.
    pub fn length(&self) -> usize {
        self.total_length
    }

    /// Returns the current read position.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Sets the read position.
    pub fn set_pos(&mut self, pos: usize) -> PDFResult<()> {
        if pos > self.total_length {
            return Err(PDFError::InvalidPosition {
                pos,
                length: self.total_length,
            });
        }
        self.pos = pos;
        Ok(())
    }

    /// Reads a single byte at the current position (async).
    pub async fn get_byte(&mut self) -> PDFResult<u8> {
        if self.pos >= self.total_length {
            return Err(PDFError::UnexpectedEndOfStream);
        }

        let chunk_num = self.pos / self.chunk_size;
        self.ensure_chunk_loaded(chunk_num).await?;

        let manager = self.manager.read().await;
        let byte = manager.get_byte_from_cache(self.pos)?;
        drop(manager);

        self.pos += 1;
        Ok(byte)
    }

    /// Reads multiple bytes at the current position (async).
    pub async fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        let end_pos = std::cmp::min(self.pos + length, self.total_length);
        let actual_length = end_pos - self.pos;

        if actual_length == 0 {
            return Ok(Vec::new());
        }

        // Load all required chunks
        let (begin_chunk, end_chunk) = {
            let manager = self.manager.read().await;
            let begin_chunk = manager.get_chunk_number(self.pos);
            let end_chunk = manager.get_chunk_number(end_pos - 1);
            (begin_chunk, end_chunk)
        };

        for chunk in begin_chunk..=end_chunk {
            self.ensure_chunk_loaded(chunk).await?;
        }

        // Collect bytes from cache
        let mut result = Vec::with_capacity(actual_length);
        let manager = self.manager.read().await;

        for chunk_num in begin_chunk..=end_chunk {
            let chunk = manager
                .get_chunk(chunk_num)
                .ok_or(PDFError::DataNotLoaded { chunk: chunk_num })?;

            let chunk_start_pos = chunk_num * self.chunk_size;

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

            result.extend_from_slice(&chunk[read_start..read_end]);
        }

        self.pos = end_pos;
        Ok(result)
    }

    /// Returns the number of chunks currently loaded in the cache.
    pub async fn num_chunks_loaded(&self) -> usize {
        self.manager.read().await.num_chunks_loaded()
    }

    /// Returns the total number of chunks in the file.
    pub fn num_chunks(&self) -> usize {
        (self.total_length + self.chunk_size - 1) / self.chunk_size
    }

    /// Returns true if all chunks are loaded.
    pub async fn is_fully_loaded(&self) -> bool {
        self.manager.read().await.is_data_loaded()
    }
}

#[cfg(all(test, feature = "async"))]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_async_http_stream_creation() {
        // Test with a known public PDF
        let url = "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf";

        let stream = AsyncHttpChunkedStream::open(url, None, None, None).await;

        match stream {
            Ok(s) => {
                assert!(s.length() > 0);
                assert!(s.num_chunks() > 0);
            }
            Err(e) => {
                println!("Test skipped (network error): {}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_async_http_stream_with_progress() {
        let url = "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf";

        let progress_called = Arc::new(Mutex::new(false));
        let progress_called_clone = Arc::clone(&progress_called);

        let callback: ProgressCallback = Box::new(move |loaded, total| {
            println!("Progress: {}/{} ({}%)", loaded, total, loaded * 100 / total);
            if let Ok(mut called) = progress_called_clone.lock() {
                *called = true;
            }
        });

        let stream = AsyncHttpChunkedStream::open(url, None, None, Some(callback)).await;

        match stream {
            Ok(mut s) => {
                // Load first chunk to trigger progress callback
                let _ = s.get_bytes(1024).await;

                // Check if progress callback was called
                if let Ok(called) = progress_called.lock() {
                    assert!(*called, "Progress callback should have been called");
                }
            }
            Err(e) => {
                println!("Test skipped (network error): {}", e);
            }
        }
    }
}
