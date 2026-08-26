//! Equivalence tests for the progressive [`DocumentLoader`] (Phase D step 1).
//!
//! The loader must produce the same document as the buffered
//! `PDFDocument::open` path for every fixture that opens at all: same page
//! count, same linearization/encryption flags. This is the gate that lets
//! later steps converge `open`/`open_file` onto the loader without behavior
//! drift.

#![cfg(feature = "rendering")]

use pdf_x_core::PDFDocument;
use pdf_x_core::core::document_loader::{DocumentLoader, OpenProgress};
use pdf_x_core::core::stream::Stream;
use pdf_x_source_file::FileChunkedStream;

fn fixtures_pdfs() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("pdfs")
}

fn load(name: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(fixtures_pdfs().join(name))
}

/// Drive a loader to completion, stepping through any Needs (a file-backed
/// source satisfies them synchronously, so at most a few rounds occur).
/// A repeated identical Need is a bug (the source isn't making progress) and
/// aborts rather than spinning — the same guard `drive` applies internally.
fn open_via_loader(
    loader: DocumentLoader<FileChunkedStream>,
) -> pdf_x_core::core::error::PDFResult<PDFDocument> {
    let mut loader = loader;
    let mut last: Option<pdf_x_core::core::byte_source::Need> = None;
    loop {
        match loader.step()? {
            OpenProgress::Ready(doc) => return Ok(*doc),
            OpenProgress::Need(need) => {
                if last.as_ref() == Some(&need) {
                    panic!("no progress loading {need:?}: ensure_range did not satisfy the range");
                }
                last = Some(need);
            }
        }
    }
}

struct Expected {
    pages: u32,
    linearized: bool,
    encrypted: bool,
}

fn buffered_open(name: &str) -> Option<Expected> {
    let bytes = load(name).ok()?;
    let mut doc = PDFDocument::open(bytes).ok()?;
    Some(Expected {
        pages: doc.page_count().ok()?,
        linearized: doc.is_linearized(),
        encrypted: doc.is_encrypted(),
    })
}

fn assert_loader_matches(name: &str) {
    let Some(expected) = buffered_open(name) else {
        return; // fixture that doesn't open buffers nothing to compare against
    };

    let path = fixtures_pdfs().join(name);
    let stream =
        FileChunkedStream::open(&path, None, None).expect("fixture should open as a stream");
    let mut loader_doc = open_via_loader(DocumentLoader::new(stream))
        .unwrap_or_else(|e| panic!("{name}: loader should open where buffered open does: {e}"));

    let pages = loader_doc
        .page_count()
        .unwrap_or_else(|e| panic!("{name}: loader document should expose pages: {e}"));
    assert_eq!(
        pages, expected.pages,
        "{name}: page count differs from buffered open"
    );
    assert_eq!(
        loader_doc.is_linearized(),
        expected.linearized,
        "{name}: linearization flag differs from buffered open"
    );
    assert_eq!(
        loader_doc.is_encrypted(),
        expected.encrypted,
        "{name}: encryption flag differs from buffered open"
    );
}

#[test]
fn loader_matches_buffered_open_on_fixtures() {
    for name in [
        "basicapi.pdf",
        "asciihexdecode.pdf",
        "annotation-border-styles.pdf",
        "xref-stream.pdf",
    ] {
        assert_loader_matches(name);
    }
}

#[test]
fn loader_opens_memory_stream_identically() {
    // The memory (fully-loaded) shape: one step, no Needs — the convergence
    // target for `PDFDocument::open(Vec<u8>)` in a later step.
    let bytes = load("basicapi.pdf").expect("fixture readable");
    let mut buffered = PDFDocument::open(bytes.clone()).expect("buffered open");
    let buffered_pages = buffered.page_count().expect("pages");

    let mut loader = DocumentLoader::new(Stream::from_bytes(bytes));
    match loader.step().expect("memory loader step") {
        OpenProgress::Ready(mut doc) => {
            assert_eq!(
                doc.page_count().expect("pages"),
                buffered_pages,
                "memory-loader page count must match buffered open"
            );
        }
        OpenProgress::Need(_) => panic!("memory source must open without Needs"),
    }
}
