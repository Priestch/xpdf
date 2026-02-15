//! Visual rendering tests for PDF-X.
//!
//! This test module renders PDF files and saves them as PNG images for visual inspection.
//! It provides a quick way to verify rendering quality without manual Tauri testing.

use pdf_x_core::rendering::Device;
use pdf_x_core::rendering::skia_device::SkiaDevice;
use tiny_skia::Pixmap;

#[cfg(feature = "rendering")]

/// Test PDFs to render for visual verification
const TEST_PDFS: &[(&str, &str)] = &[
    // (name, path)
    (
        "annotation_line",
        "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-line.pdf",
    ),
    (
        "issue19802",
        "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue19802.pdf",
    ),
    (
        "issue7200",
        "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf",
    ),
    // Add test PDFs from /home/gp/Books if available
    ("1807.03341v2", "/home/gp/Books/1807.03341v2.pdf"),
];

/// Render a single PDF page to PNG
fn render_pdf_to_png(
    pdf_path: &str,
    output_dir: &str,
    page_numbers: Option<&[usize]>,
) -> Result<usize, String> {
    // Read PDF file
    let pdf_bytes = std::fs::read(pdf_path).map_err(|e| format!("Failed to read PDF: {}", e))?;

    // Parse PDF
    let mut doc = pdf_x_core::PDFDocument::open(pdf_bytes)
        .map_err(|e| format!("Failed to parse PDF: {:?}", e))?;

    let page_count = doc
        .page_count()
        .map_err(|e| format!("Failed to get page count: {:?}", e))?;

    // Determine which pages to render
    let pages_to_render: Vec<usize> = match page_numbers {
        Some(nums) => nums
            .iter()
            .filter(|&&p| p < page_count as usize)
            .copied()
            .collect(),
        None => (0..page_count as usize).collect(),
    };

    let mut rendered_count = 0;

    for page_num in pages_to_render {
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

        // Create pixmap at 2x scale for better quality
        let scale = 2.0f64;
        let width = (page_width as f64 * scale).ceil() as u32;
        let height = (page_height as f64 * scale).ceil() as u32;

        let mut pixmap = Pixmap::new(width, height).ok_or("Failed to create pixmap")?;

        // Fill with white background
        pixmap.fill(tiny_skia::Color::WHITE);

        // Create rendering device
        let mut device = SkiaDevice::new(pixmap.as_mut());

        // Apply coordinate transform (PDF Y-up to screen Y-down)
        device.set_matrix(&[
            scale,
            0.0,
            0.0,
            -scale,
            -(x0 as f64) * scale,
            (y1 as f64) * scale,
        ]);

        // Render the page
        match page.render(&mut doc.xref_mut(), &mut device) {
            Ok(_) => {}
            Err(e) => eprintln!("Warning: Page {} render error: {:?}", page_num, e),
        }

        // Create output directory
        std::fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;

        // Save PNG
        let pdf_name = std::path::Path::new(pdf_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");

        let filename = format!("{}_page_{}.png", pdf_name, page_num);
        let filepath = std::path::Path::new(output_dir).join(&filename);

        pixmap
            .save_png(&filepath)
            .map_err(|e| format!("Failed to save PNG: {}", e))?;

        // Calculate pixel statistics
        let pixels = pixmap.data();
        let non_white_count = pixels
            .chunks(4)
            .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
            .count();

        let total_pixels = pixels.len() / 4;
        let percentage = (non_white_count as f64 / total_pixels as f64) * 100.0;

        println!(
            "  ✓ Saved {} ({}x{}, {:.2}% non-white)",
            filename, width, height, percentage
        );

        rendered_count += 1;
    }

    Ok(rendered_count)
}

/// Run all visual tests
#[test]
#[cfg(feature = "rendering")]
fn test_visual_rendering() {
    let output_dir = "/tmp/pdf-x-visual-tests";

    println!("\n=== PDF-X Visual Rendering Tests ===");
    println!("Output directory: {}\n", output_dir);

    let mut total_rendered = 0;
    let mut total_failed = 0;

    for (name, path) in TEST_PDFS {
        println!("Testing: {} ({})", name, path);

        // Check if file exists
        if !std::path::Path::new(path).exists() {
            println!("  ⊘ File not found, skipping\n");
            continue;
        }

        // Render first 3 pages only (for quick testing)
        let pages_to_render: &[usize] = &[0, 1, 2];

        match render_pdf_to_png(path, output_dir, Some(pages_to_render)) {
            Ok(count) => {
                println!("  ✓ Rendered {} pages", count);
                total_rendered += count;
            }
            Err(e) => {
                println!("  ✗ Failed: {}", e);
                total_failed += 1;
            }
        }
        println!();
    }

    // Print summary
    println!("=== Summary ===");
    println!("Total pages rendered: {}", total_rendered);
    println!("Total failures: {}", total_failed);
    println!("Output directory: {}", output_dir);
    println!("\nTo view images:");
    println!("  feh {}/*.png", output_dir);
    println!("  # or");
    println!("  eog {}/*.png", output_dir);
    println!("  # or");
    println!("  open {}", output_dir);
}

/// Test rendering a specific PDF from /home/gp/Books
#[test]
#[cfg(feature = "rendering")]
fn test_visual_books_pdf() {
    let test_path = "/home/gp/Books/1807.03341v2.pdf";

    if !std::path::Path::new(test_path).exists() {
        println!("Skipping test - {} not found", test_path);
        return;
    }

    println!("\n=== Visual Test: Books PDF ===");
    println!("File: {}", test_path);

    let output_dir = "/tmp/pdf-x-visual-tests";

    match render_pdf_to_png(test_path, output_dir, Some(&[0, 1])) {
        Ok(count) => println!("✓ Rendered {} pages successfully", count),
        Err(e) => eprintln!("✗ Failed: {}", e),
    }
}

/// Test rendering with different page ranges
#[test]
#[cfg(feature = "rendering")]
fn test_visual_page_ranges() {
    let test_path = "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-line.pdf";

    if !std::path::Path::new(test_path).exists() {
        println!("Skipping test - {} not found", test_path);
        return;
    }

    println!("\n=== Visual Test: Page Ranges ===");

    let output_dir = "/tmp/pdf-x-visual-tests/range-test";

    // Render only the first page
    match render_pdf_to_png(test_path, output_dir, Some(&[0])) {
        Ok(_) => println!("✓ Single page render successful"),
        Err(e) => eprintln!("✗ Failed: {}", e),
    }
}

/// Quick smoke test - render one page from one PDF
#[test]
#[cfg(feature = "rendering")]
fn test_visual_smoke() {
    // Find the first available PDF
    let test_pdf = TEST_PDFS
        .iter()
        .find(|(_, path)| std::path::Path::new(path).exists())
        .map(|(_, path)| *path);

    let pdf_path = match test_pdf {
        Some(p) => p,
        None => {
            println!("No test PDFs found, skipping smoke test");
            return;
        }
    };

    println!("\n=== Visual Smoke Test ===");
    println!("File: {}", pdf_path);

    let output_dir = "/tmp/pdf-x-visual-tests/smoke";

    match render_pdf_to_png(pdf_path, output_dir, Some(&[0])) {
        Ok(_) => {
            println!("✓ Smoke test passed");
            println!("Output: {}/smoke/*.png", output_dir);
        }
        Err(e) => {
            eprintln!("✗ Smoke test failed: {}", e);
            panic!("Visual smoke test failed");
        }
    }
}
