/// PDF-X performance benchmarks with criterion baselines + external tool comparison.
///
/// ## Usage
///
/// ```sh
/// # Save a named baseline (first run):
/// cargo bench --bench perf --features "rendering crypto" -- --save-baseline initial
///
/// # Compare current code against the saved baseline:
/// cargo bench --bench perf --features "rendering crypto" -- --baseline initial
///
/// # Update the baseline after an intentional change:
/// cargo bench --bench perf --features "rendering crypto" -- --save-baseline initial --force
/// ```
///
/// Criterion stores baselines under `target/criterion/<name>/`. Commit nothing;
/// baselines are machine-specific. The workflow is: save on a clean commit,
/// then compare after changes.
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::fs;
use std::process::Command;

use pdf_x_core::core::PDFDocument;

// ============================================================================
// Fixtures
// ============================================================================

/// (label, path, pages)
const FIXTURES: &[(&str, &str, usize)] = &[
    ("small", "pdf.js/test/pdfs/basicapi.pdf", 3), // 106 KB, 3 pages
    ("medium", "pdf.js/test/pdfs/tracemonkey.pdf", 14), // 1.0 MB, 14 pages
    ("large", "pdf.js/test/pdfs/freeculture.pdf", 352), // 2.5 MB, 352 pages
];

fn read_fixture(path: &str) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|_| panic!("read fixture: {path}"))
}

// ============================================================================
// PDF-X benchmarks
// ============================================================================

fn bench_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdfx_open");
    for &(label, path, _pages) in FIXTURES {
        let data = read_fixture(path);
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &data, |b, data| {
            b.iter(|| PDFDocument::open(black_box(data.clone())));
        });
    }
    group.finish();
}

