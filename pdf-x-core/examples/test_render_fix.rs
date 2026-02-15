use pdf_x_core::PDFDocument;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pdf_path = "/home/gp/Books/Automate the Boring Stuff with Python, 3rd Edition.pdf";

    println!("Loading PDF: {}", pdf_path);
    let mut doc = PDFDocument::open_file(pdf_path, None, None)?;
    println!("PDF loaded successfully");
    println!("Number of pages: {}", doc.page_count()?);

    println!("Rendering page 1...");
    let result = doc.render_page_to_image(0, Some(1.0));
    match result {
        Ok((width, height, pixels)) => {
            println!("Rendered: {}x{} ({} bytes)", width, height, pixels.len());
            // Save to file
            std::fs::write("/tmp/test_render_example.png", pixels)?;
            println!("Saved to /tmp/test_render_example.png");
        }
        Err(e) => {
            println!("Rendering error: {:?}", e);
        }
    }

    Ok(())
}
