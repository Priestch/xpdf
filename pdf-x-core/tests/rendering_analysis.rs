//! Rendering logic analysis tests.
//!
//! This test module analyzes the rendering quality and identifies issues.

#![cfg(feature = "rendering")]

use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::RenderToImage;

/// Analyze a single rendered page
struct RenderAnalysis {
    total_pixels: u32,
    non_white_pixels: u32,
    percentage: f64,
    content_bounds: Option<(u32, u32, u32, u32)>, // (min_x, min_y, max_x, max_y)
    has_content: bool,
    is_blank: bool,
}

impl RenderAnalysis {
    fn from_rgba(width: u32, height: u32, pixels: &[u8]) -> Self {
        let total_pixels = (width * height) as u32;
        let mut non_white_pixels = 0u32;
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0u32;
        let mut max_y = 0u32;

        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                let r = pixels[idx];
                let g = pixels[idx + 1];
                let b = pixels[idx + 2];

                if r < 250 || g < 250 || b < 250 {
                    non_white_pixels += 1;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }

        let percentage = (non_white_pixels as f64 / total_pixels as f64) * 100.0;
        let has_content = non_white_pixels > 0;
        let is_blank = non_white_pixels == 0;
        let content_bounds = if has_content {
            Some((min_x, min_y, max_x, max_y))
        } else {
            None
        };

        RenderAnalysis {
            total_pixels,
            non_white_pixels,
            percentage,
            content_bounds,
            has_content,
            is_blank,
        }
    }

    fn quality_score(&self) -> &'static str {
        if self.is_blank {
            "BLANK"
        } else if self.percentage < 0.5 {
            "VERY LOW"
        } else if self.percentage < 2.0 {
            "LOW"
        } else if self.percentage < 10.0 {
            "MODERATE"
        } else if self.percentage < 30.0 {
            "GOOD"
        } else {
            "EXCELLENT"
        }
    }
}

/// Resolve a repo-relative test PDF path against the workspace root
/// (`pdf-x-core/tests` → `../..`). Fixture PDFs ship in the pdf.js submodule
/// (`pdf.js/test/pdfs/`) and in `tests/fixtures/pdfs/`.
fn resolve_repo_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Render and analyze a PDF page.
///
/// Uses the `render_page_to_image` path (resolve → interpret, scale applied
/// inside interpret, annotation appearances included) rather than a manual
/// `Page::render` + pre-set device matrix: `Page::render` installs its own
/// unscaled page-initial CTM, which overwrites the caller's scale matrix and
/// leaves 1× content in the corner of a scaled canvas.
fn analyze_page(pdf_path: &str, page_num: usize) -> Result<RenderAnalysis, String> {
    let pdf_bytes = std::fs::read(pdf_path).map_err(|e| format!("Failed to read PDF: {}", e))?;

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

    let (width, height, rgba) = doc
        .render_page_to_image(page_num, Some(2.0))
        .map_err(|e| format!("Render error: {:?}", e))?;

    Ok(RenderAnalysis::from_rgba(width, height, &rgba))
}

// ============================================================================
// Analysis Tests
// ============================================================================

#[test]
fn test_analyze_text_rendering_quality() {
    println!("\n=== Text Rendering Quality Analysis ===\n");

    let test_cases = vec![
        ("Simple text", "pdf.js/test/pdfs/issue7200.pdf", 0),
        ("Annotation line", "pdf.js/test/pdfs/annotation-line.pdf", 0),
        ("Complex graphics", "pdf.js/test/pdfs/issue19802.pdf", 0),
    ];

    for (name, path, page) in test_cases {
        let path = resolve_repo_path(path);
        if !path.exists() {
            continue;
        }

        print!("{}... ", name);

        match analyze_page(&path.to_string_lossy(), page) {
            Ok(analysis) => {
                println!("{} ({:.2}%)", analysis.quality_score(), analysis.percentage);

                if analysis.is_blank {
                    println!("  ⚠ WARNING: Graphics not rendering!");
                }
            }
            Err(e) => {
                println!("FAILED: {}", e);
            }
        }
    }
}

