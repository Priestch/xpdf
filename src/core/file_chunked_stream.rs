use super::base_stream::BaseStream;
use super::chunk_manager::{ChunkLoader, ChunkManager};
use super::error::{PDFError, PDFResult};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Helper function to standardize mutex lock error handling for the file handle.
#[inline]
fn lock_file(file: &Arc<Mutex<File>>) -> PDFResult<MutexGuard<'_, File>> {
    file.lock()
        .map_err(|_| PDFError::StreamError("Failed to lock file handle (mutex poisoned)".to_string()))
}

/// Helper function to standardize mutex lock error handling for the chunk manager.
#[inline]
fn lock_manager(manager: &Arc<Mutex<ChunkManager>>) -> PDFResult<MutexGuard<'_, ChunkManager>> {
    manager.lock()
        .map_err(|_| PDFError::StreamError("Failed to lock chunk manager (mutex poisoned)".to_string()))
}

/// A chunked stream that progressively loads data from a filesystem file.
///
/// This implementation minimizes memory usage by:
/// - Loading chunks on-demand from disk
/// - Maintaining an LRU cache of recently used chunks
/// - Not loading the entire file into memory
///
/// The file handle and chunk manager are shared via Arc, allowing sub-streams
/// to reuse the same resources and cache.
///
/// This mirrors PDF.js's ChunkedStream but optimized for filesystem access
/// with minimal memory footprint.
pub struct FileChunkedStream {
    /// File handle for reading chunks (shared)
    file: Arc<Mutex<File>>,
    /// Path to the file (stored for reference)
    file_path: PathBuf,
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

impl ChunkLoader for FileChunkedStream {
    fn request_chunk(&mut self, chunk_num: usize) -> PDFResult<Vec<u8>> {
        let chunk_start = chunk_num * self.chunk_size;
        let chunk_end = std::cmp::min(chunk_start + self.chunk_size, self.total_length);
        let chunk_length = chunk_end - chunk_start;

        let mut file = lock_file(&self.file)?;

        file.seek(SeekFrom::Start(chunk_start as u64))
            .map_err(|e| PDFError::StreamError(format!("Failed to seek to chunk: {}", e)))?;

        let mut buffer = vec![0u8; chunk_length];
        file.read_exact(&mut buffer)
            .map_err(|e| PDFError::StreamError(format!("Failed to read chunk: {}", e)))?;

        Ok(buffer)
    }

    fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    fn total_length(&self) -> usize {
        self.total_length
    }
}

impl FileChunkedStream {
    /// Creates a new FileChunkedStream from a file path.
    ///
    /// # Arguments
    /// * `path` - Path to the PDF file
    /// * `chunk_size` - Size of each chunk (default: 64KB)
    /// * `max_cached_chunks` - Maximum chunks to keep in memory (default: 10)
    pub fn open<P: AsRef<Path>>(
        path: P,
        chunk_size: Option<usize>,
        max_cached_chunks: Option<usize>,
    ) -> PDFResult<Self> {
        let file_path = path.as_ref().to_path_buf();
        let mut file = File::open(&file_path).map_err(|e| {
            PDFError::StreamError(format!("Failed to open file: {}", e))
        })?;

        // Get file length
        let length = file
            .seek(SeekFrom::End(0))
            .map_err(|e| PDFError::StreamError(format!("Failed to get file length: {}", e)))?
            as usize;

        // Reset to beginning
        file.seek(SeekFrom::Start(0))
            .map_err(|e| PDFError::StreamError(format!("Failed to seek to start: {}", e)))?;

        let manager = ChunkManager::new(length, chunk_size, max_cached_chunks);

        // Cache immutable values to avoid repeated mutex locking
        let cached_chunk_size = manager.chunk_size();
        let cached_length = manager.length();

        Ok(FileChunkedStream {
            file: Arc::new(Mutex::new(file)),
            file_path,
            manager: Arc::new(Mutex::new(manager)),
            pos: 0,
            start: 0,
            chunk_size: cached_chunk_size,
            total_length: cached_length,
        })
    }

