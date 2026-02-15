//! Rendering logic analysis tests.
//!
//! This test module analyzes the rendering quality and identifies issues.

use pdf_x_core::rendering::Device;
use pdf_x_core::rendering::skia_device::SkiaDevice;
use tiny_skia::Pixmap;

#[cfg(feature = "rendering")]

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
    fn from_pixmap(pixmap: &Pixmap) -> Self {
        let pixels = pixmap.data();
        let width = pixmap.width();
        let height = pixmap.height();

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

/// Render and analyze a PDF page
fn analyze_page(pdf_path: &str, page_num: usize) -> Result<RenderAnalysis, String> {
    let pdf_bytes = std::fs::read(pdf_path).map_err(|e| format!("Failed to read PDF: {}", e))?;

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

    let page = doc
        .get_page(page_num)
        .map_err(|e| format!("Failed to get page {}: {:?}", page_num, e))?;

    // Get page dimensions
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
        _ => (0.0, 0.0, 612.0, 792.0),
    };

    let scale = 2.0;
    let width = ((x1 - x0).ceil() as f64 * scale).ceil() as u32;
    let height = ((y1 - y0).ceil() as f64 * scale).ceil() as u32;

    let mut pixmap = Pixmap::new(width, height).ok_or("Failed to create pixmap")?;

    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    device.set_matrix(&[
        scale,
        0.0,
        0.0,
        -scale,
        -(x0 as f64) * scale,
        (y1 as f64) * scale,
    ]);

    page.render(&mut doc.xref_mut(), &mut device)
        .map_err(|e| format!("Render error: {:?}", e))?;

    Ok(RenderAnalysis::from_pixmap(&pixmap))
}

// ============================================================================
// Analysis Tests
// ============================================================================

#[test]
#[cfg(feature = "rendering")]
fn test_analyze_text_rendering_quality() {
    println!("\n=== Text Rendering Quality Analysis ===\n");

    let test_cases = vec![
        (
            "Simple text",
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf",
            0,
        ),
        ("Academic paper", "/home/gp/Books/1807.03341v2.pdf", 0),
        ("Deep Learning", "/home/gp/Books/d2l-en.pdf", 0),
    ];

    let mut total_content_pages = 0;
    let mut total_blank_pages = 0;

    for (name, path, page) in test_cases {
        if !std::path::Path::new(path).exists() {
            println!("⊘ Skipping {} (file not found)", name);
            continue;
        }

        print!("{}... ", name);

        match analyze_page(path, page) {
            Ok(analysis) => {
                println!(
                    "{} ({:.2}% non-white)",
                    analysis.quality_score(),
                    analysis.percentage
                );

                if analysis.is_blank {
                    total_blank_pages += 1;
                    println!("  ⚠ WARNING: Page is completely blank!");
                } else {
                    total_content_pages += 1;

                    if let Some((min_x, min_y, max_x, max_y)) = analysis.content_bounds {
                        let content_width = max_x - min_x;
                        let content_height = max_y - min_y;
                        println!(
                            "  Content bounds: {}x{} at ({}, {})",
                            content_width, content_height, min_x, min_y
                        );
                    }
                }
            }
            Err(e) => {
                println!("FAILED: {}", e);
            }
        }
    }

    println!("\nContent pages: {}", total_content_pages);
    println!("Blank pages: {}", total_blank_pages);

    assert!(total_content_pages > 0, "No content pages rendered");
}

#[test]
#[cfg(feature = "rendering")]
fn test_analyze_font_coverage() {
    println!("\n=== Font Coverage Analysis ===\n");

    // Test different PDFs to see font rendering coverage
    let test_cases = vec![
        (
            "TrueType font",
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf",
            0,
        ),
        ("Type1/CFF font", "/home/gp/Books/1807.03341v2.pdf", 1),
    ];

    for (name, path, page) in test_cases {
        if !std::path::Path::new(path).exists() {
            continue;
        }

        println!("Testing: {}", name);

        match analyze_page(path, page) {
            Ok(analysis) => {
                println!("  Coverage: {:.2}%", analysis.percentage);
                println!("  Quality: {}", analysis.quality_score());

                if analysis.percentage < 1.0 {
                    println!("  ⚠ WARNING: Very low coverage - font may not be rendering");
                }
            }
            Err(e) => {
                println!("  ✗ Error: {}", e);
            }
        }
        println!();
    }
}

#[test]
#[cfg(feature = "rendering")]
fn test_analyze_graphics_quality() {
    println!("\n=== Graphics Rendering Quality ===\n");

    let test_cases = vec![
        (
            "Alpha transparency",
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/alphatrans.pdf",
            0,
        ),
        (
            "Annotation line",
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-line.pdf",
            0,
        ),
        (
            "Complex graphics",
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue19802.pdf",
            0,
        ),
    ];

    for (name, path, page) in test_cases {
        if !std::path::Path::new(path).exists() {
            continue;
        }

        print!("{}... ", name);

        match analyze_page(path, page) {
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
#[cfg(feature = "rendering")]
fn test_rendering_regression_detection() {
    println!("\n=== Regression Detection ===\n");
    println!("Testing for rendering regressions compared to expected quality\n");

    // These are expected quality thresholds based on previous successful runs
    let expected_quality = vec![
        (
            "1807.03341v2_p0",
            "/home/gp/Books/1807.03341v2.pdf",
            0,
            15.0,
        ), // Text-heavy
        (
            "1807.03341v2_p1",
            "/home/gp/Books/1807.03341v2.pdf",
            1,
            10.0,
        ), // More text
        (
            "issue7200_p0",
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf",
            0,
            5.0,
        ),
        (
            "annotation-line",
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-line.pdf",
            0,
            10.0,
        ),
    ];

    let mut regressions = vec![];

    for (name, path, page, min_expected_percentage) in expected_quality {
        if !std::path::Path::new(path).exists() {
            continue;
        }

        match analyze_page(path, page) {
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
#[cfg(feature = "rendering")]
fn test_comprehensive_rendering_audit() {
    println!("\n=== Comprehensive Rendering Audit ===\n");

    let mut audit_results = std::collections::HashMap::new();
    let mut total_pages = 0;
    let mut blank_pages = 0;
    let mut low_quality_pages = 0;

    // Test a variety of PDFs
    let test_pdfs = vec![
        (
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf",
            vec![0],
        ),
        (
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-line.pdf",
            vec![0],
        ),
        (
            "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-highlight.pdf",
            vec![0],
        ),
        ("/home/gp/Books/1807.03341v2.pdf", vec![0, 1, 2]),
        ("/home/gp/Books/d2l-en.pdf", vec![0, 1]),
    ];

    for (path, pages) in test_pdfs {
        if !std::path::Path::new(path).exists() {
            continue;
        }

        let pdf_name = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        for page in pages {
            total_pages += 1;

            match analyze_page(path, page) {
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
