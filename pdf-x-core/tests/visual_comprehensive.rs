//! Comprehensive visual rendering tests for PDF-X.
//!
//! This test suite provides comprehensive visual testing by rendering PDFs
//! and saving them as PNG images for inspection. Inspired by hayro's test
//! infrastructure but adapted for pdf-x's testing needs.

#![cfg(feature = "rendering")]

use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::RenderToImage;

// ============================================================================
// Test Configuration
// ============================================================================

const OUTPUT_DIR: &str = "/tmp/pdf-x-visual-tests";
const SCALE: f64 = 2.0; // Render at 2x for better quality

/// Resolve a repo-relative test PDF path against the workspace root
/// (`pdf-x-core/tests` → `../..`). Fixtures ship in the pdf.js submodule
/// (`pdf.js/test/pdfs/`).
fn resolve_repo_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Test case definition
struct TestCase {
    name: &'static str,
    path: &'static str,
    pages: &'static [usize],
    category: TestCategory,
}

#[derive(Debug, Clone, Copy)]
enum TestCategory {
    Text,
    Annotations,
    Graphics,
    Images,
    Fonts,
    Forms,
    Complex,
}

// ============================================================================
// Test Suite Definitions
// ============================================================================

/// Basic text rendering tests
const TEXT_TESTS: &[TestCase] = &[TestCase {
    name: "simple_text",
    path: "pdf.js/test/pdfs/issue7200.pdf",
    pages: &[0],
    category: TestCategory::Text,
}];

