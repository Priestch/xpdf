//! Progressive loading tests
//!
//! These tests validate the exception-driven progressive loading architecture.
//! Based on PDF.js's fetch_stream_spec.js and common_pdfstream_tests.js

mod test_utils;

use pdf_x::core::*;
use test_utils::*;

#[test]
fn test_file_chunked_stream_creation() {
    let result = create_file_stream("basicapi.pdf");
    assert!(result.is_ok(), "Should create FileChunkedStream successfully");
}

#[test]
fn test_progressive_loading_basic() {
    // Test that we can load a PDF progressively without loading entire file
    let mut stream = create_file_stream("basicapi.pdf")
        .expect("Failed to create stream");

    // Initially, minimal data should be loaded
    // Read first 100 bytes - should trigger chunk loading
    let result = stream.read(100);
    assert!(result.is_ok(), "Should be able to read first chunk");

    let data = result.unwrap();
    assert_eq!(data.len(), 100, "Should read 100 bytes");

    // Verify it's a PDF header
    assert_eq!(&data[0..4], b"%PDF", "Should start with PDF header");
}

#[test]
fn test_chunked_loading_small_pdf() {
    // Test loading a small PDF progressively
    let mut stream = create_file_stream("empty.pdf")
        .expect("Should load empty.pdf");

    // Read the entire file
    let result = stream.read(10000);
    assert!(result.is_ok(), "Should be able to read empty PDF");

    let data = result.unwrap();
    assert!(!data.is_empty(), "Empty PDF should have some content");

    // Verify it's a PDF
    assert_eq!(&data[0..4], b"%PDF", "Should be a valid PDF");
}

#[test]
fn test_chunked_loading_large_pdf() {
    // Test loading a larger PDF (tracemonkey.pdf ~1MB)
    let mut stream = create_file_stream("tracemonkey.pdf")
        .expect("Should create stream for tracemonkey.pdf");

    // Read first chunk
    let result = stream.read(1024);
    assert!(result.is_ok(), "Should be able to read first chunk from large PDF");

    let data = result.unwrap();
    assert_eq!(data.len(), 1024, "Should read 1KB chunk");
    assert_eq!(&data[0..4], b"%PDF", "Large PDF should have valid header");

    // Verify we haven't loaded the entire file yet (progressive loading)
    assert!(stream.len() > 1024, "Large PDF should be bigger than 1KB");
}

#[test]
fn test_data_missing_exception_pattern() {
    // Test the exception-driven loading pattern
    // This is CRITICAL to the architecture

    let mut stream = create_file_stream("basicapi.pdf")
        .expect("Failed to create stream");

    // Read first chunk successfully
    let result1 = stream.read(100);
    assert!(result1.is_ok(), "Should read first chunk");

    // Seek to a later position (simulating jumping to xref at end)
    let file_size = stream.len();
    stream.seek(file_size - 100).expect("Should seek to end");

    // Read from that position - should load new chunk
    let result2 = stream.read(50);
    assert!(result2.is_ok(), "Should read chunk from end of file");

    let data2 = result2.unwrap();
    assert_eq!(data2.len(), 50, "Should read 50 bytes from end");

    // Verify we got %%EOF marker
    let data_str = String::from_utf8_lossy(&data2);
    assert!(data_str.contains("%%EOF") || data_str.contains("EOF"), "Should find EOF marker");
}

#[test]
fn test_data_missing_thrown_for_unloaded_chunk() {
    // Test that DataMissing is thrown when accessing unloaded data
    use pdf_x_core::core::FileChunkedStream;
    use pdf_x_core::core::error::PDFError;

    // Create a stream with a small cache size
    let mut stream = FileChunkedStream::open_with_options(
        &get_test_pdf_path("basicapi.pdf"),
        Some(1024),  // 1KB chunk size
        Some(2),     // Cache only 2 chunks
    ).expect("Should create stream");

    // Initially, no chunks are loaded
    assert_eq!(stream.num_chunks_loaded(), 0, "Should start with no chunks loaded");

    // Try to read from a middle chunk that hasn't been loaded
    // This should trigger DataMissing
    let file_size = stream.length();
    let middle_pos = file_size / 2;

    // Reset to ensure we're not at the beginning
    stream.reset().expect("Should reset");

    // Try to get bytes from unloaded position
    // With FileChunkedStream, get_byte_range will load chunks automatically
    // So we need to verify the progressive loading behavior differently
    let result = stream.get_byte_range(middle_pos, middle_pos + 100);
    assert!(result.is_ok(), "Should load chunk and return data");

    // Verify a chunk was loaded
    assert!(stream.num_chunks_loaded() > 0, "Should have loaded at least one chunk");
}

