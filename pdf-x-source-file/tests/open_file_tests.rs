//! Behavioral tests for the shell-side file open (`pdf_x_source_file::open_file`),
//! migrated from core's `open_file_drives_progressive_load` when the file source
//! moved out of core (Phase D step 4), plus parity against the buffered open.

use pdf_x_core::PDFDocument;
use pdf_x_source_file::open_file;
use std::io::Write;
use tempfile::NamedTempFile;

fn minimal_pdf() -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");

    let o1 = buf.len();
    buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    let o2 = buf.len();
    buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    let o3 = buf.len();
    buf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
    );
    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n");
    buf.extend_from_slice(format!("{:010} 65535 f \r\n", 0).as_bytes());
    buf.extend_from_slice(format!("{:010} 00000 n \r\n", o1).as_bytes());
    buf.extend_from_slice(format!("{:010} 00000 n \r\n", o2).as_bytes());
    buf.extend_from_slice(format!("{:010} 00000 n \r\n", o3).as_bytes());
    buf.extend_from_slice(b"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n");
    buf.extend_from_slice(format!("{}\n", xref_offset).as_bytes());
    buf.extend_from_slice(b"%%EOF");
    buf
}

fn write_temp(data: &[u8]) -> NamedTempFile {
    let tmp = NamedTempFile::new().expect("temp file should be created");
    tmp.as_file()
        .write_all(data)
        .expect("temp file should be writable");
    tmp.as_file().sync_all().ok();
    tmp
}

#[test]
fn open_file_drives_progressive_load() {
    // The chunked open drives the loader's xref parse to completion with the
    // default 64 KB chunk size — the practical shape of every native open.
    // (Known pre-existing caveat, unchanged by the move: a tiny chunk size
    // like 64 forces parse into the missing-data path where the lexer loops
    // instead of surfacing DataMissing.)
    let tmp = write_temp(&minimal_pdf());

    let mut doc =
        open_file(tmp.path(), None, None).expect("open_file should drive the load to completion");
    let page = doc
        .get_page(0)
        .expect("page 0 should resolve after the driven load");
    assert_eq!(page.index(), 0);
}

#[test]
fn open_file_matches_buffered_open() {
    // Parity gate for the D.4 move: the loader-driven file open must produce
    // the same document as buffering the bytes and calling `PDFDocument::open`
    // (which itself runs the same loader over a memory source since D.2).
    // Default chunk size: a tiny one trips the known lexer-loop-on-partial-data
    // bug noted above, which is orthogonal to this move.
    let data = minimal_pdf();
    let tmp = write_temp(&data);

    let mut buffered = PDFDocument::open(data).expect("buffered open");
    let mut chunked = open_file(tmp.path(), None, None).expect("chunked open");

    assert_eq!(
        buffered.page_count().expect("buffered pages"),
        chunked.page_count().expect("chunked pages"),
        "page counts must match across open shapes"
    );
    assert_eq!(buffered.is_encrypted(), chunked.is_encrypted());
    assert_eq!(buffered.is_linearized(), chunked.is_linearized());
}

#[test]
fn open_file_matches_buffered_open_on_fixtures() {
    let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("pdfs");
    for name in [
        "basicapi.pdf",
        "asciihexdecode.pdf",
        "annotation-border-styles.pdf",
        "xref-stream.pdf",
    ] {
        let bytes = match std::fs::read(fixtures.join(name)) {
            Ok(b) => b,
            Err(_) => continue, // fixture absent in this checkout — nothing to compare
        };
        let tmp = write_temp(&bytes);
        let mut buffered = PDFDocument::open(bytes).expect("buffered open");
        let mut chunked = open_file(tmp.path(), None, None)
            .unwrap_or_else(|e| panic!("{name}: chunked open should succeed: {e}"));
        assert_eq!(
            buffered.page_count().unwrap_or(0),
            chunked.page_count().unwrap_or(0),
            "{name}: page count differs between buffered and chunked open"
        );
        assert_eq!(buffered.is_encrypted(), chunked.is_encrypted(), "{name}");
        assert_eq!(buffered.is_linearized(), chunked.is_linearized(), "{name}");
    }
}
