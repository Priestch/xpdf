//! Interpreter snapshot tests using RecordingDevice.
//!
//! These tests run the full interpreter on fixture PDFs and compare the
//! device call sequence against committed snapshots. Any regression in the
//! interpretation layer (Layer 2) will change the snapshot.

use std::path::PathBuf;

use pdf_x_core::rendering::{interpret_with_scale, recording_device::RecordingDevice};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("pdf-x-core has a parent workspace dir")
        .join("tests")
        .join("fixtures")
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(name)
}

/// Run the interpreter on a fixture page with RecordingDevice.
/// If UPDATE_SNAPSHOTS env var is set, write the snapshot; otherwise compare.
fn run_and_compare_snapshot(
    pdf_path: &std::path::Path,
    page_index: usize,
    snapshot_name: &str,
    page_width: f64,
    page_height: f64,
) {
    let bytes = std::fs::read(pdf_path).unwrap();
    let mut doc = pdf_x_core::core::PDFDocument::open(bytes).unwrap();
    let resolved = doc.resolve_page(page_index).unwrap();
    let mut device = RecordingDevice::new(page_width, page_height);
    interpret_with_scale(&resolved, &mut device, 1.0).unwrap();

    let output = device.serialize();

    let snap_file = snapshot_path(snapshot_name);
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(&snap_file, &output).unwrap();
        eprintln!("  wrote {} ({} calls)", snapshot_name, output.lines().count());
    } else {
        let expected = std::fs::read_to_string(&snap_file)
            .unwrap_or_else(|_| {
                panic!(
                    "Snapshot not found. Run UPDATE_SNAPSHOTS=1 cargo test --test recording_tests to generate."
                )
            });
        assert_eq!(output, expected, "snapshot mismatch for {}", snapshot_name);
    }
}

#[test]
fn snapshot_basicapi_page0() {
    let path = fixtures_dir().join("pdfs").join("basicapi.pdf");
    run_and_compare_snapshot(&path, 0, "basicapi_p0.txt", 612.0, 792.0);
}

#[test]
fn snapshot_tracemonkey_page0() {
    let path = fixtures_dir().join("pdfs").join("tracemonkey.pdf");
    run_and_compare_snapshot(&path, 0, "tracemonkey_p0.txt", 612.0, 792.0);
}
