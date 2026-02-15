//! Debug coordinate transform issues
#![cfg(feature = "rendering")]

use pdf_x_core::rendering::Device;
use pdf_x_core::rendering::skia_device::SkiaDevice;
use tiny_skia::Pixmap;

#[test]
#[cfg(feature = "rendering")]
fn test_debug_transforms() {
    println!("\n=== Debug Coordinate Transforms ===\n");

    // Test 1807.03341v2.pdf page 1 (showing low coverage)
    let pdf_path = "/home/gp/Books/1807.03341v2.pdf";

    let pdf_bytes = match std::fs::read(pdf_path) {
        Ok(b) => b,
        Err(_) => {
            println!("Skipping test - PDF not found");
            return;
        }
    };

    let mut doc = match pdf_x_core::PDFDocument::open(pdf_bytes) {
        Ok(d) => d,
        Err(e) => {
            println!("Failed to parse PDF: {:?}", e);
            return;
        }
    };

    let page = match doc.get_page(1) {
        Ok(p) => p,
        Err(_) => {
            println!("Failed to get page 1");
            return;
        }
    };

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

    println!("MediaBox: [{}, {}, {}, {}]", x0, y0, x1, y1);
    println!(
        "Page size: {}x{}",
        (x1 - x0).ceil() as u32,
        (y1 - y0).ceil() as u32
    );

    // Create pixmap
    let scale = 2.0;
    let width = ((x1 - x0).ceil() as f64 * scale).ceil() as u32;
    let height = ((y1 - y0).ceil() as f64 * scale).ceil() as u32;

    println!("Pixmap size: {}x{}", width, height);

    let mut pixmap = Pixmap::new(width, height).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Print the initial transform
    println!(
        "Initial transform: [{}, {}, {}, {}, {}, {}]",
        scale,
        0.0,
        0.0,
        -scale,
        -(x0 as f64) * scale,
        (y1 as f64) * scale
    );

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
        Err(e) => println!("Render error: {:?}", e),
    }

    // Analyze the result
    let pixels = pixmap.data();
    let width = pixmap.width();
    let height = pixmap.height();

    let mut non_white = 0usize;
    let mut black_pixels = 0usize;

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let r = pixels[idx];
            let g = pixels[idx + 1];
            let b = pixels[idx + 2];

            if r < 250 || g < 250 || b < 250 {
                non_white += 1;
            }
            if r < 50 && g < 50 && b < 50 {
                black_pixels += 1;
            }
        }
    }

    let total = (width * height) as usize;
    let percentage = (non_white as f64 / total as f64) * 100.0;
    let black_percentage = (black_pixels as f64 / total as f64) * 100.0;

    println!("\n=== Analysis ===");
    println!("Total pixels: {}", total);
    println!("Non-white pixels: {} ({:.2}%)", non_white, percentage);
    println!("Black pixels: {} ({:.2}%)", black_pixels, black_percentage);

    // Sample a grid of pixels to see what's there
    println!("\n=== Pixel Grid Sample ===");
    let step_x = width / 10;
    let step_y = height / 10;

    for iy in 0..10 {
        for ix in 0..10 {
            let x = ix * step_x;
            let y = iy * step_y;
            let idx = ((y * width + x) * 4) as usize;
            let r = pixels[idx];
            let g = pixels[idx + 1];
            let b = pixels[idx + 2];
            print!("({},{}):RGB({},{},{}) ", x, y, r, g, b);
        }
        println!();
    }

    // Save for inspection
    let output = "/tmp/debug_transform_page_1.png";
    pixmap.save_png(output).unwrap();
    println!("\nSaved to {}", output);
}