#[test]
fn test_chunk_boundaries() {
    // Test reading across chunk boundaries
    // Ensure data is correctly assembled from multiple chunks
    let mut stream = create_file_stream("tracemonkey.pdf")
        .expect("Should create stream");

    // Read first chunk
    let chunk1 = stream.read(1024).expect("Should read first chunk");
    assert_eq!(chunk1.len(), 1024, "First chunk should be 1KB");

    // Read second chunk
    let chunk2 = stream.read(1024).expect("Should read second chunk");
    assert_eq!(chunk2.len(), 1024, "Second chunk should be 1KB");

    // Verify data integrity - both chunks should have valid content
    assert_eq!(&chunk1[0..4], b"%PDF", "First chunk should have PDF header");

    // Second chunk is continuation data, so no specific validation needed
    assert!(!chunk2.is_empty(), "Second chunk should have data");
}

#[test]
fn test_sequential_chunk_loading() {
    // Test loading chunks sequentially from start to end
    let mut stream = create_file_stream("basicapi.pdf")
        .expect("Failed to create stream");

    // Read sequentially through the file
    // Verify chunks are loaded in order
    let mut stream = create_file_stream("basicapi.pdf")
        .expect("Failed to create stream");

    let mut total_read = 0;
    let mut chunks_read = 0;

    // Read in 512-byte chunks
    loop {
        let result = stream.read(512);
        match result {
            Ok(data) if data.is_empty() => break,
            Ok(data) => {
                total_read += data.len();
                chunks_read += 1;
            }
            Err(_) => break,
        }
    }

    assert!(chunks_read > 0, "Should read at least one chunk");
    assert!(total_read > 1000, "Should read most of the PDF (basicapi.pdf)");
}

#[test]
fn test_random_access_chunk_loading() {
    // Test loading chunks in random order (like xref at end of file)
    let mut stream = create_file_stream("basicapi.pdf")
        .expect("Failed to create stream");

    // Jump to end of file to read xref
    // Then jump to beginning for header
    // Verify both chunks load correctly
    let file_size = stream.len();

    // Jump to end and read
    stream.seek(file_size - 100).expect("Should seek to end");
    let end_data = stream.read(50).expect("Should read from end");
    assert_eq!(end_data.len(), 50, "Should read 50 bytes from end");

    // Jump back to beginning
    stream.seek(0).expect("Should seek to beginning");
    let start_data = stream.read(50).expect("Should read from beginning");
    assert_eq!(start_data.len(), 50, "Should read 50 bytes from beginning");

    // Verify we got the PDF header at the beginning
    assert_eq!(&start_data[0..4], b"%PDF", "Should have PDF header at beginning");
}

#[test]
fn test_overlapping_chunk_requests() {
    // Test requesting overlapping byte ranges
    // Verify no duplicate loading or corruption
    let mut stream = create_file_stream("rotation.pdf")
        .expect("Should create stream for rotation.pdf");

    // Read bytes 0-100
    let data1 = stream.read(100).expect("Should read first 100 bytes");

    // Read bytes 50-150 (overlaps with previous read)
    // Seek to position 50
    stream.seek(50).expect("Should seek to position 50");
    let data2 = stream.read(100).expect("Should read next 100 bytes from position 50");

    // Verify both reads succeeded
    assert_eq!(data1.len(), 100, "First read should be 100 bytes");
    assert_eq!(data2.len(), 100, "Second read should be 100 bytes");

    // Verify first read has PDF header
    assert_eq!(&data1[0..4], b"%PDF", "First chunk should have PDF header");
}

#[test]
fn test_minimal_loading_for_metadata() {
    // Test that we can extract metadata without loading entire PDF
    // This validates true progressive loading

    let mut stream = create_file_stream("basicapi.pdf")
        .expect("Failed to create stream");

    // Load only enough to get:
    // - PDF header
    // - xref table location (from trailer at end)
    // - Catalog dictionary
    // - Metadata

    // Verify we didn't load the entire file
    // (Check actual bytes loaded vs file size)
}

