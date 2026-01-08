//! Progressive loading tests for PDF-X core.
//!
//! These tests validate the exception-driven progressive loading architecture.
//! Based on PDF.js's fetch_stream_spec.js and common_pdfstream_tests.js

use pdf_x_core::core::*;
use pdf_x_core::core::file_chunked_stream::FileChunkedStream;
use pdf_x_core::core::base_stream::BaseStream;
use pdf_x_core::core::stream::Stream;
use pdf_x_core::retry_on_data_missing;

#[test]
fn test_chunk_caching_behavior() {
    // Test that loaded chunks are cached and not re-loaded
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/basicapi.pdf");
    if !test_pdf.exists() {
        return; // Skip if test PDF not available
    }

    // Create stream with small cache (2 chunks)
    let mut stream = FileChunkedStream::open(
        test_pdf,
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
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/tracemonkey.pdf");
    if !test_pdf.exists() {
        return; // Skip if test PDF not available
    }

    // Create stream with tiny cache (1 chunk)
    let mut stream = FileChunkedStream::open(
        test_pdf,
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
fn test_data_missing_propagation() {
    // Test that DataMissing errors are thrown properly
    use pdf_x_core::core::error::PDFError;

    // Create a small byte stream
    let data = vec![1, 2, 3, 4, 5];
    let mut stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;

    // Read bytes successfully
    let result = stream.get_byte();
    assert!(result.is_ok());

    // Try reading beyond available data
    // Should get UnexpectedEndOfStream, not DataMissing (since it's a memory stream)
    for _ in 0..10 {
        let _ = stream.get_byte();
    }

    let result = stream.get_byte();
    assert!(matches!(result, Err(PDFError::UnexpectedEndOfStream)));
}

#[test]
fn test_retry_macro_basic() -> Result<(), Box<dyn std::error::Error>> {
    // Test the retry_on_data_missing! macro behavior
    let data = vec![1, 2, 3, 4, 5];
    let mut stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;

    // The retry macro should work with operations that don't throw DataMissing
    let result = retry_on_data_missing!(stream, {
        stream.get_byte()
    });

    assert!(result.is_ok(), "Retry macro should succeed");
    assert_eq!(result.unwrap(), 1, "Should get first byte");
    Ok(())
}

#[test]
fn test_get_missing_chunks_list() {
    // Test getting list of chunks that haven't been loaded
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/basicapi.pdf");
    if !test_pdf.exists() {
        return;
    }

    let stream = FileChunkedStream::open(
        test_pdf,
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
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/basicapi.pdf");
    if !test_pdf.exists() {
        return;
    }

    let mut stream = FileChunkedStream::open(
        test_pdf,
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
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/empty.pdf");
    if !test_pdf.exists() {
        return;
    }

    let mut stream = FileChunkedStream::open(
        test_pdf,
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
fn test_pdf_open_with_progressive_loading() {
    // Test opening PDF with progressive loading
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/basicapi.pdf");
    if !test_pdf.exists() {
        return;
    }

    use pdf_x_core::core::PDFDocument;

    // Open with small chunks to simulate network loading
    let result = PDFDocument::open_file(
        test_pdf,
        Some(2048),  // 2KB chunks
        Some(3),      // Cache 3 chunks
    );

    assert!(result.is_ok(), "Should open PDF with progressive loading");

    let mut doc = result.unwrap();

    // Get page count - should work without loading entire file
    let page_count = doc.page_count();
    assert!(page_count.is_ok(), "Should get page count");

    let count = page_count.unwrap();
    assert!(count > 0, "Should have at least one page");
}

#[test]
fn test_lazy_page_loading() {
    // Test that page content is not loaded until requested
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/basicapi.pdf");
    if !test_pdf.exists() {
        return;
    }

    use pdf_x_core::core::PDFDocument;

    let mut doc = PDFDocument::open_file(
        test_pdf,
        Some(4096),
        Some(3),
    ).expect("Should open PDF");

    // Get page count without loading page contents
    let page_count = doc.page_count().expect("Should get page count");
    assert!(page_count > 0, "Should have pages");

    // Request specific page - only then should content load
    let page = doc.get_page(0);
    assert!(page.is_ok(), "Should get first page");
}

#[test]
fn test_xref_stream_parsing() {
    // Test parsing PDF with XRef stream (PDF 1.5+)
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/xref-stream.pdf");
    if !test_pdf.exists() {
        return; // Skip if test PDF not available
    }

    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        test_pdf,
        Some(4096),
        Some(3),
    );

    assert!(result.is_ok(), "Should open PDF with XRef stream");
}

#[test]
fn test_incremental_updates_parsing() {
    // Test loading PDF with incremental updates
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/issue3115.pdf");
    if !test_pdf.exists() {
        return; // Skip if test PDF not available
    }

    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        test_pdf,
        Some(4096),
        Some(5),
    );

    assert!(result.is_ok(), "Should open PDF with incremental updates");

    let mut doc = result.unwrap();

    // Should be able to access document despite incremental updates
    let page_count = doc.page_count();
    assert!(page_count.is_ok(), "Should get page count from incremental PDF");
}

#[test]
fn test_linearized_pdf_loading() {
    // Test loading a linearized PDF (fast web view)
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/linearized.pdf");
    if !test_pdf.exists() {
        return; // Skip if linearized PDF not available
    }

    use pdf_x_core::core::PDFDocument;

    let result = PDFDocument::open_file(
        test_pdf,
        Some(4096),
        Some(5),
    );

    assert!(result.is_ok(), "Should open linearized PDF");

    let mut doc = result.unwrap();

    // Linearized PDFs should be able to display first page quickly
    let page = doc.get_page(0);
    assert!(page.is_ok(), "Should get first page from linearized PDF");
}

#[test]
fn test_chunk_boundaries() {
    // Test reading across chunk boundaries
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/tracemonkey.pdf");
    if !test_pdf.exists() {
        return;
    }

    let mut stream = FileChunkedStream::open(
        test_pdf,
        Some(1024),
        Some(5),
    ).expect("Should create stream");

    // Read first chunk
    let chunk1 = stream.get_byte_range(0, 1024).expect("Should read first chunk");
    assert_eq!(chunk1.len(), 1024, "First chunk should be 1KB");

    // Read second chunk
    let chunk2 = stream.get_byte_range(1024, 2048).expect("Should read second chunk");
    assert_eq!(chunk2.len(), 1024, "Second chunk should be 1KB");

    // Verify data integrity - first chunk should have PDF header
    assert_eq!(&chunk1[0..4], b"%PDF", "First chunk should have PDF header");
}

#[test]
fn test_random_access_chunk_loading() {
    // Test loading chunks in random order (like xref at end of file)
    use std::path::Path;

    let test_pdf = Path::new("tests/fixtures/pdfs/basicapi.pdf");
    if !test_pdf.exists() {
        return;
    }

    let mut stream = FileChunkedStream::open(test_pdf, None, None).expect("Should create stream");

    let file_size = stream.length();

    // Jump to end and read
    let end_data = stream.get_byte_range(file_size - 100, file_size).expect("Should read from end");
    assert_eq!(end_data.len(), 100, "Should read 100 bytes from end");

    // Jump back to beginning
    let start_data = stream.get_byte_range(0, 50).expect("Should read from beginning");
    assert_eq!(start_data.len(), 50, "Should read 50 bytes from beginning");

    // Verify we got the PDF header at the beginning
    assert_eq!(&start_data[0..4], b"%PDF", "Should have PDF header at beginning");
}
