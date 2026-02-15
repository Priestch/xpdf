//! Debug test to verify PDF image rendering
//!
//! This test uses real PDF files to debug rendering issues.

use pdf_x_core::rendering::{Device, SkiaDevice};
use tiny_skia::Pixmap;

/// Find the test PDFs directory
fn get_test_pdf_path() -> Option<String> {
    let paths = [
        "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/annotation-line.pdf",
        "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue19802.pdf",
        "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf",
    ];

    for path in paths {
        if std::path::Path::new(path).exists() {
            return Some(path.to_string());
        }
    }

    None
}

#[test]
#[cfg(feature = "rendering")]
fn test_debug_real_pdf_rendering() {
    let pdf_path = match get_test_pdf_path() {
        Some(p) => {
            println!("DEBUG: Using test PDF: {}", p);
            p
        }
        None => {
            println!("DEBUG: No test PDF found, skipping test");
            return;
        }
    };

    println!("DEBUG: Reading PDF file...");
    let pdf_bytes = match std::fs::read(&pdf_path) {
        Ok(b) => {
            println!("DEBUG: Read {} bytes", b.len());
            b
        }
        Err(e) => {
            eprintln!("ERROR: Failed to read PDF: {:?}", e);
            panic!("Failed to read PDF: {:?}", e);
        }
    };

    println!("DEBUG: Parsing PDF...");
    let mut doc = match pdf_x_core::PDFDocument::open(pdf_bytes) {
        Ok(d) => {
            println!("DEBUG: PDF parsed successfully");
            d
        }
        Err(e) => {
            eprintln!("ERROR: Failed to parse PDF: {:?}", e);
            panic!("PDF parsing failed: {:?}", e);
        }
    };

    let page_count = match doc.page_count() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("ERROR: Failed to get page count: {:?}", e);
            panic!("Failed to get page count: {:?}", e);
        }
    };
    println!("DEBUG: Page count: {}", page_count);

    // Get the first page
    let page = match doc.get_page(0) {
        Ok(p) => {
            println!("DEBUG: Got page 0");
            p
        }
        Err(e) => {
            eprintln!("ERROR: Failed to get page: {:?}", e);
            panic!("Failed to get page: {:?}", e);
        }
    };

    // Get page dimensions
    let (x0, y0, x1, y1) = match page.media_box() {
        Some(pdf_x_core::PDFObject::Array(arr)) if arr.len() >= 4 => {
            let get_value = |i: usize| -> f32 {
                match &**&arr[i] {
                    pdf_x_core::PDFObject::Number(n) => n.max(0.0) as f32,
                    _ => 0.0,
                }
            };
            (get_value(0), get_value(1), get_value(2), get_value(3))
        }
        _ => (0.0, 0.0, 612.0, 792.0), // Default US Letter
    };

    let page_width = (x1 - x0).ceil() as u32;
    let page_height = (y1 - y0).ceil() as u32;

    println!(
        "DEBUG: MediaBox=[{},{}, {}, {}], page_size={}x{}",
        x0, y0, x1, y1, page_width, page_height
    );

    // Create pixmap for rendering at 2x scale
    let scale = 2.0f64;
    let width = (page_width as f64 * scale).ceil() as u32;
    let height = (page_height as f64 * scale).ceil() as u32;

    println!("DEBUG: Creating {}x{} pixmap...", width, height);
    let mut pixmap = Pixmap::new(width, height).expect("Failed to create pixmap");

    // Fill with white background
    pixmap.fill(tiny_skia::Color::WHITE);
    println!("DEBUG: Filled background with white");

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
    println!("DEBUG: Set coordinate transform");

    // Render the page
    println!("DEBUG: Starting page render...");
    match page.render(&mut doc.xref_mut(), &mut device) {
        Ok(_) => println!("DEBUG: Page render completed"),
        Err(e) => {
            eprintln!("ERROR: Page render failed: {:?}", e);
            // Don't panic - we want to see what was rendered
            println!("DEBUG: Render error: {:?}", e);
        }
    }

    // Check if we have any non-white pixels
    let pixels = pixmap.data();
    let non_white_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
        .count();

    println!("DEBUG: Total pixels: {}", pixels.len() / 4);
    println!(
        "DEBUG: Non-white pixels: {} ({:.2}%)",
        non_white_count,
        (non_white_count as f64 / (pixels.len() as f64 / 4.0)) * 100.0
    );

    // Find the bounds of non-white content
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
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if non_white_count > 0 {
        println!(
            "DEBUG: Content bounds: x=[{}, {}], y=[{}, {}]",
            min_x, max_x, min_y, max_y
        );

        // Sample a few pixels within the content bounds
        println!("DEBUG: Sample pixels:");
        for y in min_y..=max_y.min(min_y + 5) {
            for x in min_x..=max_x.min(min_x + 5) {
                let idx = ((y * width + x) * 4) as usize;
                let r = pixels[idx];
                let g = pixels[idx + 1];
                let b = pixels[idx + 2];
                let a = pixels[idx + 3];
                println!("  Pixel at ({}, {}): RGBA=({},{},{},{})", x, y, r, g, b, a);
            }
        }
    }

    // Save the rendered image for inspection
    let output_path = "/tmp/debug_render_output.png";
    match pixmap.save_png(output_path) {
        Ok(_) => println!("DEBUG: Saved rendered image to {}", output_path),
        Err(e) => eprintln!("ERROR: Failed to save PNG: {:?}", e),
    }

    // Print summary
    println!("\n=== RENDER SUMMARY ===");
    if non_white_count == 0 {
        println!("WARNING: No content rendered! Output is completely white.");
        println!("This indicates a rendering bug - content should be visible.");
    } else {
        let percentage = (non_white_count as f64 / (pixels.len() as f64 / 4.0)) * 100.0;
        println!(
            "SUCCESS: Rendered {} non-white pixels ({:.2}%)",
            non_white_count, percentage
        );

        if percentage < 0.1 {
            println!(
                "WARNING: Very little content rendered - may indicate partial rendering failure"
            );
        } else if percentage > 90.0 {
            println!("NOTE: Most of the page is filled - this is expected for text-heavy PDFs");
        }
    }
    println!("=====================\n");
}

