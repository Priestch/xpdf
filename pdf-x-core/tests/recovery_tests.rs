//! Recovery-scan integration tests: a PDF whose tail (xref table + startxref +
//! %%EOF) has been truncated must still open via the full-file recovery scan
//! (`XRef::recover_by_scan`, porting pdf.js `indexObjects`).

use pdf_x_core::core::PDFDocument;

fn fixtures_pdfs() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("pdfs")
}

/// Strip everything from the last `startxref` onward (simulating a truncated tail:
/// no xref, no startxref, no %%EOF).
fn truncate_at_last_startxref(data: &[u8]) -> Vec<u8> {
    if let Some(i) = data
        .windows(b"startxref".len())
        .rposition(|w| w == b"startxref")
    {
        data[..i].to_vec()
    } else {
        data.to_vec()
    }
}

#[test]
fn truncated_pdf_opens_via_recovery_scan() {
    let path = fixtures_pdfs().join("basicapi.pdf");
    let original = std::fs::read(&path).expect("basicapi.pdf fixture exists");

    // The intact document must open (baseline) and report a known page count.
    let intact_pages = PDFDocument::open(original.clone())
        .expect("intact basicapi.pdf opens")
        .page_count()
        .expect("intact page_count");

    // Now truncate the tail and confirm recovery still opens it with the same page count.
    let truncated = truncate_at_last_startxref(&original);
    assert!(
        truncated.len() < original.len(),
        "truncation should have removed the tail"
    );
    assert!(
        !truncated
            .windows(b"startxref".len())
            .any(|w| w == b"startxref"),
        "truncated buffer must have no startxref"
    );

    let recovered_pages = PDFDocument::open(truncated)
        .expect("truncated PDF opens via full-file recovery scan")
        .page_count()
        .expect("recovered page_count");

    assert_eq!(
        intact_pages, recovered_pages,
        "recovery must restore the same page count as the intact file"
    );
}
