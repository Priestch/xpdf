//! Malformed PDF tests for robustness validation.
//!
//! These tests ensure that PDF-X handles malformed PDFs gracefully without panicking.
//! Based on PDF.js's malformed PDF handling and error recovery tests.

mod test_utils;

use pdf_x_core::core::*;
use test_utils::*;

// ============================================================================
// Malformed XRef Table Tests
// ============================================================================

#[test]
fn test_invalid_xref_table() {
    // XRef table with invalid format
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog >>\nendobj\n\
xref\n\
INVALID DATA HERE\n\
trailer\n<< /Size 1 /Root 1 0 R >>\n\
startxref\n0\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    // Should handle gracefully - either return error or minimal valid PDF
    assert!(result.is_err() || result.is_ok(), "Should not panic on invalid xref");
}

#[test]
fn test_truncated_xref() {
    // XRef table that ends unexpectedly
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog >>\nendobj\n\
xref\n\
0 1\n\
0000000000 65535 f\n\
trailer\n\
%%EOF";  // Missing startxref

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle truncated xref without panic");
}

#[test]
fn test_corrupted_xref_offset() {
    // XRef with invalid offset (points beyond file)
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog >>\nendobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
9999999999 00000 n\n\
trailer\n<< /Size 2 /Root 1 0 R >>\n\
startxref\n50\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle corrupted offset without panic");
}

// ============================================================================
// Malformed Object Tests
// ============================================================================

#[test]
fn test_malformed_dictionary() {
    // Dictionary with missing closing >>
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog\n\
2 0 obj\n<< /Type /Pages >>\nendobj\n\
xref\n\
0 3\n\
0000000000 65535 f\n\
0000000020 00000 n\n\
0000000050 00000 n\n\
trailer\n<< /Size 3 /Root 1 0 R >>\n\
startxref\n70\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle malformed dict without panic");
}

#[test]
fn test_unmatched_array_bracket() {
    // Array with missing closing bracket
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n[1 2 3\n\
endobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000010 00000 n\n\
trailer\n<< /Size 2 >>\n\
startxref\n40\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle unmatched bracket without panic");
}

#[test]
fn test_invalid_hex_string() {
    // Hex string with invalid characters
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<GGGGGG>\n\
endobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000010 00000 n\n\
trailer\n<< /Size 2 >>\n\
startxref\n30\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle invalid hex string without panic");
}

#[test]
fn test_invalid_string_escape() {
    // String with invalid escape sequence
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n(Invalid escape: \\xZZ)\n\
endobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000010 00000 n\n\
trailer\n<< /Size 2 >>\n\
startxref\n40\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle invalid escape without panic");
}

// ============================================================================
// Malformed Stream Tests
// ============================================================================

#[test]
fn test_truncated_stream() {
    // Stream with /Length but actual data is shorter
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Length 100 >>\n\
stream\n\
SHORT DATA\n\
endstream\n\
endobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000030 00000 n\n\
trailer\n<< /Size 2 >>\n\
startxref\n70\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle truncated stream without panic");
}

#[test]
fn test_stream_without_keywords() {
    // Stream data without stream/endstream keywords
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Length 10 >>\n\
NOT A STREAM\n\
endobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000020 00000 n\n\
trailer\n<< /Size 2 >>\n\
startxref\n50\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle missing stream keywords without panic");
}

#[test]
fn test_invalid_stream_filter() {
    // Stream with unknown filter
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Filter /UnknownFilter /Length 5 >>\n\
stream\n\
TEST\n\
endstream\n\
endobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000040 00000 n\n\
trailer\n<< /Size 2 >>\n\
startxref\n80\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle unknown filter gracefully");
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn test_empty_pdf() {
    // Completely empty file
    let pdf_data = b"";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err(), "Empty file should be rejected");
}

#[test]
fn test_not_a_pdf() {
    // File without PDF header
    let pdf_data = b"This is not a PDF file. Just plain text.";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err(), "Non-PDF file should be rejected");
}

