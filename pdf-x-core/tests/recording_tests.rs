//! Interpreter and parser snapshot tests.
//!
//! Two layers of snapshot testing:
//!
//! **Layer 1 — Operator dump**: Parses the content stream into `Operation`s and
//! serializes them with deterministic formatting (`Operation::format_snapshot`).
//! Catches parsing bugs independently of interpretation.
//!
//! **Layer 2 — RecordingDevice**: Runs the full interpreter (parse + interpret)
//! and records every `Device` call via `RecordingDevice`. Catches interpretation bugs.
//!
//! Both layers use the same `UPDATE_SNAPSHOTS=1` env-var gate to regenerate.

use std::path::PathBuf;

use pdf_x_core::core::content_stream::ContentStreamEvaluator;
use pdf_x_core::core::lexer::Lexer;
use pdf_x_core::core::parser::Parser;
use pdf_x_core::core::stream::Stream;
use pdf_x_core::rendering::{interpret_with_scale, recording_device::RecordingDevice};

// -- Paths ------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("pdf-x-core has a parent workspace dir")
        .join("tests")
        .join("fixtures")
        .join("pdfs")
}

fn hayro_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("pdf-x-core has a parent workspace dir")
        .join("hayro")
        .join("hayro-tests")
        .join("pdfs")
        .join("custom")
}

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
}

fn snapshot_path(name: &str) -> PathBuf {
    snapshot_dir().join(name)
}

// -- Snapshot write/compare -------------------------------------------------

fn write_or_compare(name: &str, output: &str) {
    let snap_file = snapshot_path(name);
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(&snap_file, output).unwrap();
        eprintln!("  wrote {} ({} lines)", name, output.lines().count());
    } else {
        let expected = std::fs::read_to_string(&snap_file).unwrap_or_else(|_| {
            panic!(
                "Snapshot not found: {}. Run UPDATE_SNAPSHOTS=1 cargo test --test recording_tests to generate.",
                name
            )
        });
        assert_eq!(output, expected, "snapshot mismatch for {}", name);
    }
}

// -- Layer 1: Operator dump ------------------------------------------------

/// Parse the page's content stream and dump Operations as text.
fn dump_operations(resolved: &pdf_x_core::core::resolved_page::ResolvedPage) -> String {
    let stream = Stream::from_bytes(resolved.contents().to_vec());
    let lexer = Lexer::new(Box::new(stream)).unwrap();
    let parser = Parser::new(lexer).unwrap();
    let mut evaluator = ContentStreamEvaluator::new(parser);

    let mut lines = Vec::new();
    while let Some(op) = evaluator.read_operation().unwrap() {
        lines.push(op.format_snapshot());
    }
    lines.join("\n")
}

fn run_operator_dump(pdf_path: &std::path::Path, page_index: usize, snapshot_name: &str) {
    let bytes = std::fs::read(pdf_path).unwrap();
    let mut doc = pdf_x_core::core::PDFDocument::open(bytes).unwrap();
    let resolved = doc.resolve_page(page_index).unwrap();
    let output = dump_operations(&resolved);
    write_or_compare(snapshot_name, &output);
}

// -- Layer 2: RecordingDevice (interpretation) ------------------------------

fn run_recording_snapshot(
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
    write_or_compare(snapshot_name, &output);
}

// -- Fixture definitions ----------------------------------------------------

/// (path, page_index, page_width, page_height, name_prefix)
struct Fixture {
    path: PathBuf,
    page: usize,
    w: f64,
    h: f64,
    name: &'static str,
}

fn all_fixtures() -> Vec<Fixture> {
    let fd = fixtures_dir();
    let hd = hayro_dir();
    vec![
        // --- Original fixtures ---
        Fixture {
            path: fd.join("basicapi.pdf"),
            page: 0, w: 595.28, h: 841.89, name: "basicapi",
        },
        Fixture {
            path: fd.join("tracemonkey.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "tracemonkey",
        },
        // --- Type3 fonts ---
        Fixture {
            path: fd.join("simpletype3font.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "type3_simple",
        },
        Fixture {
            path: hd.join("font_type3_filled_glyphs.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "type3_filled",
        },
        // --- CJK / CID fonts ---
        Fixture {
            path: hd.join("font_cid_1.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "cid_1",
        },
        // --- Shading ---
        Fixture {
            path: hd.join("pattern_shading_type0_7.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "shading_axial",
        },
        Fixture {
            path: hd.join("pattern_shading_type2_1.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "shading_radial",
        },
        // --- Tiling patterns ---
        Fixture {
            path: hd.join("pattern_tiling_simple.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "tiling_simple",
        },
        // --- Transparency / soft masks ---
        Fixture {
            path: hd.join("fireworks.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "fireworks",
        },
        Fixture {
            path: hd.join("mask_alpha.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "mask_alpha",
        },
        // --- CMYK images ---
        Fixture {
            path: hd.join("image_cmyk_jpg.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "cmyk_jpg",
        },
        // --- Inline images ---
        Fixture {
            path: hd.join("image_inline_2.pdf"),
            page: 0, w: 612.0, h: 792.0, name: "inline_img",
        },
    ]
}

// -- Layer 1 tests (operator dump) -----------------------------------------

#[test]
fn ops_basicapi_page0() {
    let f = &all_fixtures()[0];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_tracemonkey_page0() {
    let f = &all_fixtures()[1];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_type3_simple() {
    let f = &all_fixtures()[2];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_type3_filled() {
    let f = &all_fixtures()[3];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_cid_1() {
    let f = &all_fixtures()[4];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_shading_axial() {
    let f = &all_fixtures()[5];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_shading_radial() {
    let f = &all_fixtures()[6];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_tiling_simple() {
    let f = &all_fixtures()[7];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_fireworks() {
    let f = &all_fixtures()[8];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_mask_alpha() {
    let f = &all_fixtures()[9];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_cmyk_jpg() {
    let f = &all_fixtures()[10];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

#[test]
fn ops_inline_img() {
    let f = &all_fixtures()[11];
    run_operator_dump(&f.path, f.page, &format!("ops_{}.txt", f.name));
}

// -- Layer 2 tests (RecordingDevice) ----------------------------------------

#[test]
fn snapshot_basicapi_page0() {
    let f = &all_fixtures()[0];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_tracemonkey_page0() {
    let f = &all_fixtures()[1];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_type3_simple() {
    let f = &all_fixtures()[2];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_type3_filled() {
    let f = &all_fixtures()[3];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_cid_1() {
    let f = &all_fixtures()[4];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_shading_axial() {
    let f = &all_fixtures()[5];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_shading_radial() {
    let f = &all_fixtures()[6];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_tiling_simple() {
    let f = &all_fixtures()[7];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_fireworks() {
    let f = &all_fixtures()[8];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_mask_alpha() {
    let f = &all_fixtures()[9];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_cmyk_jpg() {
    let f = &all_fixtures()[10];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}

#[test]
fn snapshot_inline_img() {
    let f = &all_fixtures()[11];
    run_recording_snapshot(&f.path, f.page, &format!("dev_{}.txt", f.name), f.w, f.h);
}
