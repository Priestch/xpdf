/// Comprehensive rendering debug trace
///
/// This test renders a PDF page with full logging to diagnose rendering issues.

use pdf_x_core::rendering::device::Device;
use pdf_x_core::rendering::skia_device::SkiaDevice;
use pdf_x_core::rendering::{Color, Paint};
use pdf_x_core::{PDFDocument, Page};
use std::fs::File;
use std::io::Write;
use tiny_skia::Pixmap;

#[test]
#[ignore] // Run with: cargo test --features rendering debug_rendering_trace -- --ignored --nocapture
fn debug_rendering_trace() {
    let log_file_path = "/tmp/pdf_render_debug.log";
    let mut log = File::create(log_file_path).expect("Failed to create log file");

    macro_rules! log {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            writeln!(log, "{}", msg).ok();
            println!("{}", msg);
        }};
    }

    log!("=== PDF Rendering Debug Trace ===");
    log!("Log file: {}", log_file_path);

    // Test with a real PDF
    let pdf_path = "/home/gp/Books/d2l-en.pdf";
    log!("Loading PDF: {}", pdf_path);

    let pdf_data = match std::fs::read(pdf_path) {
        Ok(data) => {
            log!("✓ PDF loaded: {} bytes", data.len());
            data
        }
        Err(e) => {
            log!("✗ Failed to load PDF: {}", e);
            // Try another PDF
            let fallback = "/home/gp/Books/1807.03341v2.pdf";
            log!("Trying fallback: {}", fallback);
            std::fs::read(fallback).expect("Failed to load fallback PDF")
        }
    };

    let mut doc = PDFDocument::open(pdf_data).expect("Failed to open PDF");
    log!("✓ PDF opened, {} pages", doc.page_count());

    let page_num = 0;
    log!("\n--- Rendering page {} ---", page_num);

    let page = doc.page(page_num).expect("Failed to get page");
    let page_obj = page.page_obj();

    // Get MediaBox
    let media_box = page_obj
        .get("MediaBox")
        .or_else(|| page_obj.get("CropBox"))
        .expect("No MediaBox");

    log!("MediaBox: {:?}", media_box);

    // Parse dimensions
    use pdf_x_core::core::parser::PDFObject;
    let (width, height) = match media_box {
        PDFObject::Array(arr) => {
            let x0 = if let PDFObject::Number(n) = &**arr.get(0).unwrap() { *n } else { 0.0 };
            let y0 = if let PDFObject::Number(n) = &**arr.get(1).unwrap() { *n } else { 0.0 };
            let x1 = if let PDFObject::Number(n) = &**arr.get(2).unwrap() { *n } else { 612.0 };
            let y1 = if let PDFObject::Number(n) = &**arr.get(3).unwrap() { *n } else { 792.0 };
            ((x1 - x0) as u32, (y1 - y0) as u32)
        }
        _ => (612, 792),
    };

    log!("Page dimensions: {}x{}", width, height);

    // Create pixmap
    let mut pixmap = Pixmap::new(width, height).expect("Failed to create pixmap");
    log!("✓ Created pixmap: {}x{}", pixmap.width(), pixmap.height());

    // Fill with white background
    pixmap.fill(tiny_skia::Color::WHITE);
    log!("✓ Filled with white background");

    // Create device
    let mut device = SkiaDevice::new(pixmap.as_mut());
    log!("✓ Created SkiaDevice");

    // Set up coordinate transform
    let scale = 1.5;
    let ctm = [
        scale,
        0.0,
        0.0,
        -scale,
        0.0,
        height as f64 * scale,
    ];
    device.set_matrix(&ctm);
    log!("✓ Set CTM: [{:.1}, {:.1}, {:.1}, {:.1}, {:.1}, {:.1}]",
         ctm[0], ctm[1], ctm[2], ctm[3], ctm[4], ctm[5]);

    // Render the page
    log!("\n--- Starting page rendering ---");
    let render_start = std::time::Instant::now();

    match page.render(&mut device) {
        Ok(_) => {
            let elapsed = render_start.elapsed();
            log!("✓ Page rendered in {:.2}ms", elapsed.as_secs_f64() * 1000.0);
        }
        Err(e) => {
            log!("✗ Rendering error: {:?}", e);
        }
    }

    // Analyze the rendered pixmap
    log!("\n--- Analyzing rendered output ---");
    let pixels = pixmap.data();
    let total_pixels = (pixmap.width() * pixmap.height()) as usize;

    let mut white_count = 0;
    let mut black_count = 0;
    let mut gray_count = 0;
    let mut color_count = 0;
    let mut unique_colors = std::collections::HashMap::new();

    for chunk in pixels.chunks_exact(4) {
        let r = chunk[0];
        let g = chunk[1];
        let b = chunk[2];
        let a = chunk[3];

        let color_key = format!("{},{},{},{}", r, g, b, a);
        *unique_colors.entry(color_key).or_insert(0) += 1;

        if r == 255 && g == 255 && b == 255 {
            white_count += 1;
        } else if r == 0 && g == 0 && b == 0 {
            black_count += 1;
        } else if r == g && g == b {
            gray_count += 1;
        } else {
            color_count += 1;
        }
    }

    let non_white = total_pixels - white_count;

    log!("Total pixels: {}", total_pixels);
    log!("White pixels: {} ({:.2}%)", white_count, white_count as f64 / total_pixels as f64 * 100.0);
    log!("Black pixels: {} ({:.2}%)", black_count, black_count as f64 / total_pixels as f64 * 100.0);
    log!("Gray pixels: {} ({:.2}%)", gray_count, gray_count as f64 / total_pixels as f64 * 100.0);
    log!("Color pixels: {} ({:.2}%)", color_count, color_count as f64 / total_pixels as f64 * 100.0);
    log!("Non-white pixels: {} ({:.2}%)", non_white, non_white as f64 / total_pixels as f64 * 100.0);
    log!("Unique colors: {}", unique_colors.len());

    // Show top 10 colors
    let mut color_vec: Vec<_> = unique_colors.iter().collect();
    color_vec.sort_by(|a, b| b.1.cmp(a.1));
    log!("\nTop 10 colors:");
    for (i, (color, count)) in color_vec.iter().take(10).enumerate() {
        log!("  {}: {} ({} pixels, {:.2}%)",
             i + 1, color, count, *count as f64 / total_pixels as f64 * 100.0);
    }

    // Save output
    let output_path = "/tmp/debug_render_output.png";
    pixmap.save_png(output_path).expect("Failed to save PNG");
    log!("\n✓ Saved output to: {}", output_path);

    log!("\n=== Summary ===");
    if non_white == 0 {
        log!("❌ ISSUE: Rendered page is completely white!");
        log!("   Possible causes:");
        log!("   1. Text color is white (check SetFillGray/SetFillRGB values)");
        log!("   2. Text is being clipped outside the visible area");
        log!("   3. Transform is placing content off-screen");
        log!("   4. Font glyphs are not being rendered");
        log!("\n   Check the debug logs above for clues.");
    } else {
        log!("✓ SUCCESS: Page rendered with content ({} non-white pixels)", non_white);
    }

    log!("\nLog file saved to: {}", log_file_path);
    log!("Image saved to: {}", output_path);

    // Assert that we have content
    assert!(non_white > 0, "Rendered page is completely white! Check logs at {}", log_file_path);
}
