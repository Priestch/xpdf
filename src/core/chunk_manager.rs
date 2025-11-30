use super::error::{PDFError, PDFResult};
use std::collections::{HashMap, HashSet, VecDeque};

/// Default chunk size: 64KB (same as PDF.js)
pub const DEFAULT_CHUNK_SIZE: usize = 65536;

/// Default maximum number of chunks to keep in memory cache
pub const DEFAULT_MAX_CACHED_CHUNKS: usize = 10;

/// Trait for loading chunks from various data sources.
///
/// This trait is analogous to PDF.js's ChunkedStreamManager interface,
/// which handles the actual data loading from network, filesystem, or other sources.
///
/// Implementers (FileChunkedStream, HttpChunkedStream) are responsible for:
/// - Managing their own data source (File handle, HTTP client, etc.)
/// - Loading chunks on demand when requested
/// - Returning chunk data to be managed by ChunkManager
pub trait ChunkLoader {
    /// Requests and loads a specific chunk from the data source.
    ///
    /// This method should perform the actual I/O operation to fetch the chunk data.
    /// For filesystem loaders: seek to position and read
    /// For HTTP loaders: make range request
    ///
    /// # Arguments
    /// * `chunk_num` - The chunk number to load (0-based index)
    ///
    /// # Returns
    /// The chunk data as a Vec<u8>. May be shorter than chunk_size for the last chunk.
    fn request_chunk(&mut self, chunk_num: usize) -> PDFResult<Vec<u8>>;

    /// Returns the chunk size in bytes.
    fn chunk_size(&self) -> usize;

    /// Returns the total data length in bytes.
    fn total_length(&self) -> usize;

    /// Returns the total number of chunks.
    fn num_chunks(&self) -> usize {
        self.total_length().div_ceil(self.chunk_size())
    }
}

/// Manages chunk data storage and tracks which chunks are loaded.
///
/// This is analogous to PDF.js's ChunkedStream class, which:
/// - Stores chunk data (we use LRU cache instead of full buffer for memory efficiency)
/// - Tracks which chunks are loaded (_loadedChunks Set in JS)
/// - Provides methods to query and access loaded chunks
///
/// ChunkManager is NOT generic - it's a concrete struct that manages data
/// regardless of where it comes from. The ChunkLoader trait handles the loading.
pub struct ChunkManager {
    /// Total length of the data in bytes
    total_length: usize,
    /// Size of each chunk in bytes
    chunk_size: usize,
    /// Total number of chunks
    num_chunks: usize,

    /// Cache of loaded chunks (chunk_number -> data)
    /// This is an LRU cache for memory efficiency
    chunk_cache: HashMap<usize, Vec<u8>>,

    /// Set of all chunks that have been loaded at some point
    /// (analogous to _loadedChunks in PDF.js)
    loaded_chunks: HashSet<usize>,

    /// LRU queue for cache eviction (stores chunk numbers)
    lru_queue: VecDeque<usize>,

    /// Maximum number of chunks to keep in cache
    max_cached_chunks: usize,
}

impl ChunkManager {
    /// Creates a new ChunkManager.
    ///
    /// # Arguments
    /// * `total_length` - Total length of the data
    /// * `chunk_size` - Size of each chunk (default: 64KB)
    /// * `max_cached_chunks` - Maximum chunks to keep in memory (default: 10)
    pub fn new(
        total_length: usize,
        chunk_size: Option<usize>,
        max_cached_chunks: Option<usize>,
    ) -> Self {
        let chunk_size = chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
        let max_cached_chunks = max_cached_chunks.unwrap_or(DEFAULT_MAX_CACHED_CHUNKS);
        let num_chunks = total_length.div_ceil(chunk_size);

        ChunkManager {
            total_length,
            chunk_size,
            num_chunks,
            chunk_cache: HashMap::new(),
            loaded_chunks: HashSet::new(),
            lru_queue: VecDeque::new(),
            max_cached_chunks,
        }
    }

    /// Returns the total length of the data.
    pub fn length(&self) -> usize {
        self.total_length
    }

