//! Create a test PDF with visible colored shapes
//!
//! This generates a simple PDF with colored rectangles and lines
//! to verify the rendering pipeline works correctly.

use pdf_x_core::rendering::Device;
use std::fs::File;
use std::io::Write;

/// Create a minimal PDF with colored vector graphics
fn create_colored_shapes_pdf() -> Vec<u8> {
    // Simplified PDF structure with exact offsets
    // Each object ends with "endobj" followed by newline
    let pdf = "%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R/Resources<<>>>>endobj
4 0 obj<</Length 223>>stream
100 100 200 100 re
1 0 0 rg
f
100 250 200 100 re
0 1 0 rg
f
100 400 200 100 re
0 0 1 rg
f
100 550 200 100 re
1 1 0 rg
f
5 w
0 0 0 RG
400 100 m
500 650 l
S
endstream
endobj
xref
0 5
0000000000 65535 f
0000000009 00000 n
0000000056 00000 n
0000000111 00000 n
0000000204 00000 n
trailer
<</Size 5/Root 1 0 R>>
startxref
494
%%EOF";

    pdf.as_bytes().to_vec()
}

#[test]
#[cfg(feature = "rendering")]
fn test_create_and_render_colored_pdf() {
    let pdf_bytes = create_colored_shapes_pdf();

    // Save the PDF to a file for inspection
    let pdf_path = "/tmp/test_colored_shapes.pdf";
    if let Ok(mut file) = File::create(pdf_path) {
        file.write_all(&pdf_bytes).expect("Failed to write PDF");
        println!("DEBUG: Saved test PDF to {}", pdf_path);
    }

    println!("DEBUG: Parsing test PDF...");
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
        Err(e) => panic!("Failed to get page count: {:?}", e),
    };
    println!("DEBUG: Page count: {}", page_count);

    let page = match doc.get_page(0) {
        Ok(p) => p,
        Err(e) => panic!("Failed to get page: {:?}", e),
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
        _ => (0.0, 0.0, 612.0, 792.0),
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
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("Failed to create pixmap");

    // Fill with white background
    pixmap.fill(tiny_skia::Color::WHITE);

    // Create rendering device
    let mut device = pdf_x_core::rendering::SkiaDevice::new(pixmap.as_mut());

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
        }
    }

    // Check if we have non-white pixels
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

    // Count specific colors
    let red_count = pixels
        .chunks(4)
        .filter(|p| p[0] > 200 && p[1] < 50 && p[2] < 50)
        .count();
    let green_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 50 && p[1] > 200 && p[2] < 50)
        .count();
    let blue_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 50 && p[1] < 50 && p[2] > 200)
        .count();
    let yellow_count = pixels
        .chunks(4)
        .filter(|p| p[0] > 200 && p[1] > 200 && p[2] < 50)
        .count();
    let black_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 50 && p[1] < 50 && p[2] < 50)
        .count();

    println!("DEBUG: Color breakdown:");
    println!("  Red pixels: {}", red_count);
    println!("  Green pixels: {}", green_count);
    println!("  Blue pixels: {}", blue_count);
    println!("  Yellow pixels: {}", yellow_count);
    println!("  Black pixels: {}", black_count);

    // Save the rendered image
    let output_path = "/tmp/test_colored_shapes_rendered.png";
    match pixmap.save_png(output_path) {
        Ok(_) => println!("DEBUG: Saved rendered image to {}", output_path),
        Err(e) => eprintln!("ERROR: Failed to save PNG: {:?}", e),
    }

    // Print summary
    println!("\n=== RENDER SUMMARY ===");
    if non_white_count == 0 {
        panic!("ERROR: No content rendered! Output is completely white.");
    } else {
        println!(
            "SUCCESS: Rendered {} non-white pixels ({:.2}%)",
            non_white_count,
            (non_white_count as f64 / (pixels.len() as f64 / 4.0)) * 100.0
        );
    }

    // Verify each color is present
    println!("\nVerifying colors:");
    println!(
        "  Red rectangle (100, 100) 200x100: {} pixels - {}",
        red_count,
        if red_count > 1000 { "✓" } else { "✗ FAIL" }
    );
    println!(
        "  Green rectangle (100, 250) 200x100: {} pixels - {}",
        green_count,
        if green_count > 1000 {
            "✓"
        } else {
            "✗ FAIL"
        }
    );
    println!(
        "  Blue rectangle (100, 400) 200x100: {} pixels - {}",
        blue_count,
        if blue_count > 1000 { "✓" } else { "✗ FAIL" }
    );
    println!(
        "  Yellow rectangle (100, 550) 200x100: {} pixels - {}",
        yellow_count,
        if yellow_count > 1000 {
            "✓"
        } else {
            "✗ FAIL"
        }
    );
    println!(
        "  Black line (400,100)-(500,650): {} pixels - {}",
        black_count,
        if black_count > 100 { "✓" } else { "✗ FAIL" }
    );

    assert!(red_count > 1000, "Expected red rectangle to be visible");
    assert!(green_count > 1000, "Expected green rectangle to be visible");
    assert!(blue_count > 1000, "Expected blue rectangle to be visible");
    assert!(
        yellow_count > 1000,
        "Expected yellow rectangle to be visible"
    );
    assert!(black_count > 100, "Expected black line to be visible");

    println!("\n✓ All colored shapes rendered correctly!");
    println!("✓ Full rendering pipeline (PDF parse -> content stream -> device) works!");
}
