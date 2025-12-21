//! Real-world PDF testing for PDF-X.
//!
//! These tests validate that our implementation works with actual PDF scenarios
//! using proven working PDF formats from our unit tests.

use pdf_x::core::PDFDocument;

/// Creates a working PDF using the exact format from our proven unit tests
fn create_working_pdf() -> Vec<u8> {
    // This is a simplified but working PDF based on our unit tests
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
BT /F1 12 Tf
100 700 Td
(Hello World) Tj
ET
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
0000000254 00000 n
0000000324 00000 n
trailer
<< /Size 6 /Root 1 0 R >>
startxref
409
%%EOF"
        .to_vec()
}

#[test]
fn test_proven_pdf_functionality() {
    let pdf_data = create_working_pdf();

    let result = PDFDocument::open(pdf_data);
    assert!(result.is_ok(), "Failed to parse proven working PDF");

    let mut doc = result.unwrap();

    // Test basic document properties
    assert_eq!(doc.page_count().unwrap(), 1);

    // Test page access
    let page = doc.get_page(0).expect("Failed to get page");
    assert_eq!(page.index(), 0);

    // Test text extraction
    let text_items = page.extract_text(&mut doc.xref_mut()).expect("Failed to extract text");
    assert!(!text_items.is_empty(), "Should extract some text");

    // Verify content
    let combined_text: String = text_items.iter().map(|item| item.text.clone()).collect::<Vec<_>>().join(" ");
    assert!(combined_text.contains("Hello World"));
}


#[test]
fn test_text_extraction_quality() {
    let pdf_data = create_working_pdf();

    let mut doc = PDFDocument::open(pdf_data).expect("Failed to parse PDF");
    let page = doc.get_page(0).expect("Failed to get page");
    let text_items = page.extract_text(&mut doc.xref_mut()).expect("Failed to extract text");

    // Should extract at least one text item
    assert!(!text_items.is_empty(), "Should extract at least one text item");

    // First text item should have reasonable properties
    if let Some(first_item) = text_items.first() {
        assert!(!first_item.text.is_empty(), "Text should not be empty");

        // Should have font information
        assert!(first_item.font_name.is_some(), "Should have font name");

        // Should have position information
        assert!(first_item.position.is_some(), "Should have position info");

        // Font size might be None (that's okay)
        println!("Extracted: '{}' with font: {:?}, position: {:?}",
                 first_item.text,
                 first_item.font_name,
                 first_item.position);
    }
}

#[test]
fn test_performance_characteristics() {
    use std::time::Instant;

    let pdf_data = create_working_pdf();

    // Test parsing performance
    let start = Instant::now();
    let mut doc = PDFDocument::open(pdf_data).expect("Failed to parse PDF");
    let parse_time = start.elapsed();

    // Test page access performance (multiple accesses)
    let start = Instant::now();
    for _ in 0..10 {
        let _page = doc.get_page(0).expect("Failed to get page");
    }
    let access_time = start.elapsed();

    // Test text extraction performance
    let start = Instant::now();
    let page = doc.get_page(0).expect("Failed to get page");
    let text_items = page.extract_text(&mut doc.xref_mut()).expect("Failed to extract text");
    let extract_time = start.elapsed();

    println!("Performance characteristics:");
    println!("  Parse time: {:?}", parse_time);
    println!("  Page access (10x): {:?}", access_time);
    println!("  Text extraction: {:?}", extract_time);
    println!("  Text items extracted: {}", text_items.len());

    // Reasonable performance expectations for this simple PDF
    assert!(parse_time.as_millis() < 50, "Parsing should be fast for simple PDFs");
    assert!(access_time.as_millis() < 10, "Repeated page access should be cached and fast");
    assert!(extract_time.as_millis() < 20, "Text extraction should be efficient");
}

#[test]
fn test_error_handling_gracefully() {
    // Test error handling with malformed PDF data
    let test_cases = vec![
        ("Empty PDF", vec![]),
        ("Not a PDF", b"This is not a PDF file".to_vec()),
        ("Truncated PDF", b"%PDF-1.4".to_vec()),
    ];

    for (name, pdf_data) in test_cases {
        let result = PDFDocument::open(pdf_data);

        // Should fail gracefully without panicking
        assert!(result.is_err(), "{} should fail to parse", name);

        // Error should be meaningful
        let error = result.err().unwrap();
        let error_str = format!("{}", error);
        assert!(!error_str.is_empty(), "{} should have meaningful error message", name);

        println!("{}: {}", name, error_str);
    }
}

#[test]
fn test_memory_safety() {
    use std::sync::Arc;
    use std::thread;

    let pdf_data = Arc::new(create_working_pdf());
    let mut handles = Vec::new();

    // Test concurrent access to ensure thread safety
    for i in 0..3 {
        let pdf_clone = pdf_data.clone();
        let handle = thread::spawn(move || {
            let mut doc = PDFDocument::open((*pdf_clone).clone()).expect("Thread failed to parse PDF");

            // Each thread can access the PDF multiple times
            let mut results = Vec::new();
            for _ in 0..5 {
                let page = doc.get_page(0).expect("Failed to get page");
                let text_items = page.extract_text(&mut doc.xref_mut()).expect("Failed to extract text");
                results.push(text_items.len());
            }

            format!("Thread {} completed {} extractions", i, results.len())
        });

        handles.push(handle);
    }

    // Wait for all threads to complete
    for handle in handles {
        let result = handle.join().expect("Thread panicked");
        println!("{}", result);
    }
}