    /// Returns the chunk size.
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Returns the total number of chunks.
    pub fn num_chunks(&self) -> usize {
        self.num_chunks
    }

    /// Gets the chunk number for a given byte position.
    pub fn get_chunk_number(&self, pos: usize) -> usize {
        pos / self.chunk_size
    }

    /// Receives chunk data and stores it.
    ///
    /// Analogous to ChunkedStream.onReceiveData() in PDF.js.
    /// This is called by the stream after it loads a chunk.
    ///
    /// # Arguments
    /// * `chunk_num` - The chunk number
    /// * `chunk` - The chunk data
    pub fn on_receive_data(&mut self, chunk_num: usize, chunk: Vec<u8>) -> PDFResult<()> {
        if chunk_num >= self.num_chunks {
            return Err(PDFError::InvalidByteRange {
                begin: chunk_num * self.chunk_size,
                end: (chunk_num + 1) * self.chunk_size,
            });
        }

        // Mark as loaded
        self.loaded_chunks.insert(chunk_num);

        // If already in cache, update LRU
        if self.chunk_cache.contains_key(&chunk_num) {
            self.lru_queue.retain(|&x| x != chunk_num);
            self.lru_queue.push_back(chunk_num);
            self.chunk_cache.insert(chunk_num, chunk);
            return Ok(());
        }

        // Evict LRU chunk if cache is full
        if self.chunk_cache.len() >= self.max_cached_chunks {
            if let Some(lru_chunk) = self.lru_queue.pop_front() {
                self.chunk_cache.remove(&lru_chunk);
            }
        }

        // Add to cache
        self.chunk_cache.insert(chunk_num, chunk);
        self.lru_queue.push_back(chunk_num);

        Ok(())
    }

    /// Checks if a specific chunk has been loaded.
    ///
    /// Analogous to ChunkedStream.hasChunk() in PDF.js.
    pub fn has_chunk(&self, chunk: usize) -> bool {
        self.loaded_chunks.contains(&chunk)
    }

    /// Returns a list of chunk numbers that have not been loaded.
    ///
    /// Analogous to ChunkedStream.getMissingChunks() in PDF.js.
    pub fn get_missing_chunks(&self) -> Vec<usize> {
        (0..self.num_chunks)
            .filter(|chunk| !self.loaded_chunks.contains(chunk))
            .collect()
    }

    /// Returns the next unloaded chunk starting from beginChunk, with wraparound.
    ///
    /// Analogous to ChunkedStream.nextEmptyChunk() in PDF.js.
    pub fn next_empty_chunk(&self, begin_chunk: usize) -> Option<usize> {
        for i in 0..self.num_chunks {
            let chunk = (begin_chunk + i) % self.num_chunks; // Wrap around to beginning
            if !self.loaded_chunks.contains(&chunk) {
                return Some(chunk);
            }
        }
        None
    }

    /// Returns the number of chunks currently loaded (ever loaded, not just cached).
    ///
    /// Analogous to ChunkedStream.numChunksLoaded in PDF.js.
    pub fn num_chunks_loaded(&self) -> usize {
        self.loaded_chunks.len()
    }

    /// Returns true if all chunks have been loaded.
    ///
    /// Analogous to ChunkedStream.isDataLoaded in PDF.js.
    pub fn is_data_loaded(&self) -> bool {
        self.loaded_chunks.len() == self.num_chunks
    }

    /// Gets a reference to a cached chunk.
    ///
    /// Returns None if the chunk is not currently in the cache
    /// (it may have been loaded but evicted).
    pub fn get_chunk(&self, chunk_num: usize) -> Option<&Vec<u8>> {
        self.chunk_cache.get(&chunk_num)
    }

