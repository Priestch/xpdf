/// Benchmarks for PDF-X parsing performance
///
/// Run with: cargo bench
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use pdf_x::PDFDocument;
use std::fs;

/// Benchmark PDF document opening
fn benchmark_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("document_opening");

    // Test with different PDFs if available
    let test_files = [
        "pdf.js/test/pdfs/basicapi.pdf",
        "pdf.js/test/pdfs/tracemonkey.pdf",
    ];

    for file_path in &test_files {
        if let Ok(data) = fs::read(file_path) {
            let file_size = data.len();
            group.throughput(Throughput::Bytes(file_size as u64));

            group.bench_with_input(BenchmarkId::from_parameter(file_path), &data, |b, data| {
                b.iter(|| PDFDocument::open(black_box(data.clone())));
            });
        }
    }

    group.finish();
}

/// Benchmark progressive file opening
fn benchmark_open_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("progressive_loading");

    let test_files = [
        "pdf.js/test/pdfs/basicapi.pdf",
        "pdf.js/test/pdfs/tracemonkey.pdf",
    ];

    for file_path in &test_files {
        if std::path::Path::new(file_path).exists() {
            group.bench_with_input(
                BenchmarkId::from_parameter(file_path),
                file_path,
                |b, path| {
                    b.iter(|| PDFDocument::open_file(black_box(path), None, None));
                },
            );
        }
    }

    group.finish();
}

/// Benchmark text extraction
fn benchmark_text_extraction(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_extraction");

    let file_path = "pdf.js/test/pdfs/tracemonkey.pdf";
    if let Ok(mut doc) = PDFDocument::open_file(file_path, None, None) {
        if let Ok(page) = doc.get_page(0) {
            group.bench_function("extract_text_page_0", |b| {
                b.iter(|| page.extract_text(black_box(doc.xref_mut())));
            });

            group.bench_function("extract_text_as_string_page_0", |b| {
                b.iter(|| page.extract_text_as_string(black_box(doc.xref_mut())));
            });
        }
    }

    group.finish();
}

/// Benchmark page access patterns
fn benchmark_page_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("page_access");

    let file_path = "pdf.js/test/pdfs/tracemonkey.pdf";
    if let Ok(mut doc) = PDFDocument::open_file(file_path, None, None) {
        // Sequential access
        group.bench_function("sequential_page_access", |b| {
            b.iter(|| {
                for i in 0..5 {
                    let _ = doc.get_page(black_box(i));
                }
            });
        });

        // Random access
        group.bench_function("random_page_access", |b| {
            b.iter(|| {
                let _ = doc.get_page(black_box(0));
                let _ = doc.get_page(black_box(4));
                let _ = doc.get_page(black_box(2));
                let _ = doc.get_page(black_box(1));
                let _ = doc.get_page(black_box(3));
            });
        });

        // Repeated access (cache hit)
        group.bench_function("cached_page_access", |b| {
            b.iter(|| {
                for _ in 0..10 {
                    let _ = doc.get_page(black_box(0));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    benchmark_open,
    benchmark_open_file,
    benchmark_text_extraction,
    benchmark_page_access
);
criterion_main!(benches);
