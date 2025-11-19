use super::base_stream::BaseStream;
use super::error::{PDFError, PDFResult};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Default chunk size: 64KB (same as PDF.js)
pub const DEFAULT_CHUNK_SIZE: usize = 65536;

/// Default maximum number of chunks to keep in memory cache
pub const DEFAULT_MAX_CACHED_CHUNKS: usize = 10;

/// A chunked stream that progressively loads data from a filesystem file.
///
/// This implementation minimizes memory usage by:
/// - Loading chunks on-demand from disk
/// - Maintaining an LRU cache of recently used chunks
/// - Not loading the entire file into memory
///
/// This mirrors PDF.js's ChunkedStream but optimized for filesystem access
/// with minimal memory footprint.
pub struct FileChunkedStream {
    /// File handle for reading chunks
    file: File,
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
        let mut file = File::open(path).map_err(|e| {
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

        let chunk_size = chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
        let max_cached_chunks = max_cached_chunks.unwrap_or(DEFAULT_MAX_CACHED_CHUNKS);
        let num_chunks = length.div_ceil(chunk_size);

        Ok(FileChunkedStream {
            file,
            length,
            chunk_size,
            num_chunks,
            pos: 0,
            start: 0,
            chunk_cache: HashMap::new(),
            lru_queue: VecDeque::new(),
            max_cached_chunks,
        })
    }

    /// Gets the chunk number for a given byte position.
    fn get_chunk_number(&self, pos: usize) -> usize {
        pos / self.chunk_size
    }

    /// Loads a chunk from disk if not already cached.
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

        // Load chunk from disk
        let chunk_start = chunk_num * self.chunk_size;
        let chunk_end = std::cmp::min(chunk_start + self.chunk_size, self.length);
        let chunk_length = chunk_end - chunk_start;

        self.file
            .seek(SeekFrom::Start(chunk_start as u64))
            .map_err(|e| PDFError::StreamError(format!("Failed to seek to chunk: {}", e)))?;

        let mut buffer = vec![0u8; chunk_length];
        self.file
            .read_exact(&mut buffer)
            .map_err(|e| PDFError::StreamError(format!("Failed to read chunk: {}", e)))?;

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
}

impl BaseStream for FileChunkedStream {
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
        // would require sharing the file handle or cloning it
        Err(PDFError::Generic(
            "Sub-streams not yet supported for FileChunkedStream".to_string(),
        ))
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
        assert_eq!(stream.num_chunks_loaded(), 2);
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
}
