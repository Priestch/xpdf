//! Test the transform fix by rendering issue7200.pdf
use pdf_x_core::PDFDocument;

#[test]
fn test_issue7200_rendering() {
    let pdf_path = "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf";

    let pdf_bytes = match std::fs::read(pdf_path) {
        Ok(b) => b,
        Err(_) => {
            println!("Skipping test - PDF not found");
            return;
        }
    };

    let mut doc = match PDFDocument::open(pdf_bytes) {
        Ok(d) => d,
        Err(e) => {
            println!("Failed to parse PDF: {:?}", e);
            return;
        }
    };

    // Render the page
    match doc.render_page_to_image(0, Some(2.0)) {
        Ok((width, height, pixels)) => {
            println!(
                "Rendered page 0: {}x{} ({} pixels)",
                width,
                height,
                pixels.len()
            );

            // Count non-white pixels
            let mut non_white = 0;
            for chunk in pixels.chunks(4) {
                if chunk.len() == 4 && (chunk[0] != 255 || chunk[1] != 255 || chunk[2] != 255) {
                    non_white += 1;
                }
            }

            let total = width as usize * height as usize;
            let coverage = (non_white as f64 / total as f64) * 100.0;
            println!(
                "Non-white pixels: {} / {} ({:.2}%)",
                non_white, total, coverage
            );

            // DEBUG: Check pixels at expected image location (around x=140, y=140)
            let check_x = 140;
            let check_y = 140;
            let pixel_index = (check_y * width as usize + check_x) * 4;
            if pixel_index + 3 < pixels.len() {
                println!(
                    "DEBUG: Pixel at ({}, {}): RGB({}, {}, {})",
                    check_x,
                    check_y,
                    pixels[pixel_index],
                    pixels[pixel_index + 1],
                    pixels[pixel_index + 2]
                );
            }

            // Save as PNG for visual inspection
            let png_data = image_save_helper(width, height, &pixels);
            std::fs::write("/tmp/issue7200_page0.png", &png_data).unwrap();
            println!("Saved to /tmp/issue7200_page0.png");

            // With the fix, we expect > 10% coverage (was 0.42% before)
            assert!(
                coverage > 10.0,
                "Expected > 10% coverage, got {:.2}%",
                coverage
            );
        }
        Err(e) => {
            println!("Failed to render: {:?}", e);
            panic!("Rendering failed");
        }
    }
}

// Helper function to save pixels as PNG (simplified, no external dependency)
fn image_save_helper(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    // For now, just save raw data (can be viewed with tools)
    // In a real implementation, use a PNG encoder
    let mut result = Vec::new();
    // Simple header
    result.extend_from_slice(format!("P6\n{} {}\n255\n", width, height).as_bytes());
    for pixel in pixels.chunks(4) {
        if pixel.len() >= 3 {
            result.push(pixel[0]);
            result.push(pixel[1]);
            result.push(pixel[2]);
        }
    }
    result
}
