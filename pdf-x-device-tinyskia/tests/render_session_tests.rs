//! Parity gate for [`RenderSession`] (Phase E): rendering an immutable
//! `ResolvedPage` snapshot outside the document must produce byte-identical
//! pixels to the doc-locked `render_page_to_image` path — this is what lets
//! the shell drop its document lock across renders with no visual drift.

use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::{RenderSession, RenderToImage};

fn fixtures_pdfs() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("pdfs")
}

fn open_fixture(name: &str) -> Option<PDFDocument> {
    let bytes = std::fs::read(fixtures_pdfs().join(name)).ok()?;
    PDFDocument::open(bytes).ok()
}

#[test]
fn session_matches_doc_locked_render() {
    // basicapi.pdf exercises embedded fonts, images, and graphics operators.
    let Some(mut doc) = open_fixture("basicapi.pdf") else {
        return; // fixture absent in this checkout — nothing to compare
    };
    let Some(count) = doc.page_count().ok() else {
        return;
    };
    let scale = Some(1.5f32);

    for page in 0..count.min(3) as usize {
        let expected = doc
            .render_page_to_image(page, scale)
            .unwrap_or_else(|e| panic!("page {page}: doc-locked render: {e}"));

        // Fresh session (cold cache) and a warm re-render of the same session
        // must both match the doc path byte-for-byte.
        let snapshot = doc
            .resolve_page(page)
            .unwrap_or_else(|e| panic!("page {page}: resolve: {e}"));
        let mut session = RenderSession::new();
        let cold = session
            .render(&snapshot, scale)
            .unwrap_or_else(|e| panic!("page {page}: session render: {e}"));
        let warm = session
            .render(&snapshot, scale)
            .unwrap_or_else(|e| panic!("page {page}: warm session render: {e}"));

        assert_eq!(expected, cold, "page {page}: cold session bytes differ");
        assert_eq!(expected, warm, "page {page}: warm session bytes differ");
    }
}

#[test]
fn sessions_render_concurrently_to_identical_output() {
    // The shape the shell uses: resolve under a short lock, then render the
    // snapshots on parallel threads, each with its own session.
    let Some(mut doc) = open_fixture("basicapi.pdf") else {
        return;
    };
    let Some(count) = doc.page_count().ok() else {
        return;
    };
    let scale = Some(1.0f32);
    let pages: Vec<usize> = (0..count.min(4) as usize).collect();

    let expected: Vec<_> = pages
        .iter()
        .map(|&p| doc.render_page_to_image(p, scale).expect("doc render"))
        .collect();
    let snapshots: Vec<_> = pages
        .iter()
        .map(|&p| doc.resolve_page(p).expect("resolve"))
        .collect();

    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = snapshots
            .into_iter()
            .map(|snap| {
                s.spawn(move || {
                    let mut session = RenderSession::new();
                    session.render(&snap, scale).expect("session render")
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("render thread panicked"))
            .collect()
    });

    assert_eq!(expected, results, "parallel session output differs");
}