/// Annotation tests
const ANNOTATION_TESTS: &[TestCase] = &[
    TestCase {
        name: "annotation_line",
        path: "pdf.js/test/pdfs/annotation-line.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
    TestCase {
        name: "annotation_highlight",
        path: "pdf.js/test/pdfs/annotation-highlight.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
    TestCase {
        name: "annotation_link",
        path: "pdf.js/test/pdfs/annotation-link-text-popup.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
    TestCase {
        name: "annotation_square_circle",
        path: "pdf.js/test/pdfs/annotation-square-circle.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
    TestCase {
        name: "annotation_freetext",
        path: "pdf.js/test/pdfs/annotation-freetext.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
];

/// Graphics and path tests
const GRAPHICS_TESTS: &[TestCase] = &[
    TestCase {
        name: "alpha_transparency",
        path: "pdf.js/test/pdfs/alphatrans.pdf",
        pages: &[0],
        category: TestCategory::Graphics,
    },
    TestCase {
        name: "issue19802",
        path: "pdf.js/test/pdfs/issue19802.pdf",
        pages: &[0],
        category: TestCategory::Graphics,
    },
];

/// Font rendering tests
const FONT_TESTS: &[TestCase] = &[TestCase {
    name: "truetype_font",
    path: "pdf.js/test/pdfs/issue7200.pdf",
    pages: &[0],
    category: TestCategory::Fonts,
}];

/// Complex document tests
///
/// Previously pointed at dev-machine-only book PDFs; the surviving complex
/// coverage lives in GRAPHICS_TESTS (issue19802). Kept as an (empty) group so
/// ALL_TESTS and the per-category runners stay shape-compatible.
const COMPLEX_TESTS: &[TestCase] = &[];

/// All tests combined
const ALL_TESTS: &[&[TestCase]] = &[
    TEXT_TESTS,
    ANNOTATION_TESTS,
    GRAPHICS_TESTS,
    FONT_TESTS,
    COMPLEX_TESTS,
];

// ============================================================================
// Rendering Infrastructure
// ============================================================================

/// Render a single PDF page to PNG
fn render_page(pdf_path: &str, page_num: usize, output_path: &str) -> Result<(f64, u32), String> {
    // Read PDF file
    let pdf_bytes = std::fs::read(pdf_path).map_err(|e| format!("Failed to read PDF: {}", e))?;

    // Parse PDF
    let mut doc =
        PDFDocument::open(pdf_bytes).map_err(|e| format!("Failed to parse PDF: {:?}", e))?;

    let page_count = doc
        .page_count()
        .map_err(|e| format!("Failed to get page count: {:?}", e))?;

    if page_num >= page_count as usize {
        return Err(format!(
            "Page {} out of range (max {})",
            page_num, page_count
        ));
    }

    // Render through the resolve → interpret path: scale is applied inside
    // interpret and annotation appearances are included. (A manual
    // `Page::render` + pre-set device matrix does not work — Page::render
    // installs its own unscaled page-initial CTM and overwrites the scale.)
    let (_width, _height, rgba) = doc
        .render_page_to_image(page_num, Some(SCALE as f32))
        .map_err(|e| format!("Render error: {:?}", e))?;

    // Calculate pixel statistics
    let non_white_count = rgba
        .chunks(4)
        .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
        .count();

    let total_pixels = (rgba.len() / 4) as u32;
    let percentage = (non_white_count as f64 / total_pixels as f64) * 100.0;

    // Save PNG
    let file = std::fs::File::create(output_path)
        .map_err(|e| format!("Failed to create PNG {}: {}", output_path, e))?;
    let mut encoder = png::Encoder::new(file, _width, _height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("Failed to write PNG header: {}", e))?;
    writer
        .write_image_data(&rgba)
        .map_err(|e| format!("Failed to write PNG data: {}", e))?;

    Ok((percentage, total_pixels))
}

/// Run a single test case
fn run_test_case(test: &TestCase) -> Result<TestResult, String> {
    let path = resolve_repo_path(test.path);
    let pdf_name = std::path::Path::new(test.path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let mut pages_rendered = 0;
    let mut results = Vec::new();

    for &page_num in test.pages {
        let filename = format!("{}_{}_page_{}.png", test.name, pdf_name, page_num);
        let filepath = std::path::Path::new(OUTPUT_DIR).join(&filename);

        match render_page(
            &path.to_string_lossy(),
            page_num,
            filepath.to_str().unwrap(),
        ) {
            Ok((percentage, pixels)) => {
                results.push(PageResult {
                    page_num,
                    percentage,
                    pixels,
                    status: RenderStatus::Success,
                });
                pages_rendered += 1;
            }
            Err(e) => {
                results.push(PageResult {
                    page_num,
                    percentage: 0.0,
                    pixels: 0,
                    status: RenderStatus::Failed(e),
                });
            }
        }
    }

    Ok(TestResult {
        test_name: test.name.to_string(),
        category: test.category,
        pages_rendered,
        total_pages: test.pages.len(),
        page_results: results,
    })
}

#[derive(Debug)]
struct PageResult {
    page_num: usize,
    percentage: f64,
    pixels: u32,
    status: RenderStatus,
}

#[derive(Debug)]
enum RenderStatus {
    Success,
    Failed(String),
}

#[derive(Debug)]
struct TestResult {
    test_name: String,
    category: TestCategory,
    pages_rendered: usize,
    total_pages: usize,
    page_results: Vec<PageResult>,
}

// ============================================================================
// Test Functions
// ============================================================================

#[test]
fn test_visual_text_rendering() {
    println!("\n=== Text Rendering Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in TEXT_TESTS {
        if !resolve_repo_path(test.path).exists() {
            println!("⊘ Skipping {} (file not found)", test.name);
            continue;
        }

        println!("Testing: {}", test.name);

        match run_test_case(test) {
            Ok(result) => {
                for page_result in &result.page_results {
                    match &page_result.status {
                        RenderStatus::Success => {
                            println!(
                                "  ✓ Page {}: {:.2}% non-white ({} pixels)",
                                page_result.page_num, page_result.percentage, page_result.pixels
                            );
                            total_passed += 1;
                        }
                        RenderStatus::Failed(e) => {
                            println!("  ✗ Page {}: {}", page_result.page_num, e);
                            total_failed += 1;
                        }
                    }
                }
            }
            Err(e) => {
                println!("  ✗ Failed: {}", e);
                total_failed += 1;
            }
        }
    }

    println!(
        "\nText tests: {} passed, {} failed\n",
        total_passed, total_failed
    );
    assert!(total_passed > 0, "No text tests passed");
}

#[test]
fn test_visual_annotations() {
    println!("\n=== Annotation Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in ANNOTATION_TESTS {
        if !resolve_repo_path(test.path).exists() {
            println!("⊘ Skipping {} (file not found)", test.name);
            continue;
        }

        println!("Testing: {}", test.name);

        match run_test_case(test) {
            Ok(result) => {
                for page_result in &result.page_results {
                    match &page_result.status {
                        RenderStatus::Success => {
                            println!(
                                "  ✓ Page {}: {:.2}% non-white",
                                page_result.page_num, page_result.percentage
                            );
                            total_passed += 1;
                        }
                        RenderStatus::Failed(e) => {
                            println!("  ✗ Page {}: {}", page_result.page_num, e);
                            total_failed += 1;
                        }
                    }
                }
            }
            Err(e) => {
                println!("  ✗ Failed: {}", e);
                total_failed += 1;
            }
        }
    }

    println!(
        "\nAnnotation tests: {} passed, {} failed\n",
        total_passed, total_failed
    );
}

#[test]
fn test_visual_graphics() {
    println!("\n=== Graphics Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in GRAPHICS_TESTS {
        if !resolve_repo_path(test.path).exists() {
            println!("⊘ Skipping {} (file not found)", test.name);
            continue;
        }

        println!("Testing: {}", test.name);

        match run_test_case(test) {
            Ok(result) => {
                for page_result in &result.page_results {
                    match &page_result.status {
                        RenderStatus::Success => {
                            println!(
                                "  ✓ Page {}: {:.2}% non-white",
                                page_result.page_num, page_result.percentage
                            );
                            total_passed += 1;
                        }
                        RenderStatus::Failed(e) => {
                            println!("  ✗ Page {}: {}", page_result.page_num, e);
                            total_failed += 1;
                        }
                    }
                }
            }
            Err(e) => {
                println!("  ✗ Failed: {}", e);
                total_failed += 1;
            }
        }
    }

    println!(
        "\nGraphics tests: {} passed, {} failed\n",
        total_passed, total_failed
    );
}

#[test]
fn test_visual_fonts() {
    println!("\n=== Font Rendering Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in FONT_TESTS {
        if !resolve_repo_path(test.path).exists() {
            println!("⊘ Skipping {} (file not found)", test.name);
            continue;
        }

        println!("Testing: {}", test.name);

        match run_test_case(test) {
            Ok(result) => {
                for page_result in &result.page_results {
                    match &page_result.status {
                        RenderStatus::Success => {
                            println!(
                                "  ✓ Page {}: {:.2}% non-white",
                                page_result.page_num, page_result.percentage
                            );
                            total_passed += 1;
                        }
                        RenderStatus::Failed(e) => {
                            println!("  ✗ Page {}: {}", page_result.page_num, e);
                            total_failed += 1;
                        }
                    }
                }
            }
            Err(e) => {
                println!("  ✗ Failed: {}", e);
                total_failed += 1;
            }
        }
    }

    println!(
        "\nFont tests: {} passed, {} failed\n",
        total_passed, total_failed
    );
}

#[test]
fn test_visual_complex_documents() {
    println!("\n=== Complex Document Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in COMPLEX_TESTS {
        if !resolve_repo_path(test.path).exists() {
            println!("⊘ Skipping {} (file not found)", test.name);
            continue;
        }

        println!("Testing: {}", test.name);

        match run_test_case(test) {
            Ok(result) => {
                for page_result in &result.page_results {
                    match &page_result.status {
                        RenderStatus::Success => {
                            println!(
                                "  ✓ Page {}: {:.2}% non-white",
                                page_result.page_num, page_result.percentage
                            );
                            total_passed += 1;
                        }
                        RenderStatus::Failed(e) => {
                            println!("  ✗ Page {}: {}", page_result.page_num, e);
                            total_failed += 1;
                        }
                    }
                }
            }
            Err(e) => {
                println!("  ✗ Failed: {}", e);
                total_failed += 1;
            }
        }
    }

    println!(
        "\nComplex document tests: {} passed, {} failed\n",
        total_passed, total_failed
    );
}

#[test]
fn test_visual_all() {
    println!("\n=== All Visual Tests ===\n");
    println!("Output directory: {}\n", OUTPUT_DIR);

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_pages = 0;
    let mut total_passed = 0;
    let mut total_failed = 0;
    let mut category_stats: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();

    for test_group in ALL_TESTS {
        for test in *test_group {
            if !resolve_repo_path(test.path).exists() {
                continue;
            }

            let category_name = format!("{:?}", test.category);

            match run_test_case(test) {
                Ok(result) => {
                    total_pages += result.total_pages;

                    for page_result in &result.page_results {
                        match &page_result.status {
                            RenderStatus::Success => {
                                total_passed += 1;
                                let entry = category_stats
                                    .entry(category_name.clone())
                                    .or_insert((0, 0));
                                entry.0 += 1;
                            }
                            RenderStatus::Failed(_) => {
                                total_failed += 1;
                                let entry = category_stats
                                    .entry(category_name.clone())
                                    .or_insert((0, 0));
                                entry.1 += 1;
                            }
                        }
                    }
                }
                Err(_) => {
                    total_failed += test.pages.len();
                }
            }
        }
    }

    // Print summary by category
    println!("\n=== Summary by Category ===");
    let mut categories: Vec<_> = category_stats.iter().collect();
    categories.sort_by(|a, b| a.0.cmp(b.0));

    for (category, (passed, failed)) in categories {
        let total = passed + failed;
        println!(
            "{:20}: {} passed, {} failed (total: {})",
            category, passed, failed, total
        );
    }

    println!("\n=== Overall Summary ===");
    println!("Total pages: {}", total_pages);
    println!("Total passed: {}", total_passed);
    println!("Total failed: {}", total_failed);
    println!("Output directory: {}", OUTPUT_DIR);
    println!("\nTo view images:");
    println!("  feh {}/*.png", OUTPUT_DIR);
}

/// Quick smoke test - render one page from each category
#[test]
fn test_visual_smoke_comprehensive() {
    println!("\n=== Comprehensive Smoke Test ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut passed = 0;
    let mut failed = 0;

    // Pick one test from each category
    let smoke_tests: Vec<&TestCase> = vec![
        &TEXT_TESTS[0],
        &ANNOTATION_TESTS[0],
        &GRAPHICS_TESTS.get(0).unwrap_or(&TEXT_TESTS[0]),
    ];

    for test in smoke_tests {
        if !resolve_repo_path(test.path).exists() {
            println!("⊘ Skipping {} (file not found)", test.name);
            continue;
        }

        println!("Testing: {} ({:?})", test.name, test.category);

        match run_test_case(test) {
            Ok(result) => {
                if result.pages_rendered > 0 {
                    println!("  ✓ Rendered {} pages", result.pages_rendered);
                    passed += 1;
                } else {
                    println!("  ✗ No pages rendered");
                    failed += 1;
                }
            }
            Err(e) => {
                println!("  ✗ Failed: {}", e);
                failed += 1;
            }
        }
    }

    println!("\nSmoke test: {} passed, {} failed\n", passed, failed);

    if failed == 0 {
        println!("✓ All smoke tests passed!");
    } else {
        println!("✗ Some smoke tests failed");
        panic!("Smoke test failures");
    }
}
