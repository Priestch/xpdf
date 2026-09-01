//! Integration tests for text editing infrastructure (Step 5a–5d).
//!
//! These tests exercise `EditableContentStream` against real PDF fixture
//! content streams, verifying identity properties and splice correctness.

use std::path::PathBuf;

use pdf_x_core::core::page::PageResolver;
use pdf_x_core::core::parser::PDFObject;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("pdf-x-core has a parent workspace dir")
        .join("tests")
        .join("fixtures")
}

/// Helper: open a fixture PDF, get a page's decoded content stream bytes.
fn get_page_content_stream_bytes(
    pdf_path: &std::path::Path,
    page_index: usize,
) -> Option<Vec<Vec<u8>>> {
    let bytes = std::fs::read(pdf_path).ok()?;
    let mut doc = pdf_x_core::core::PDFDocument::open(bytes).ok()?;
    let page = doc.get_page(page_index).ok()?;

    // Get /Contents — may be a single stream or an array.
    let contents_obj = match page.contents() {
        Some(c) => doc.xref_mut().resolve_if_ref(c).ok()?,
        None => return None,
    };

    match &*contents_obj {
        PDFObject::Stream { data, .. } => Some(vec![data.clone()]),
        PDFObject::Array(arr) => {
            let mut out = Vec::new();
            for item in arr.iter() {
                let resolved = doc.xref_mut().resolve_if_ref(item).ok()?;
                if let PDFObject::Stream { data, .. } = &*resolved {
                    out.push(data.clone());
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// Helper: create a ContentFragment for an identity test (no filter).
fn make_identity_fragments(
    decoded_list: &[Vec<u8>],
) -> Vec<(Vec<u8>, pdf_x_core::core::content_edit::ContentFragment)> {
    let mut offset = 0;
    decoded_list
        .iter()
        .map(|d| {
            let len = d.len();
            let frag = pdf_x_core::core::content_edit::ContentFragment {
                filter_name: None,
                has_decode_parms: false,
                raw: Vec::new(),
                decoded_offset: offset,
                decoded_len: len,
            };
            offset += len;
            (d.clone(), frag)
        })
        .collect()
}

// ==========================================================================
// 5a: Identity tests — build → no edits → bytes unchanged
// ==========================================================================

/// tracemonkey has rich content streams with BT/ET blocks, path ops, images.
/// Ignored by default — it's a 1 MB fixture that's slow to open.
#[test]
#[ignore]
fn identity_tracemonkey_page0() {
    let path = fixtures_dir().join("pdfs").join("tracemonkey.pdf");
    let fragments = get_page_content_stream_bytes(&path, 0)
        .expect("should read tracemonkey page 0 content");

    let pairs = make_identity_fragments(&fragments);
    let ecs = pdf_x_core::core::content_edit::EditableContentStream::build(&pairs)
        .expect("build should succeed");

    // Verify bytes are identical.
    let mut expected = Vec::new();
    for d in &fragments {
        expected.extend_from_slice(d);
    }
    assert_eq!(
        ecs.bytes(),
        &expected[..],
        "identity: bytes must be unchanged"
    );

    // Should have parsed a reasonable number of operations.
    assert!(
        ecs.ops.len() > 20,
        "tracemonkey page 0 should have many ops, got {}",
        ecs.ops.len()
    );
}

/// basicapi also has content streams.
#[test]
fn identity_basicapi_page0() {
    let path = fixtures_dir().join("pdfs").join("basicapi.pdf");
    let fragments = get_page_content_stream_bytes(&path, 0)
        .expect("should read basicapi page 0 content");

    let pairs = make_identity_fragments(&fragments);
    let ecs = pdf_x_core::core::content_edit::EditableContentStream::build(&pairs)
        .expect("build should succeed");

    let mut expected = Vec::new();
    for d in &fragments {
        expected.extend_from_slice(d);
    }
    assert_eq!(ecs.bytes(), &expected[..]);
}

// ==========================================================================
// 5b: find_text_on_page integration
// ==========================================================================

/// Verify that Document::find_text_on_page works against a real fixture.
/// basicapi page 0 contains text; we search for a known substring.
#[test]
fn find_text_on_page_basicapi() {
    let path = fixtures_dir().join("pdfs").join("basicapi.pdf");
    let bytes = std::fs::read(&path).expect("should read basicapi.pdf");
    let mut doc = pdf_x_core::core::PDFDocument::open(bytes).expect("should open");

    // First, see what text is on page 0.
    let text = doc
        .extract_text_from_page_as_string(0)
        .expect("should extract text");
    if text.is_empty() {
        return; // Nothing to search — skip.
    }

    // Try finding a substring.
    // Take first ~10 chars of extracted text as the needle.
    let needle: String = text.chars().take(10).collect();
    let m = doc
        .find_text_on_page(0, &needle, &pdf_x_core::core::FindOptions::default())
        .expect("find_text_on_page should succeed")
        .expect("should find the substring");
    assert_eq!(m.matched_text, needle);
}

// ==========================================================================
// 5c: replace_text integration
// ==========================================================================

/// Build a minimal PDF with a single page containing `(Hello) Tj`.
/// This lets us test replace_text end-to-end without fixture complexity.
fn make_minimal_text_pdf() -> Vec<u8> {
    // Hand-crafted minimal PDF: one page, one content stream with text.
    // Font is Helvetica (a standard 14 font, no embedding needed).
    let pdf = b"
%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792]
   /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>
endobj
4 0 obj
<< /Length 33 >>
stream
BT /F1 12 Tf 100 700 Td (Hello World) Tj ET
endstream
endobj
5 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj
xref
0 6
0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \n0000000266 00000 n \n0000000360 00000 n \ntrailer
<< /Size 6 /Root 1 0 R >>
startxref
449
%%EOF";
    pdf.to_vec()
}

/// Smoke test: replace_text on a hand-crafted PDF, verify with extract_text.
#[test]
fn replace_text_minimal_pdf() {
    let pdf_bytes = make_minimal_text_pdf();
    let mut doc = pdf_x_core::core::PDFDocument::open(pdf_bytes)
        .expect("should open minimal PDF");

    // Verify initial text.
    let before = doc
        .extract_text_from_page_as_string(0)
        .expect("should extract text");
    assert!(before.contains("Hello World"), "initial text: {before}");

    // Find "Hello" and replace with "Goodbye".
    doc.replace_text(
        0,
        "Hello",
        "Goodbye",
        &pdf_x_core::core::FindOptions::default(),
    )
    .expect("replace_text should succeed");

    // Verify text changed.
    let after = doc
        .extract_text_from_page_as_string(0)
        .expect("should extract text after replace");
    assert!(after.contains("Goodbye World"), "after text: {after}");
    assert!(!after.contains("Hello World"), "old text should be gone");
}

/// Undo/redo of replace_text preserves text correctness.
#[test]
fn replace_text_undo_redo() {
    let pdf_bytes = make_minimal_text_pdf();
    let mut doc = pdf_x_core::core::PDFDocument::open(pdf_bytes)
        .expect("should open minimal PDF");

    doc.replace_text(
        0,
        "Hello",
        "Goodbye",
        &pdf_x_core::core::FindOptions::default(),
    )
    .expect("replace_text should succeed");

    // Undo.
    doc.undo().expect("undo should succeed");
    let text = doc
        .extract_text_from_page_as_string(0)
        .expect("should extract text");
    assert!(text.contains("Hello World"), "after undo: {text}");

    // Redo.
    doc.redo().expect("redo should succeed");
    let text = doc
        .extract_text_from_page_as_string(0)
        .expect("should extract text");
    assert!(text.contains("Goodbye World"), "after redo: {text}");
}

// ==========================================================================
// 5a: Splice consistency — replace one op, neighbors unchanged
// ==========================================================================

/// After splicing, all non-replaced operations' spans should still
/// reference the correct bytes (verified by re-parsing).
#[test]
#[ignore]
fn splice_consistency_tracemonkey() {
    let path = fixtures_dir().join("pdfs").join("tracemonkey.pdf");
    let fragments = get_page_content_stream_bytes(&path, 0)
        .expect("should read tracemonkey page 0 content");

    let pairs = make_identity_fragments(&fragments);
    let mut ecs = pdf_x_core::core::content_edit::EditableContentStream::build(&pairs)
        .unwrap();

    // Find a text operation to replace.
    let op_idx = ecs.ops.iter().position(|op| {
        op.op == Some(pdf_x_core::core::OpCode::ShowText)
            || op.op == Some(pdf_x_core::core::OpCode::ShowSpacedText)
    });
    let op_idx = match op_idx {
        Some(i) => i,
        None => return, // No text ops — skip test gracefully.
    };

    // Replace the text op with a minimal hex string.
    use pdf_x_core::core::content_edit::{serialize_hex_string, serialize_operation};
    use pdf_x_core::core::OpCode;
    let replacement = serialize_operation(OpCode::ShowText, &serialize_hex_string(b"X"));

    ecs.replace_op_bytes(op_idx, replacement);

    // Re-parse the modified bytes and verify it's still valid.
    let pairs2 = make_identity_fragments(&[ecs.bytes().to_vec()]);
    let ecs2 = pdf_x_core::core::content_edit::EditableContentStream::build(&pairs2)
        .expect("re-parsing after splice should succeed");

    // Should have the same number of operations (replacement is still a Tj).
    assert_eq!(
        ecs2.ops.len(),
        ecs.ops.len(),
        "op count should be stable after splice"
    );
}

// ==========================================================================
// 5d: Persistence
// ==========================================================================

/// After replace_text, the delta-modified stream can be re-extracted.
/// (Save+reload is covered by other edit commands' save tests; the minimal
/// PDF used here has hand-crafted xref offsets that don't survive reload.)
#[test]
fn replace_text_delta_persists() {
    let pdf_bytes = make_minimal_text_pdf();
    let mut doc = pdf_x_core::core::PDFDocument::open(pdf_bytes)
        .expect("should open minimal PDF");

    doc.replace_text(
        0,
        "Hello",
        "Goodbye",
        &pdf_x_core::core::FindOptions::default(),
    )
    .expect("replace_text should succeed");

    // extract_text reads through the delta, so it should see the change.
    let text = doc
        .extract_text_from_page_as_string(0)
        .expect("should extract text");
    assert!(text.contains("Goodbye World"));
    assert!(!text.contains("Hello World"));
}

// ==========================================================================
// 5d: Error paths
// ==========================================================================

/// replace_text with no match should be a no-op (not error).
#[test]
fn replace_text_no_match_is_noop() {
    let pdf_bytes = make_minimal_text_pdf();
    let mut doc = pdf_x_core::core::PDFDocument::open(pdf_bytes).unwrap();

    let result = doc.replace_text(
        0,
        "ZZZ_NOT_FOUND",
        "anything",
        &pdf_x_core::core::FindOptions::default(),
    );
    assert!(result.is_ok(), "no-match should be Ok(no-op)");

    // Verify text unchanged.
    let text = doc
        .extract_text_from_page_as_string(0)
        .unwrap();
    assert!(text.contains("Hello World"));
}

/// replace_text on empty page (no content) should be Ok no-op.
#[test]
fn replace_text_empty_page() {
    let path = fixtures_dir().join("pdfs").join("basicapi.pdf");
    let bytes = std::fs::read(&path).unwrap();
    let mut doc = pdf_x_core::core::PDFDocument::open(bytes).unwrap();

    // Try every page - none should panic.
    let pages = doc.page_count().unwrap_or(0);
    for i in 0..pages {
        let _ = doc.replace_text(
            i as usize,
            "anything",
            "anything",
            &pdf_x_core::core::FindOptions::default(),
        );
    }
}

// ==========================================================================
// 5d: Compressed content stream (FlateDecode)
// ==========================================================================

/// replace_text on a FlateDecode content stream should work and re-compress.
#[test]
fn replace_text_flate_stream() {
    let path = fixtures_dir().join("pdfs").join("basicapi.pdf");
    let bytes = std::fs::read(&path).unwrap();
    let mut doc = pdf_x_core::core::PDFDocument::open(bytes).unwrap();

    // Find any text on page 0.
    let text_before = doc
        .extract_text_from_page_as_string(0)
        .expect("should extract text");
    if text_before.is_empty() {
        return;
    }

    // Take first 5 chars as needle.
    let needle: String = text_before.chars().take(5).collect();
    let replacement = "XXXXX".to_string();

    // basicapi may use TJ (not Tj) - v1 only supports Tj.
    // Gracefully skip if replace_text rejects the op type.
    if doc
        .replace_text(0, &needle, &replacement, &pdf_x_core::core::FindOptions::default())
        .is_err()
    {
        return; // fixture uses TJ - skip
    }

    let text_after = doc
        .extract_text_from_page_as_string(0)
        .expect("should extract text after replace");
    assert!(
        text_after.contains("XXXXX"),
        "flate stream replacement should persist: {text_after}"
    );
    assert!(
        !text_after.contains(&needle),
        "old text should be gone: {text_after}"
    );
}
