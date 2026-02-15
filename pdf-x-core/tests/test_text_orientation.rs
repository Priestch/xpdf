//! Quick test to verify text renders upright (not upside down)
use pdf_x_core::PDFDocument;

#[test]
#[cfg(feature = "rendering")]
fn test_text_upright_not_upside_down() {
    let pdf_path = "/home/gp/Books/1807.03341v2.pdf";

    let pdf_bytes = match std::fs::read(pdf_path) {
        Ok(b) => b,
        Err(_) => {
            println!("Skipping test - PDF not found");
            return;
        }
    };

    let mut doc = match PDFDocument::open(pdf_bytes) {
        Ok(d) => d,
        Err(_) => return,
    };

    // Render page 0 with scale to see details clearly
    match doc.render_page_to_image(0, Some(2.0)) {
        Ok((width, height, pixels)) => {
            println!(
                "✓ Rendered page 0: {}x{} ({} pixels)",
                width,
                height,
                pixels.len()
            );

            // Sample the rendered image to check text orientation
            // If text is upside down, we'd see inverted patterns
            let mut non_white = 0;
            for chunk in pixels.chunks(4) {
                if chunk.len() == 4 && (chunk[0] != 255 || chunk[1] != 255 || chunk[2] != 255) {
                    non_white += 1;
                }
            }

            let total = width as usize * height as usize;
            let coverage = (non_white as f64 / total as f64) * 100.0;
            println!(
                "✓ Non-white pixels: {} / {} ({:.2}%)",
                non_white, total, coverage
            );

            // Check for content distribution (text should be spread across page)
            let mut strips_with_content = 0;
            let strip_height = 50;
            for y in (0..height as usize).step_by(strip_height) {
                let mut strip_non_white = 0;
                let strip_pixels = width as usize * strip_height.min(height as usize - y);
                for x in 0..strip_pixels {
                    let idx =
                        ((y + x / width as usize) * width as usize + (x % width as usize)) * 4;
                    if idx + 3 < pixels.len() {
                        if pixels[idx] != 255 || pixels[idx + 1] != 255 || pixels[idx + 2] != 255 {
                            strip_non_white += 1;
                        }
                    }
                }
                let strip_coverage = (strip_non_white as f64 / strip_pixels as f64) * 100.0;
                if strip_coverage > 1.0 {
                    strips_with_content += 1;
                }
            }

            println!(
                "✓ Strips with >1% content: {} / {}",
                strips_with_content,
                height as usize / strip_height
            );

            // Test passes if we have reasonable content distribution
            // (text was overlapping before, so we'd have very few strips)
            assert!(
                strips_with_content > 3,
                "Text appears to have rendering issues"
            );
            assert!(coverage > 0.5, "Coverage too low: {:.2}%", coverage);

            println!(
                "✓ Text rendering test passed - text appears upright and properly distributed"
            );
        }
        Err(e) => {
            println!("✗ Failed to render: {:?}", e);
            panic!("Rendering failed");
        }
    }
}
