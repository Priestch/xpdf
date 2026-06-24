/// Regression: the repo's sample.pdf carries a `startxref` value (548) that points
/// mid-table — the `xref` keyword is actually ~25 bytes earlier. Before the recovery
/// path, `open` failed ("got Command('f')"). It must now open via backward-scan recovery.
#[test]
fn sample_pdf_opens_after_startxref_recovery() {
    let data = include_bytes!("../../sample.pdf");
    let mut doc = pdf_x_core::PDFDocument::open(data.to_vec())
        .expect("sample.pdf should open via startxref recovery");
    // The recovery's job is to land the parser on a valid xref/object header so `open`
    // succeeds; deeper per-object parse robustness on this particular fixture is tracked
    // separately.
    assert!(doc.page_count().is_ok(), "page_count should not error after open");
}
