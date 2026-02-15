//! PDF Rendering Example
//!
//! Renders a PDF page to a PNG image.
//!
//! Usage:
//!     cargo run --example render --features rendering -- input.pdf output.png [page_number]
//!
//! Example:
//!     cargo run --example render --features rendering -- test.pdf output.png 0

use pdf_x_core::rendering::Device; // Import Device trait for concat_matrix
use pdf_x_core::rendering::skia_device::SkiaDevice;
use pdf_x_core::{PDFDocument, Page};
use std::env;
use std::path::Path;
use tiny_skia::Pixmap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: {} <input.pdf> <output.png> [page_number]", args[0]);
        eprintln!("Example: {} test.pdf output.png 0", args[0]);
        eprintln!();
        eprintln!("Arguments:");
        eprintln!("  input.pdf    - Path to input PDF file");
        eprintln!("  output.png   - Path to output PNG file");
        eprintln!("  page_number  - Page to render (default: 0, first page)");
        std::process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];
    let page_number: usize = if args.len() > 3 {
        args[3].parse().unwrap_or(0)
    } else {
        0
    };

    // Check if input file exists
    if !Path::new(input_path).exists() {
        eprintln!("Error: Input file '{}' not found", input_path);
        std::process::exit(1);
    }

    println!("Loading PDF: {}", input_path);

    // Load PDF document
    let pdf_data = std::fs::read(input_path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    // Get page count
    let page_count = doc.page_count()?;
    println!("PDF has {} pages", page_count);

    if page_number >= page_count as usize {
        eprintln!(
            "Error: Page {} out of range (0-{})",
            page_number,
            page_count - 1
        );
        std::process::exit(1);
    }

    // Get the requested page
    let page = doc.get_page(page_number)?;

    // Get the page dimensions from the MediaBox
    let mediabox = page.media_box().ok_or("No MediaBox found")?;

    // MediaBox is typically an array [x1 y1 x2 y2]
    let coords = if let pdf_x_core::PDFObject::Array(arr) = mediabox {
        let mut coords = [0.0f64; 4];
        for (i, val) in arr.iter().take(4).enumerate() {
            if let pdf_x_core::PDFObject::Number(n) = &**val {
                coords[i] = *n;
            }
        }
        coords
    } else {
        return Err("Invalid MediaBox format".into());
    };

    let width = coords[2] - coords[0]; // x2 - x1
    let height = coords[3] - coords[1]; // y2 - y1

    println!(
        "Page {} dimensions: {:.1} x {:.1}",
        page_number, width, height
    );

    // Create pixmap for rendering (2x scale for better quality)
    let scale = 2.0;
    let pixmap_width = (width * scale).ceil() as u32;
    let pixmap_height = (height * scale).ceil() as u32;

    let mut pixmap = Pixmap::new(pixmap_width, pixmap_height).ok_or("Failed to create pixmap")?;

    // Fill with white background
    pixmap.fill(tiny_skia::Color::WHITE);

    // Create rendering device
    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Set up coordinate transform
    // PDF coordinates: (0,0) at bottom-left, y increases upward
    // Image coordinates: (0,0) at top-left, y increases downward
    // We need to flip Y and translate the origin
    device.concat_matrix(&[
        scale,     // sx: scale X
        0.0,       // kx: no skew
        0.0,       // ky: no skew
        -scale,    // sy: negative scale flips Y-axis
        0.0,       // tx: no X translation
        coords[3] * scale, // ty: move origin to page height
    ]);

    println!("Rendering page {}...", page_number);

    // Render the page
    match page.render(&mut doc.xref_mut(), &mut device) {
        Ok(_) => println!("Rendering complete!"),
        Err(e) => {
            eprintln!("Warning: Rendering encountered errors: {:?}", e);
            eprintln!(
                "The output may be incomplete. Note: Full font and image support is still in development."
            );
        }
    }

    // Save the rendered image
    println!("Saving to: {}", output_path);
    pixmap.save_png(output_path)?;

    println!("Done!");
    Ok(())
}
