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

/// Linearized Illustrator export (`issue53.pdf`) whose first-page xref's `/Prev`
/// points into the middle of the classic table (lands on free-flag `f`). The
/// partial first-page section omits object 1 (`/Pages`); lazy fetch must
/// `recover_by_scan` and still resolve the page tree. Content streams are
/// separately broken-flate and recovered by the tolerant flate fallback.
#[test]
fn issue53_linearized_broken_prev_opens_via_lazy_recovery() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("hayro/hayro-tests/pdfs/custom/issue53.pdf");
    if !path.exists() {
        // hayro submodule not checked out in this environment — skip.
        eprintln!("skip: {} not present", path.display());
        return;
    }
    let data = std::fs::read(&path).expect("issue53.pdf readable");
    let mut doc = PDFDocument::open(data).expect("issue53 opens despite broken /Prev xref");
    let pages = doc.page_count().expect("page_count after lazy recovery");
    assert_eq!(pages, 1, "issue53 is a single-page document");
    // Touch the page tree so object 1 is actually fetched (triggers recovery).
    let _page = doc
        .get_page(0)
        .expect("page 0 resolves after ObjectNotInXRef recovery");
}

/// Multi-fragment /Contents arrays must extract as one program (fragments can
/// end mid-operator). Regression for font_truetype_slow_post_lookup.pdf.
#[test]
fn multi_contents_array_extracts_without_trailing_operand_error() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("hayro/hayro-tests/pdfs/custom/font_truetype_slow_post_lookup.pdf");
    if !path.exists() {
        return;
    }
    let data = std::fs::read(&path).expect("read");
    let mut doc = PDFDocument::open(data).expect("open");
    let text = doc
        .extract_text_from_page_as_string(0)
        .expect("extract must survive mid-stream Content splits");
    assert!(
        text.len() > 100,
        "expected substantial extracted text, got {} chars",
        text.len()
    );
}