    /// Gets a byte from the cache (chunk must be in cache).
    ///
    /// Returns error if chunk is not in cache or position is invalid.
    pub fn get_byte_from_cache(&self, pos: usize) -> PDFResult<u8> {
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

    /// Marks a chunk as accessed, updating the LRU queue.
    ///
    /// This should be called when a chunk is accessed to maintain proper LRU ordering.
    pub fn mark_chunk_accessed(&mut self, chunk_num: usize) {
        if self.chunk_cache.contains_key(&chunk_num) {
            self.lru_queue.retain(|&x| x != chunk_num);
            self.lru_queue.push_back(chunk_num);
        }
    }

    /// Checks if a chunk is currently in the cache (not just loaded).
    pub fn is_chunk_cached(&self, chunk_num: usize) -> bool {
        self.chunk_cache.contains_key(&chunk_num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_creation() {
        let manager = ChunkManager::new(1024, None, None);

        assert_eq!(manager.length(), 1024);
        assert_eq!(manager.chunk_size(), DEFAULT_CHUNK_SIZE);
        assert_eq!(manager.num_chunks(), 1);
    }

    #[test]
    fn test_on_receive_data() {
        let mut manager = ChunkManager::new(200, Some(100), Some(2));
        let chunk_data = vec![1u8, 2, 3, 4, 5];

        assert_eq!(manager.num_chunks_loaded(), 0);
        assert!(!manager.has_chunk(0));

        manager.on_receive_data(0, chunk_data).unwrap();

        assert_eq!(manager.num_chunks_loaded(), 1);
        assert!(manager.has_chunk(0));
        assert!(manager.is_chunk_cached(0));
    }

    #[test]
    fn test_get_missing_chunks() {
        let mut manager = ChunkManager::new(300, Some(100), Some(2));

        // Initially all chunks are missing
        let missing = manager.get_missing_chunks();
        assert_eq!(missing, vec![0, 1, 2]);

        // Load chunk 1
        manager.on_receive_data(1, vec![1u8; 100]).unwrap();

        let missing = manager.get_missing_chunks();
        assert_eq!(missing, vec![0, 2]);
    }

    #[test]
    fn test_next_empty_chunk() {
        let mut manager = ChunkManager::new(300, Some(100), Some(2));

        // Load chunks 0 and 2
        manager.on_receive_data(0, vec![0u8; 100]).unwrap();
        manager.on_receive_data(2, vec![2u8; 100]).unwrap();

        // Next empty from 0 should be 1
        assert_eq!(manager.next_empty_chunk(0), Some(1));

        // Next empty from 1 should be 1
        assert_eq!(manager.next_empty_chunk(1), Some(1));

        // Load chunk 1
        manager.on_receive_data(1, vec![1u8; 100]).unwrap();

        // No empty chunks
        assert_eq!(manager.next_empty_chunk(0), None);
    }

    #[test]
    fn test_lru_eviction() {
        let mut manager = ChunkManager::new(300, Some(100), Some(2));

        // Load chunks 0 and 1 (fills cache)
        manager.on_receive_data(0, vec![0u8; 100]).unwrap();
        manager.on_receive_data(1, vec![1u8; 100]).unwrap();

        assert!(manager.is_chunk_cached(0));
        assert!(manager.is_chunk_cached(1));

        // Load chunk 2 (should evict chunk 0)
        manager.on_receive_data(2, vec![2u8; 100]).unwrap();

        assert!(!manager.is_chunk_cached(0)); // Evicted from cache
        assert!(manager.has_chunk(0));        // But still marked as loaded
        assert!(manager.is_chunk_cached(1));
        assert!(manager.is_chunk_cached(2));
    }

    #[test]
    fn test_is_data_loaded() {
        let mut manager = ChunkManager::new(200, Some(100), Some(2));

        assert!(!manager.is_data_loaded());

        manager.on_receive_data(0, vec![0u8; 100]).unwrap();
        assert!(!manager.is_data_loaded());

        manager.on_receive_data(1, vec![1u8; 100]).unwrap();
        assert!(manager.is_data_loaded());
    }

    #[test]
    fn test_get_byte_from_cache() {
        let mut manager = ChunkManager::new(200, Some(100), Some(2));
        let chunk_data: Vec<u8> = (0..100).collect();

        manager.on_receive_data(0, chunk_data).unwrap();

        assert_eq!(manager.get_byte_from_cache(0).unwrap(), 0);
        assert_eq!(manager.get_byte_from_cache(50).unwrap(), 50);
        assert_eq!(manager.get_byte_from_cache(99).unwrap(), 99);
    }
}