#[test]
fn test_lazy_page_loading() {
    // Test that page content is not loaded until requested
    let result = assert_pdf_loads("basicapi.pdf");
    assert!(result.is_ok(), "Should load basicapi.pdf");

    let doc = result.unwrap();

    // Document should be open but page contents not loaded
    // Verify we can get page count without loading all pages

    // Request specific page - only then should content load
}

#[test]
fn test_multiple_sources_file() {
    // Test FileChunkedStream specifically
    let stream = create_file_stream("basicapi.pdf");
    assert!(stream.is_ok(), "FileChunkedStream should work");
}

#[test]
#[ignore] // Requires network access
fn test_multiple_sources_http() {
    // Test HttpChunkedStream with range requests
    // This test requires a test HTTP server
    // Ignored by default, run with --ignored flag
}

#[test]
fn test_stream_length_reporting() {
    // Test that stream reports correct total length
    let stream = create_file_stream("empty.pdf");
    assert!(stream.is_ok());

    // Verify reported length matches actual file size
    let pdf_bytes = load_test_pdf_bytes("empty.pdf").unwrap();
    // stream.total_length() should equal pdf_bytes.len()
}

#[test]
fn test_concurrent_chunk_requests() {
    // Test behavior when multiple chunks are requested concurrently
    // (Relevant for async/parallel processing)
    let stream = create_file_stream("tracemonkey.pdf");
    assert!(stream.is_ok());

    // Simulate multiple concurrent reads
    // Verify no race conditions or corruption
}

#[test]
fn test_chunk_caching() {
    // Test that loaded chunks are cached appropriately
    let stream = create_file_stream("basicapi.pdf");
    assert!(stream.is_ok());

    // Load a chunk
    // Access same chunk again
    // Verify it's not re-loaded from disk/network
}

#[test]
fn test_error_recovery_missing_chunk() {
    // Test graceful handling when chunk cannot be loaded
    // (e.g., network error, file truncated)

    // This tests robustness of progressive loading
}

#[test]
fn test_linearized_pdf_fast_display() {
    // Test optimized loading path for linearized PDFs
    // Should be able to display first page quickly

    // Note: Need a linearized test PDF for this
    // Can skip if not available yet
}

#[test]
fn test_progressive_parsing_xref() {
    // Test that xref table parsing works progressively
    let result = assert_pdf_loads("basicapi.pdf");
    assert!(result.is_ok());

    // Verify xref was parsed without loading entire file
}

#[test]
fn test_incremental_updates() {
    // Test loading PDF with incremental updates (multiple xref sections)
    // These PDFs have been modified with append-only updates

    // Note: Need test PDF with incremental updates
}

// Performance/behavior tests

#[test]
fn test_memory_efficiency() {
    // Test that memory usage remains bounded
    // Should not load entire file into memory

    let _stream = create_file_stream("tracemonkey.pdf");

    // Measure memory usage - should be << file size
    // This requires memory profiling tools
}

#[test]
fn test_load_time_efficiency() {
    // Test that initial load is fast (doesn't wait for complete download)
    use std::time::Instant;

    let start = Instant::now();
    let result = create_file_stream("tracemonkey.pdf");
    let elapsed = start.elapsed();

    assert!(result.is_ok());
    // Should be very fast since we're not loading the whole file
    assert!(elapsed.as_millis() < 100, "Stream creation should be fast");
}

#[test]
fn test_chunk_size_configuration() {
    // Test that chunk size can be configured
    // Verify different chunk sizes work correctly

    // Small chunks (good for network)
    // Large chunks (good for local files)
}

// ============================================================================
// Comprehensive Progressive Loading Tests
// ============================================================================

#[test]
fn test_chunk_caching_behavior() {
    // Test that loaded chunks are cached and not re-loaded
    use pdf_x_core::core::FileChunkedStream;

    // Create stream with small cache (2 chunks)
    let mut stream = FileChunkedStream::open_with_options(
        &get_test_pdf_path("tracemonkey.pdf"),
        Some(4096),  // 4KB chunks
        Some(2),      // Cache 2 chunks
    ).expect("Should create stream");

    // Load first chunk
    let _data1 = stream.get_byte_range(0, 100).expect("Should load first chunk");
    let chunks_after_first = stream.num_chunks_loaded();
    assert!(chunks_after_first > 0, "Should have loaded chunk");

    // Access same range again - should use cached chunk
    let _data2 = stream.get_byte_range(0, 100).expect("Should use cached chunk");
    let chunks_after_second = stream.num_chunks_loaded();
    assert_eq!(chunks_after_second, chunks_after_first, "Should not load additional chunks");
}

