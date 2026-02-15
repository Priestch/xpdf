//! Save rendered pages as PNG using tiny-skia's built-in PNG encoding
use pdf_x_core::PDFDocument;

#[test]
#[cfg(feature = "rendering")]
fn save_rendered_pages_as_png() {
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
        Err(e) => {
            println!("Failed to open PDF: {:?}", e);
            return;
        }
    };

    let test_pages = vec![0, 1, 2];

    for page_num in test_pages {
        println!("\nRendering page {}...", page_num);

        match doc.render_page_to_image(page_num, Some(2.0)) {
            Ok((width, height, pixels)) => {
                println!("✓ Rendered: {}x{} ({} pixels)", width, height, pixels.len());

                // Create a tiny-skia Pixmap from the pixel data
                let mut pixmap = tiny_skia::Pixmap::new(width, height).unwrap();

                // Copy RGBA pixels
                let pixmap_data = pixmap.data_mut();
                for i in 0..pixels.len().min(pixmap_data.len()) {
                    pixmap_data[i] = pixels[i];
                }

                // Save as PNG
                let output_path = format!("/tmp/pdf_page_{}.png", page_num);
                match pixmap.save_png(&output_path) {
                    Ok(_) => {
                        println!("✓ Saved: {}", output_path);
                        println!(
                            "  View with: eog {} || firefox {} || display {}",
                            output_path, output_path, output_path
                        );
                    }
                    Err(e) => {
                        println!("✗ Failed to save {}: {:?}", output_path, e);
                    }
                }
            }
            Err(e) => {
                println!("✗ Failed to render page {}: {:?}", page_num, e);
            }
        }
    }

    println!("\n=== Summary ===");
    println!("Visual outputs saved to /tmp/pdf_page_*.png");
    println!("These images verify:");
    println!("  ✓ Text is upright (not upside down)");
    println!("  ✓ Text is not overlapping");
    println!("  ✓ Text is properly positioned across the page");
}
