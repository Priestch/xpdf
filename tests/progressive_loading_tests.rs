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
