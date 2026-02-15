//! Comprehensive visual rendering tests for PDF-X.
//!
//! This test suite provides comprehensive visual testing by rendering PDFs
//! and saving them as PNG images for inspection. Inspired by hayro's test
//! infrastructure but adapted for pdf-x's testing needs.

use pdf_x_core::rendering::Device;
use pdf_x_core::rendering::skia_device::SkiaDevice;
use tiny_skia::Pixmap;

#[cfg(feature = "rendering")]

// ============================================================================
// Test Configuration
// ============================================================================

const OUTPUT_DIR: &str = "/tmp/pdf-x-visual-tests";
const SCALE: f64 = 2.0; // Render at 2x for better quality

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
const TEXT_TESTS: &[TestCase] = &[
    TestCase {
        name: "simple_text",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf",
        pages: &[0],
        category: TestCategory::Text,
    },
    TestCase {
        name: "academic_paper",
        path: "/home/gp/Books/1807.03341v2.pdf",
        pages: &[0, 1, 2, 3],
        category: TestCategory::Text,
    },
    TestCase {
        name: "deep_learning_book",
        path: "/home/gp/Books/d2l-en.pdf",
        pages: &[0, 1, 50, 100],
        category: TestCategory::Text,
    },
];

/// Annotation tests
const ANNOTATION_TESTS: &[TestCase] = &[
    TestCase {
        name: "annotation_line",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-line.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
    TestCase {
        name: "annotation_highlight",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-highlight.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
    TestCase {
        name: "annotation_link",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-link-text-popup.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
    TestCase {
        name: "annotation_square_circle",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-square-circle.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
    TestCase {
        name: "annotation_freetext",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-freetext.pdf",
        pages: &[0],
        category: TestCategory::Annotations,
    },
];

/// Graphics and path tests
const GRAPHICS_TESTS: &[TestCase] = &[
    TestCase {
        name: "alpha_transparency",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/alphatrans.pdf",
        pages: &[0],
        category: TestCategory::Graphics,
    },
    TestCase {
        name: "issue19802",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue19802.pdf",
        pages: &[0],
        category: TestCategory::Graphics,
    },
];

/// Font rendering tests
const FONT_TESTS: &[TestCase] = &[
    TestCase {
        name: "truetype_font",
        path: "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf",
        pages: &[0],
        category: TestCategory::Fonts,
    },
    TestCase {
        name: "cjk_text",
        path: "/home/gp/Books/人类起源的故事.pdf",
        pages: &[0, 1],
        category: TestCategory::Fonts,
    },
];

/// Complex document tests
const COMPLEX_TESTS: &[TestCase] = &[
    TestCase {
        name: "pdf_reference",
        path: "/home/gp/Books/pdf17.pdf",
        pages: &[0, 1, 50, 100, 500],
        category: TestCategory::Complex,
    },
    TestCase {
        name: "programming_rust",
        path: "/home/gp/Books/Programming Rust.pdf",
        pages: &[0, 1, 50],
        category: TestCategory::Complex,
    },
    TestCase {
        name: "ml_book",
        path: "/home/gp/Books/Deep Learning by Ian Goodfellow, Yoshua Bengio, Aaron Courville (z-lib.org).pdf",
        pages: &[0, 1, 2],
        category: TestCategory::Complex,
    },
];

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
    let mut doc = pdf_x_core::PDFDocument::open(pdf_bytes)
        .map_err(|e| format!("Failed to parse PDF: {:?}", e))?;

    let page_count = doc
        .page_count()
        .map_err(|e| format!("Failed to get page count: {:?}", e))?;

    if page_num >= page_count as usize {
        return Err(format!(
            "Page {} out of range (max {})",
            page_num, page_count
        ));
    }

    // Get the page
    let page = doc
        .get_page(page_num)
        .map_err(|e| format!("Failed to get page {}: {:?}", page_num, e))?;

    // Get page dimensions from MediaBox
    let (x0, y0, x1, y1) = match page.media_box() {
        Some(pdf_x_core::PDFObject::Array(arr)) if arr.len() >= 4 => {
            let get_value = |i: usize| -> f64 {
                match &**&arr[i] {
                    pdf_x_core::PDFObject::Number(n) => n.max(0.0),
                    _ => 0.0,
                }
            };
            (get_value(0), get_value(1), get_value(2), get_value(3))
        }
        _ => (0.0, 0.0, 612.0, 792.0), // Default US Letter
    };

    let page_width = (x1 - x0).ceil() as u32;
    let page_height = (y1 - y0).ceil() as u32;

    // Create pixmap at specified scale
    let width = (page_width as f64 * SCALE).ceil() as u32;
    let height = (page_height as f64 * SCALE).ceil() as u32;

    let mut pixmap = Pixmap::new(width, height).ok_or("Failed to create pixmap")?;

    // Fill with white background
    pixmap.fill(tiny_skia::Color::WHITE);

    // Create rendering device
    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Apply coordinate transform (PDF Y-up to screen Y-down)
    device.set_matrix(&[
        SCALE,
        0.0,
        0.0,
        -SCALE,
        -(x0 as f64) * SCALE,
        (y1 as f64) * SCALE,
    ]);

    // Render the page
    page.render(&mut doc.xref_mut(), &mut device)
        .map_err(|e| format!("Render error: {:?}", e))?;

    // Calculate pixel statistics
    let pixels = pixmap.data();
    let non_white_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
        .count();

    let total_pixels = (pixels.len() / 4) as u32;
    let percentage = (non_white_count as f64 / total_pixels as f64) * 100.0;

    // Save PNG
    pixmap
        .save_png(output_path)
        .map_err(|e| format!("Failed to save PNG: {}", e))?;

    Ok((percentage, total_pixels))
}

/// Run a single test case
fn run_test_case(test: &TestCase) -> Result<TestResult, String> {
    let pdf_name = std::path::Path::new(test.path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let mut pages_rendered = 0;
    let mut results = Vec::new();

    for &page_num in test.pages {
        let filename = format!("{}_{}_page_{}.png", test.name, pdf_name, page_num);
        let filepath = std::path::Path::new(OUTPUT_DIR).join(&filename);

        match render_page(test.path, page_num, filepath.to_str().unwrap()) {
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
#[cfg(feature = "rendering")]
fn test_visual_text_rendering() {
    println!("\n=== Text Rendering Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in TEXT_TESTS {
        if !std::path::Path::new(test.path).exists() {
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
#[cfg(feature = "rendering")]
fn test_visual_annotations() {
    println!("\n=== Annotation Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in ANNOTATION_TESTS {
        if !std::path::Path::new(test.path).exists() {
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
#[cfg(feature = "rendering")]
fn test_visual_graphics() {
    println!("\n=== Graphics Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in GRAPHICS_TESTS {
        if !std::path::Path::new(test.path).exists() {
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
#[cfg(feature = "rendering")]
fn test_visual_fonts() {
    println!("\n=== Font Rendering Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in FONT_TESTS {
        if !std::path::Path::new(test.path).exists() {
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
#[cfg(feature = "rendering")]
fn test_visual_complex_documents() {
    println!("\n=== Complex Document Tests ===\n");

    std::fs::create_dir_all(OUTPUT_DIR).ok();

    let mut total_passed = 0;
    let mut total_failed = 0;

    for test in COMPLEX_TESTS {
        if !std::path::Path::new(test.path).exists() {
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
#[cfg(feature = "rendering")]
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
            if !std::path::Path::new(test.path).exists() {
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
#[cfg(feature = "rendering")]
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
        if !std::path::Path::new(test.path).exists() {
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
