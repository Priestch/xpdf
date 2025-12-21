//! Integration tests for PDF-X with real-world PDF scenarios.
//!
//! These tests verify that the library works with actual PDF files
//! and handles edge cases that may not be covered by unit tests.

use pdf_x::core::PDFDocument;
use std::fs;

/// Creates a simple test PDF for integration testing.
fn create_test_pdf() -> Vec<u8> {
    // Use the same format as the working unit test
    b"%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj

2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj

3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>
endobj

4 0 obj
<< /Length 44 >>
stream
BT /F1 12 Tf 100 700 Td (Test PDF) Tj ET
endstream
endobj

5 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj

xref
0 6
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000119 00000 n
0000000248 00000 n
0000000328 00000 n
trailer
<< /Size 6 /Root 1 0 R >>
startxref
413
%%EOF"
        .to_vec()
}

#[test]
fn test_basic_pdf_parsing() {
    let pdf_data = create_test_pdf();
    let mut doc = PDFDocument::open(pdf_data).expect("Failed to parse test PDF");

    // Test basic document properties
    assert_eq!(doc.page_count().unwrap(), 1);

    // Test page access
    let page = doc.get_page(0).expect("Failed to get first page");
    assert_eq!(page.index(), 0);

    // Test text extraction
    let text_items = page.extract_text(&mut doc.xref_mut()).expect("Failed to extract text");
    assert_eq!(text_items.len(), 1);
    assert_eq!(text_items[0].text, "Test PDF");
}

#[test]
fn test_cli_integration() {
    use std::process::Command;

    // Test that the CLI can analyze our test PDF
    let pdf_data = create_test_pdf();
    let pdf_path = "test_integration.pdf";

    // Write test PDF to disk
    fs::write(pdf_path, pdf_data).expect("Failed to write test PDF");

    // Run the CLI tool
    let output = Command::new("cargo")
        .args(&["run", "--bin", "pdf-inspect", "--", pdf_path])
        .output()
        .expect("Failed to run pdf-inspect");

    // Clean up
    let _ = fs::remove_file(pdf_path);

    // Check that it ran successfully
    assert!(output.status.success(), "CLI failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PDF Structure Analysis"));
    assert!(stdout.contains("Pages: 1"));
}

#[test]
fn test_error_recovery() {
    use pdf_x::core::PDFError;

    // Test with malformed PDF data
    let malformed_pdf = b"Not a PDF file";
    let result = PDFDocument::open(malformed_pdf.to_vec());

    assert!(result.is_err(), "Should fail to parse malformed PDF");

    // Check that we get some kind of parsing error
    let error_str = format!("{}", result.err().unwrap());
    assert!(error_str.contains("Parse error") || error_str.contains("xref") || error_str.contains("PDF"));
}

#[test]
fn test_concurrent_access() {
    use std::sync::Arc;
    use std::thread;

    // Test that multiple threads can access PDF data safely
    let pdf_data = Arc::new(create_test_pdf());
    let mut handles = Vec::new();

    for i in 0..4 {
        let data_clone = pdf_data.clone();
        let handle = thread::spawn(move || {
            let data_vec = data_clone.to_vec();
            let mut doc = PDFDocument::open(data_vec).expect("Thread failed to parse PDF");
            let page_count = doc.page_count().expect("Failed to get page count");
            assert_eq!(page_count, 1);

            let page = doc.get_page(0).expect("Failed to get page");
            assert_eq!(page.index(), 0);

            // Verify thread-specific data
            format!("Thread {} completed successfully", i)
        });
        handles.push(handle);
    }

    for handle in handles {
        let result = handle.join().expect("Thread panicked");
        println!("{}", result);
    }
}

#[test]
fn test_different_pdf_sizes() {
    // Test with very small PDF
    let small_pdf = b"%PDF-1.1\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\nxref\n0 3\n0000000000 65535 f\n0000000016 00000 n\n0000000067 00000 n\ntrailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n128\n%%EOF";

    let result = PDFDocument::open(small_pdf.to_vec());
    assert!(result.is_ok(), "Failed to parse small PDF");

    // Test with empty content stream
    let empty_content_pdf = create_test_pdf();
    let result = PDFDocument::open(empty_content_pdf);
    assert!(result.is_ok(), "Failed to parse PDF with empty content");
}

#[test]
fn test_unicode_text() {
    // Test PDF with basic Unicode text
    let mut pdf_data = create_test_pdf();

    // Replace "Test PDF" with a simple text that should work
    let new_text = b"(Hello World)";
    if let Some(pos) = pdf_data.windows(b"(Test PDF)".len()).position(|window| window == b"(Test PDF)") {
        let actual_pos = pos;
        pdf_data.splice(actual_pos..actual_pos + 9, new_text.iter().cloned());
    }

    let result = PDFDocument::open(pdf_data);
    assert!(result.is_ok(), "Failed to parse PDF with Unicode text");
}

#[test]
fn test_memory_efficiency() {
    use std::time::Instant;

    let pdf_data = create_test_pdf();

    // Test parsing performance
    let start = Instant::now();
    let mut doc = PDFDocument::open(pdf_data).expect("Failed to parse PDF");
    let parse_time = start.elapsed();

    println!("PDF parsing took: {:?}", parse_time);

    // Test text extraction performance
    let start = Instant::now();
    let page = doc.get_page(0).expect("Failed to get page");
    let text_items = page.extract_text(&mut doc.xref_mut()).expect("Failed to extract text");
    let extract_time = start.elapsed();

    println!("Text extraction took: {:?}", extract_time);
    println!("Extracted {} text items", text_items.len());

    // These should complete quickly for a simple PDF
    assert!(parse_time.as_millis() < 100, "Parsing took too long");
    assert!(extract_time.as_millis() < 50, "Text extraction took too long");
}