fn bench_resolve_page(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdfx_resolve_page");

    // Cold: first resolve (BFS + cache miss)
    for &(label, path, _pages) in FIXTURES {
        let data = read_fixture(path);
        group.bench_with_input(BenchmarkId::new(label, "cold"), &data, |b, data| {
            b.iter_with_setup(
                || PDFDocument::open(data.clone()).expect("open"),
                |mut doc| {
                    let page = doc.get_page(0).expect("get_page");
                    doc.resolve_page(0).expect("resolve");
                    page
                },
            );
        });
    }

    // Cached: second resolve (LRU hit) — measures cache lookup overhead only
    for &(label, path, _pages) in FIXTURES {
        let data = read_fixture(path);
        group.bench_with_input(BenchmarkId::new(label, "cached"), &data, |b, data| {
            b.iter_batched(
                || {
                    let mut doc = PDFDocument::open(data.clone()).expect("open");
                    doc.resolve_page(0).expect("warm resolve");
                    doc
                },
                |mut doc| {
                    doc.resolve_page(0).expect("cached resolve");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_render_single(c: &mut Criterion) {
    use pdf_x_device_tinyskia::RenderToImage;

    let mut group = c.benchmark_group("pdfx_render_single");

    for &(label, path, _pages) in FIXTURES {
        let data = read_fixture(path);
        group.throughput(Throughput::Elements(1u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &data, |b, data| {
            b.iter_batched(
                || {
                    let mut doc = PDFDocument::open(data.clone()).expect("open");
                    // Warm the LRU so we measure interpret + rasterize, not resolve
                    doc.render_page_to_image(0, Some(1.0)).expect("warm render");
                    doc
                },
                |mut doc| {
                    let _ = doc.render_page_to_image(0, Some(1.0)).expect("render");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_render_multi(c: &mut Criterion) {
    use pdf_x_device_tinyskia::RenderToImage;

    let mut group = c.benchmark_group("pdfx_render_multi");

    for &(label, path, pages) in FIXTURES {
        let data = read_fixture(path);
        let page_indices: Vec<usize> = (0..pages.min(8)).collect();
        let n = page_indices.len() as u64;
        group.throughput(Throughput::Elements(n));

        // Sequential
        group.bench_with_input(BenchmarkId::new(label, "sequential"), &data, |b, data| {
            b.iter_batched_ref(
                || {
                    let mut doc = PDFDocument::open(data.clone()).expect("open");
                    // Pre-warm all pages so we measure interpret only
                    for &i in &page_indices {
                        let _ = doc.render_page_to_image(i, Some(1.0));
                    }
                    doc
                },
                |doc| {
                    for &i in &page_indices {
                        let _ = doc.render_page_to_image(i, Some(1.0));
                    }
                },
                criterion::BatchSize::SmallInput,
            );
        });

        // Parallel
        group.bench_with_input(BenchmarkId::new(label, "parallel"), &data, |b, data| {
            b.iter_batched_ref(
                || {
                    let mut doc = PDFDocument::open(data.clone()).expect("open");
                    for &i in &page_indices {
                        let _ = doc.render_page_to_image(i, Some(1.0));
                    }
                    doc
                },
                |doc| {
                    let _ = doc.render_pages_to_images(&page_indices, Some(1.0));
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_extract_text(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdfx_extract_text");

    for &(label, path, _pages) in FIXTURES {
        let data = read_fixture(path);
        group.bench_with_input(BenchmarkId::from_parameter(label), &data, |b, data| {
            b.iter_batched_ref(
                || {
                    let mut doc = PDFDocument::open(data.clone()).expect("open");
                    let _ = doc.get_page(0); // warm page cache
                    doc
                },
                |doc| {
                    let _ = doc.extract_text_from_page_as_string(0);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ============================================================================
// External tool benchmarks
// ============================================================================

fn bench_mutool_draw(c: &mut Criterion) {
    let mut group = c.benchmark_group("mutool_draw");
    group.throughput(Throughput::Elements(1u64));

    for &(label, path, _pages) in FIXTURES {
        let abs_path = std::path::Path::new(path).canonicalize();
        if abs_path.is_err() {
            continue;
        }
        let abs = abs_path.unwrap();
        group.bench_with_input(BenchmarkId::from_parameter(label), &abs, |b, path| {
            b.iter(|| {
                let _ = Command::new("mutool")
                    .args(["draw", "-o", "/dev/null", "-F", "pnm", "-r", "72"])
                    .arg(path)
                    .arg("1") // page 1 (1-based)
                    .output();
            });
        });
    }

    group.finish();
}

fn bench_mutool_draw_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("mutool_draw_all");

    for &(label, path, pages) in FIXTURES {
        let abs_path = std::path::Path::new(path).canonicalize();
        if abs_path.is_err() {
            continue;
        }
        let abs = abs_path.unwrap();
        let n = pages.min(8) as u64;
        group.throughput(Throughput::Elements(n));

        group.bench_with_input(BenchmarkId::from_parameter(label), &abs, |b, path| {
            b.iter(|| {
                let _ = Command::new("mutool")
                    .args(["draw", "-o", "/dev/null", "-F", "pnm", "-r", "72"])
                    .arg(path)
                    .args(["1", &format!("{}", pages.min(8))])
                    .output();
            });
        });
    }

    group.finish();
}

fn bench_pdftoppm(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdftoppm");
    group.throughput(Throughput::Elements(1u64));

    for &(label, path, _pages) in FIXTURES {
        let abs_path = std::path::Path::new(path).canonicalize();
        if abs_path.is_err() {
            continue;
        }
        let abs = abs_path.unwrap();
        group.bench_with_input(BenchmarkId::from_parameter(label), &abs, |b, path| {
            b.iter(|| {
                let _ = Command::new("pdftoppm")
                    .args(["-r", "72", "-f", "1", "-l", "1", "-png"])
                    .arg(path)
                    .arg("/dev/null")
                    .output();
            });
        });
    }

    group.finish();
}

fn bench_pdftotext(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdftotext");

    for &(label, path, _pages) in FIXTURES {
        let abs_path = std::path::Path::new(path).canonicalize();
        if abs_path.is_err() {
            continue;
        }
        let abs = abs_path.unwrap();
        group.bench_with_input(BenchmarkId::from_parameter(label), &abs, |b, path| {
            b.iter(|| {
                let _ = Command::new("pdftotext")
                    .args(["-f", "1", "-l", "1"])
                    .arg(path)
                    .arg("/dev/null")
                    .output();
            });
        });
    }

    group.finish();
}

fn bench_pdftotext_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdftotext_all");

    for &(label, path, _pages) in FIXTURES {
        let abs_path = std::path::Path::new(path).canonicalize();
        if abs_path.is_err() {
            continue;
        }
        let abs = abs_path.unwrap();
        group.bench_with_input(BenchmarkId::from_parameter(label), &abs, |b, path| {
            b.iter(|| {
                let _ = Command::new("pdftotext")
                    .arg(path)
                    .arg("/dev/null")
                    .output();
            });
        });
    }

    group.finish();
}

// ============================================================================
// Metadata benchmarks (outline, fonts, page tree)
// ============================================================================

fn bench_outline(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdfx_outline");

    for &(label, path, _pages) in FIXTURES {
        let data = read_fixture(path);
        group.bench_with_input(BenchmarkId::from_parameter(label), &data, |b, data| {
            b.iter_batched(
                || PDFDocument::open(data.clone()).expect("open"),
                |mut doc| {
                    let _ = doc.document_outline().expect("outline");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_page_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdfx_page_tree");

    for &(label, path, pages) in FIXTURES {
        let data = read_fixture(path);
        let max = pages.min(50);
        group.throughput(Throughput::Elements(max as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &data, |b, data| {
            b.iter_batched(
                || PDFDocument::open(data.clone()).expect("open"),
                |mut doc| {
                    for i in 0..max {
                        let _ = doc.get_page(i);
                    }
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_ttfr(c: &mut Criterion) {
    use pdf_x_device_tinyskia::RenderToImage;

    let mut group = c.benchmark_group("pdfx_ttfr");

    for &(label, path, _pages) in FIXTURES {
        let data = read_fixture(path);
        group.bench_with_input(BenchmarkId::from_parameter(label), &data, |b, data| {
            b.iter(|| {
                let mut doc = PDFDocument::open(black_box(data.clone())).expect("open");
                let _ = doc.render_page_to_image(0, Some(1.0)).expect("render");
            });
        });
    }

    group.finish();
}

// ============================================================================
// CLI benchmarks (subprocess — end-to-end including process startup)
// ============================================================================

fn bench_cli(c: &mut Criterion) {
    let cli_bin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/release/pdf-inspect");

    if !cli_bin.exists() {
        eprintln!("CLI binary not found at {cli_bin:?}, skipping cli benchmarks");
        return;
    }
    eprintln!("CLI binary found at {cli_bin:?}");

    let mut group = c.benchmark_group("cli");

    for &sub in &["info", "outline", "fonts", "page-sizes", "stats"] {
        for &(label, path, _pages) in FIXTURES {
            let abs = std::path::Path::new(path).canonicalize().unwrap();
            group.bench_with_input(BenchmarkId::new(sub, label), &abs, |b, path| {
                b.iter(|| {
                    let _ = Command::new(&cli_bin)
                        .arg(path)
                        .arg(format!("--{sub}"))
                        .output();
                });
            });
        }
    }

    group.finish();
}

// ============================================================================
// Group registration
// ============================================================================

criterion_group!(
    benches,
    bench_open,
    bench_resolve_page,
    bench_render_single,
    bench_render_multi,
    bench_extract_text,
    bench_outline,
    bench_page_tree,
    bench_ttfr,
    bench_mutool_draw,
    bench_mutool_draw_all,
    bench_pdftoppm,
    bench_pdftotext,
    bench_pdftotext_all,
    bench_cli,
);
criterion_main!(benches);