    /// Creates a new FileChunkedStream that shares resources with another stream.
    ///
    /// This is used internally for creating sub-streams.
    fn from_shared(
        file: Arc<Mutex<File>>,
        file_path: PathBuf,
        manager: Arc<Mutex<ChunkManager>>,
        chunk_size: usize,
        total_length: usize,
    ) -> Self {
        FileChunkedStream {
            file,
            file_path,
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
}

impl BaseStream for FileChunkedStream {
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

    fn ensure_range(&mut self, start: usize, length: usize) -> PDFResult<()> {
        // This is the critical method for exception-driven progressive loading!
        // When the parser throws DataMissing, it calls this to load the required chunks.
        self.preload_range(start, start + length)
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

        // Create a new FileChunkedStream sharing the same file handle and manager
        let new_stream = FileChunkedStream::from_shared(
            Arc::clone(&self.file),
            self.file_path.clone(),
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
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_file(size: usize) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        file.write_all(&data).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn test_file_chunked_stream_creation() {
        let temp_file = create_test_file(1024);
        let stream = FileChunkedStream::open(temp_file.path(), None, None).unwrap();

        assert_eq!(stream.length(), 1024);
        assert_eq!(stream.pos(), 0);
        assert!(!stream.is_empty());
        assert_eq!(stream.num_chunks(), 1); // 1024 bytes / 64KB = 1 chunk
    }

    #[test]
    fn test_get_byte_loads_chunk() {
        let temp_file = create_test_file(1024);
        let mut stream = FileChunkedStream::open(temp_file.path(), None, None).unwrap();

        assert_eq!(stream.num_chunks_loaded(), 0);

        let byte = stream.get_byte().unwrap();
        assert_eq!(byte, 0);
        assert_eq!(stream.pos(), 1);
        assert_eq!(stream.num_chunks_loaded(), 1);
    }

    #[test]
    fn test_get_bytes() {
        let temp_file = create_test_file(1024);
        let mut stream = FileChunkedStream::open(temp_file.path(), None, None).unwrap();

        let bytes = stream.get_bytes(10).unwrap();
        assert_eq!(bytes.len(), 10);
        assert_eq!(bytes, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(stream.pos(), 10);
    }

    #[test]
    fn test_chunk_caching() {
        let temp_file = create_test_file(200_000); // ~3 chunks at 64KB each
        let mut stream =
            FileChunkedStream::open(temp_file.path(), Some(65536), Some(2)).unwrap();

        assert_eq!(stream.num_chunks(), 4);

        // Load first chunk
        stream.get_byte().unwrap();
        assert_eq!(stream.num_chunks_loaded(), 1);

        // Load second chunk
        stream.set_pos(65536).unwrap();
        stream.get_byte().unwrap();
        assert_eq!(stream.num_chunks_loaded(), 2);

        // Load third chunk (should evict first due to cache size = 2)
        stream.set_pos(131072).unwrap();
        stream.get_byte().unwrap();
        assert_eq!(stream.num_chunks_loaded(), 3); // Total loaded (not cached)
    }

    #[test]
    fn test_preload_chunk() {
        let temp_file = create_test_file(200_000);
        let mut stream = FileChunkedStream::open(temp_file.path(), None, None).unwrap();

        assert_eq!(stream.num_chunks_loaded(), 0);

        stream.preload_chunk(1).unwrap();
        assert_eq!(stream.num_chunks_loaded(), 1);
    }

    #[test]
    fn test_preload_range() {
        let temp_file = create_test_file(200_000);
        let mut stream = FileChunkedStream::open(temp_file.path(), None, None).unwrap();

        stream.preload_range(0, 100_000).unwrap();
        assert!(stream.num_chunks_loaded() >= 2);
    }

    #[test]
    fn test_reset() {
        let temp_file = create_test_file(1024);
        let mut stream = FileChunkedStream::open(temp_file.path(), None, None).unwrap();

        stream.get_bytes(10).unwrap();
        assert_eq!(stream.pos(), 10);

        stream.reset().unwrap();
        assert_eq!(stream.pos(), 0);
    }

    #[test]
    fn test_peek_byte() {
        let temp_file = create_test_file(1024);
        let mut stream = FileChunkedStream::open(temp_file.path(), None, None).unwrap();

        let byte = stream.peek_byte().unwrap();
        assert_eq!(byte, 0);
        assert_eq!(stream.pos(), 0); // Position should not change
    }

    #[test]
    fn test_sub_stream_shares_resources() {
        let temp_file = create_test_file(1024);
        let stream = FileChunkedStream::open(temp_file.path(), None, None).unwrap();

        // Create two sub-streams
        let sub1 = stream.make_sub_stream(0, 512).unwrap();
        let sub2 = stream.make_sub_stream(512, 512).unwrap();

        // They should share the same file handle and manager
        assert_eq!(Arc::strong_count(&stream.file), 3); // stream + sub1 + sub2
        assert_eq!(Arc::strong_count(&stream.manager), 3); // stream + sub1 + sub2
    }
}