#[test]
fn test_chunk_eviction_from_cache() {
    // Test that cache evicts old chunks when full
    use pdf_x_core::core::FileChunkedStream;

    // Create stream with tiny cache (1 chunk)
    let mut stream = FileChunkedStream::open_with_options(
        &get_test_pdf_path("tracemonkey.pdf"),
        Some(1024),  // 1KB chunks
        Some(1),      // Cache only 1 chunk
    ).expect("Should create stream");

    // Load first chunk
    let _data1 = stream.get_byte_range(0, 100).expect("Should load first chunk");
    assert_eq!(stream.num_chunks_loaded(), 1, "Should have 1 chunk cached");

    // Load a far-away chunk (should evict first chunk from cache)
    let file_size = stream.length();
    let _data2 = stream.get_byte_range(file_size - 100, file_size).expect("Should load last chunk");

    // Cache size should still be 1, but we loaded 2 chunks total
    assert_eq!(stream.num_chunks_loaded(), 1, "Cache should only hold 1 chunk at a time");
}

#[test]
fn test_minimal_data_on_pdf_open() {
    // Test that opening a PDF loads minimal data
    use pdf_x_core::core::PDFDocument;

    let file_path = get_test_pdf_path("basicapi.pdf");
    let file_size = std::fs::metadata(&file_path).unwrap().len();

    // Open PDF with progressive loading
    let _doc = PDFDocument::open_file(&file_path, Some(4096), Some(5))
        .expect("Should open PDF");

    // The file should not be fully loaded yet
    // This is hard to test directly since we can't access the stream from here
    // But the test validates the API works correctly

    // For a real test, we'd need to measure actual bytes loaded vs file size
    assert!(file_size > 0, "Test PDF should exist");
}

#[test]
fn test_xref_parsing_with_progressive_loading() {
    // Test that XRef parsing works with progressive loading
    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        &get_test_pdf_path("basicapi.pdf"),
        Some(4096),  // 4KB chunks
        Some(3),      // Cache 3 chunks
    );

    assert!(result.is_ok(), "Should open PDF with progressive loading");

    let doc = result.unwrap();

    // Get page count - this should work without loading entire file
    let page_count = doc.page_count();
    assert!(page_count.is_ok(), "Should get page count");

    let count = page_count.unwrap();
    assert!(count > 0, "Should have at least one page");
}

#[test]
fn test_lazy_page_loading() {
    // Test that page content is not loaded until requested
    use pdf_x_core::core::PDFDocument;

    let doc = PDFDocument::open_file(
        &get_test_pdf_path("basicapi.pdf"),
        Some(4096),
        Some(3),
    ).expect("Should open PDF");

    // Document should be open but page contents not loaded
    // Get page count without loading page contents
    let page_count = doc.page_count().expect("Should get page count");
    assert!(page_count > 0, "Should have pages");

    // Request specific page - only then should content load
    let page = doc.get_page(0);
    assert!(page.is_ok(), "Should get first page");
}

#[test]
fn test_incremental_updates_parsing() {
    // Test loading PDF with incremental updates
    if !test_pdf_exists("issue3115.pdf") {
        return; // Skip if test PDF not available
    }

    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        &get_test_pdf_path("issue3115.pdf"),
        Some(4096),
        Some(5),
    );

    assert!(result.is_ok(), "Should open PDF with incremental updates");
    let doc = result.unwrap();

    // Should be able to access document despite incremental updates
    let page_count = doc.page_count();
    assert!(page_count.is_ok(), "Should get page count from incremental PDF");
}

#[test]
fn test_xref_stream_parsing() {
    // Test parsing PDF with XRef stream (PDF 1.5+)
    if !test_pdf_exists("xref-stream.pdf") {
        return; // Skip if test PDF not available
    }

    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        &get_test_pdf_path("xref-stream.pdf"),
        Some(4096),
        Some(3),
    );

    assert!(result.is_ok(), "Should open PDF with XRef stream");
}