#[test]
fn test_rendering_regression_detection() {
    println!("\n=== Regression Detection ===\n");
    println!("Testing for rendering regressions compared to expected quality\n");

    // Minimum non-white percentages measured from current correct renders
    // (scale 2, via render_page_to_image). These assert *presence* of
    // content — a blank-page regression renders 0.00%. They are NOT density
    // targets: annotation-line.pdf's page contains only the word "Line" and
    // one line annotation (~0.17% non-white), so a high threshold can never
    // pass on a correct render.
    let expected_quality = vec![
        ("issue7200", "pdf.js/test/pdfs/issue7200.pdf", 0, 5.0), // measured 10.84%
        (
            "annotation-line",
            "pdf.js/test/pdfs/annotation-line.pdf",
            0,
            0.05,
        ), // measured 0.17%
    ];

    let mut regressions = vec![];

    for (name, path, page, min_expected_percentage) in expected_quality {
        let path = resolve_repo_path(path);
        if !path.exists() {
            continue;
        }

        match analyze_page(&path.to_string_lossy(), page) {
            Ok(analysis) => {
                let status = if analysis.percentage < min_expected_percentage {
                    println!(
                        "✗ {}: {:.2}% (expected >= {:.2}%) - REGRESSION",
                        name, analysis.percentage, min_expected_percentage
                    );
                    regressions.push(name.to_string());
                } else {
                    println!(
                        "✓ {}: {:.2}% (expected >= {:.2}%)",
                        name, analysis.percentage, min_expected_percentage
                    );
                };
            }
            Err(e) => {
                println!("✗ {}: Error - {}", name, e);
                regressions.push(name.to_string());
            }
        }
    }

    println!();

    if regressions.is_empty() {
        println!("✓ No regressions detected");
    } else {
        println!("✗ Regressions detected in: {}", regressions.join(", "));
        panic!("Rendering regressions detected");
    }
}

#[test]
fn test_comprehensive_rendering_audit() {
    println!("\n=== Comprehensive Rendering Audit ===\n");

    let mut audit_results = std::collections::HashMap::new();
    let mut total_pages = 0;
    let mut blank_pages = 0;
    let mut low_quality_pages = 0;

    // Test a variety of PDFs
    let test_pdfs = vec![
        ("pdf.js/test/pdfs/issue7200.pdf", vec![0]),
        ("pdf.js/test/pdfs/annotation-line.pdf", vec![0]),
        ("pdf.js/test/pdfs/annotation-highlight.pdf", vec![0]),
    ];

    for (path, pages) in test_pdfs {
        let path = resolve_repo_path(path);
        if !path.exists() {
            continue;
        }

        let pdf_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        for page in pages {
            total_pages += 1;

            match analyze_page(&path.to_string_lossy(), page) {
                Ok(analysis) => {
                    let entry = audit_results
                        .entry(pdf_name.to_string())
                        .or_insert_with(|| vec![]);

                    entry.push((page, analysis.percentage, analysis.quality_score()));

                    if analysis.is_blank {
                        blank_pages += 1;
                    } else if analysis.percentage < 2.0 {
                        low_quality_pages += 1;
                    }
                }
                Err(_) => {
                    blank_pages += 1;
                }
            }
        }
    }

    // Print audit results
    println!("Total pages tested: {}", total_pages);
    println!(
        "Blank pages: {} ({:.1}%)",
        blank_pages,
        (blank_pages as f64 / total_pages as f64) * 100.0
    );
    println!(
        "Low quality pages: {} ({:.1}%)",
        low_quality_pages,
        (low_quality_pages as f64 / total_pages as f64) * 100.0
    );

    println!("\nDetailed results:");
    for (pdf_name, results) in &audit_results {
        println!("\n{}:", pdf_name);
        for (page, percentage, quality) in results {
            println!("  Page {}: {:.2}% - {}", page, percentage, quality);
        }
    }

    println!();

    // Fail if more than 20% of pages are blank (allowing for some edge cases)
    let blank_percentage = (blank_pages as f64 / total_pages as f64) * 100.0;
    if blank_percentage > 20.0 {
        panic!(
            "Too many blank pages: {:.1}% (> 20% threshold)",
            blank_percentage
        );
    }

    println!("✓ Rendering audit passed");
}