#[test]
#[cfg(feature = "rendering")]
fn test_debug_device_drawing() {
    // Test basic device drawing without PDF parsing
    println!("\nDEBUG: Testing basic device drawing...");

    let width = 400u32;
    let height = 400u32;

    let mut pixmap = Pixmap::new(width, height).expect("Failed to create pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Draw a simple red rectangle at (100, 100) with size 200x100
    println!("DEBUG: Drawing red rectangle at (100, 100) size 200x100");
    device.begin_path();
    device.rect(100.0, 100.0, 200.0, 100.0);

    use pdf_x_core::rendering::{FillRule, Paint, PathDrawMode, StrokeProps};
    let paint = Paint::Solid(pdf_x_core::rendering::Color::rgb(255, 0, 0));

    device
        .draw_path(
            PathDrawMode::Fill(FillRule::NonZero),
            &paint,
            &StrokeProps::default(),
        )
        .expect("Failed to draw path");

    // Check if we have non-white pixels
    let pixels = pixmap.data();
    let non_white_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
        .count();

    println!(
        "DEBUG: Non-white pixels: {} ({:.2}%)",
        non_white_count,
        (non_white_count as f64 / (pixels.len() as f64 / 4.0)) * 100.0
    );

    // Find bounds
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let r = pixels[idx];
            if r < 250 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if non_white_count > 0 {
        println!(
            "DEBUG: Content bounds: x=[{}, {}], y=[{}, {}]",
            min_x, max_x, min_y, max_y
        );
        println!("DEBUG: Expected bounds: x=[100, 300], y=[100, 200]");
    }

    let output_path = "/tmp/debug_device_drawing.png";
    pixmap.save_png(output_path).expect("Failed to save PNG");
    println!("DEBUG: Saved to {}", output_path);

    assert!(
        non_white_count > 0,
        "Device drawing test failed - no content rendered"
    );
}