#[test]
fn test_pdf_version_too_high() {
    // PDF with future version number
    let pdf_data = b"%PDF-9.9\n\
1 0 obj\n<< /Type /Catalog >>\nendobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000020 00000 n\n\
trailer\n<< /Size 2 /Root 1 0 R >>\n\
startxref\n50\n%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    // Should handle gracefully - may reject or attempt to parse
    assert!(result.is_err() || result.is_ok(), "Should handle future version without panic");
}

#[test]
fn test_missing_trailer() {
    // PDF without trailer dictionary
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog >>\nendobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000020 00000 n\n\
startxref\n50\n\
%%EOF";  // No trailer

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle missing trailer without panic");
}

#[test]
fn test_missing_root_in_trailer() {
    // Trailer without /Root reference
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog >>\nendobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000020 00000 n\n\
trailer\n<< /Size 2 >>\n\
startxref\n50\n\
%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err(), "Missing /Root should be rejected");
}

#[test]
fn test_circular_object_reference() {
    // Objects that reference each other in a cycle
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [1 0 R] /Count 1 >>\nendobj\n\
xref\n\
0 3\n\
0000000000 65535 f\n\
0000000025 00000 n\n\
0000000070 00000 n\n\
trailer\n<< /Size 3 /Root 1 0 R >>\n\
startxref\n95\n\
%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    // Circular reference should be detected or handled
    assert!(result.is_ok() || result.is_err(), "Should handle circular reference without panic");
}

// ============================================================================
// Tests with Generated Bad PDFs
// ============================================================================

#[test]
fn test_zero_length_object() {
    // Object with zero length
    let pdf_data = b"%PDF-1.4\n\
1 0 obj\n\
endobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000010 00000 n\n\
trailer\n<< /Size 2 >>\n\
startxref\n20\n\
%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    // Empty object should be handled
    assert!(result.is_ok() || result.is_err(), "Should handle empty object");
}

#[test]
fn test_object_number_mismatch() {
    // Object number doesn't match actual position
    let pdf_data = b"%PDF-1.4\n\
99 0 obj\n<< /Type /Catalog >>\nendobj\n\
xref\n\
0 2\n\
0000000000 65535 f\n\
0000000015 00000 n\n\
trailer\n<< /Size 100 /Root 99 0 R >>\n\
startxref\n60\n\
%%EOF";

    let result = PDFDocument::open(pdf_data.to_vec());
    assert!(result.is_err() || result.is_ok(), "Should handle object number mismatch");
}

// ============================================================================
// Recovery Tests - Using bad-xref.pdf
// ============================================================================

#[test]
fn test_bad_xref_pdf_loading() {
    // Test the generated bad-xref.pdf file
    if !test_pdf_exists("bad-xref.pdf") {
        println!("Skipping test: bad-xref.pdf not found");
        return;
    }

    let result = assert_pdf_loads("bad-xref.pdf");
    // The bad-xref.pdf has an invalid startxref, so it should fail gracefully
    assert!(result.is_err(), "bad-xref.pdf should fail to load but not panic");
}

// ============================================================================
// Stress Tests
// ============================================================================

#[test]
fn test_extremely_deep_nesting() {
    // Very deep nesting of arrays/dictionaries
    let mut pdf = String::from("%PDF-1.4\n1 0 obj\n");

    // Create deeply nested structure
    pdf.push_str("<< /Root ");
    for _ in 0..50 {
        pdf.push_str("<< /Nested ");
    }
    pdf.push_str("1 >>");
    for _ in 0..50 {
        pdf.push_str(">>");
    }
    pdf.push_str(" >>\nendobj\nxref\n0 2\n0000000000 65535 f\n0000000005 00000 n\n");
    pdf.push_str("trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n");
    pdf.push_str(&format!("{}\n", pdf.len()));
    pdf.push_str("%%EOF");

    let result = PDFDocument::open(pdf.as_bytes().to_vec());
    // Should handle deep nesting without stack overflow
    assert!(result.is_ok() || result.is_err(), "Should handle deep nesting without panic");
}