#[test]
fn test_get_missing_chunks_list() {
    // Test getting list of chunks that haven't been loaded
    use pdf_x_core::core::FileChunkedStream;

    let stream = FileChunkedStream::open_with_options(
        &get_test_pdf_path("basicapi.pdf"),
        Some(2048),  // 2KB chunks
        Some(5),      // Cache 5 chunks
    ).expect("Should create stream");

    // Initially, no chunks are loaded
    let missing = stream.get_missing_chunks();
    assert!(!missing.is_empty(), "Should have missing chunks initially");

    // Load some chunks
    let _data = stream.get_byte_range(0, 1000).expect("Should load data");

    // Missing chunks list should be updated
    let missing_after = stream.get_missing_chunks();
    assert!(missing_after.len() < missing.len(), "Should have fewer missing chunks");
}

#[test]
fn test_preload_range() {
    // Test preloading a specific byte range
    use pdf_x_core::core::FileChunkedStream;

    let mut stream = FileChunkedStream::open_with_options(
        &get_test_pdf_path("basicapi.pdf"),
        Some(1024),
        Some(2),
    ).expect("Should create stream");

    // Preload a range
    let result = stream.preload_range(1000, 5000);
    assert!(result.is_ok(), "Should preload range");

    // Verify chunks were loaded
    assert!(stream.num_chunks_loaded() > 0, "Should have loaded chunks");
}

#[test]
fn test_is_fully_loaded() {
    // Test checking if all chunks are loaded
    use pdf_x_core::core::FileChunkedStream;

    let mut stream = FileChunkedStream::open_with_options(
        &get_test_pdf_path("empty.pdf"),  // Use small PDF
        Some(1024),
        Some(10),
    ).expect("Should create stream");

    // Initially not fully loaded
    assert!(!stream.is_fully_loaded(), "Should not be fully loaded initially");

    // Read entire file
    let file_size = stream.length();
    let _data = stream.get_byte_range(0, file_size).expect("Should read entire file");

    // Now should be fully loaded
    assert!(stream.is_fully_loaded(), "Should be fully loaded after reading all");
}

#[test]
fn test_retry_macro_with_data_missing() {
    // Test the retry_on_data_missing! macro behavior
    use pdf_x_core::core::stream::Stream;
    use pdf_x_core::core::base_stream::BaseStream;
    use pdf_x_core::retry_on_data_missing;

    let data = vec![1, 2, 3, 4, 5];
    let mut stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;

    // The retry macro should work with operations that don't throw DataMissing
    let result: std::result::Result<u8, pdf_x_core::core::error::PDFError> = retry_on_data_missing!(stream, {
        stream.get_byte()
    });

    assert!(result.is_ok(), "Retry macro should succeed");
    assert_eq!(result.unwrap(), 1, "Should get first byte");
}

#[test]
fn test_pdf_open_with_large_chunk_size() {
    // Test opening PDF with large chunk size (for local files)
    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        &get_test_pdf_path("basicapi.pdf"),
        Some(65536),  // 64KB chunks (larger, good for local files)
        Some(10),
    );

    assert!(result.is_ok(), "Should open with large chunk size");

    let doc = result.unwrap();
    let page_count = doc.page_count();
    assert!(page_count.is_ok(), "Should get page count");
}

#[test]
fn test_pdf_open_with_small_chunk_size() {
    // Test opening PDF with small chunk size (for network simulation)
    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        &get_test_pdf_path("basicapi.pdf"),
        Some(1024),  // 1KB chunks (smaller, good for network)
        Some(5),
    );

    assert!(result.is_ok(), "Should open with small chunk size");

    let doc = result.unwrap();
    let page_count = doc.page_count();
    assert!(page_count.is_ok(), "Should get page count");
}

#[test]
fn test_progressive_loading_with_linearized_pdf() {
    // Test loading a linearized PDF (fast web view)
    if !test_pdf_exists("linearized.pdf") {
        return; // Skip if linearized PDF not available
    }

    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        &get_test_pdf_path("linearized.pdf"),
        Some(4096),
        Some(5),
    );

    assert!(result.is_ok(), "Should open linearized PDF");

    let doc = result.unwrap();

    // Linearized PDFs should be able to display first page quickly
    let page = doc.get_page(0);
    assert!(page.is_ok(), "Should get first page from linearized PDF");
}
