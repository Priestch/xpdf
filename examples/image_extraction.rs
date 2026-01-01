//! Image Extraction Example
//!
//! This example demonstrates comprehensive image extraction capabilities:
//! - Extracting image metadata without full decoding
//! - Complete image decoding with specialized decoders
//! - Format detection and decoder routing
//! - Handling different image types in PDFs
//!
//! Run with: cargo run --example image_extraction <pdf_file>

use pdf_x::core::PDFDocument;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: cargo run --example image_extraction <pdf_file>");
        eprintln!("\nThis example extracts images from PDF files.");
        eprintln!("\nFeature flags:");
        eprintln!("  --features jpeg-decoding        Enable JPEG support");
        eprintln!("  --features png-decoding         Enable PNG support");
        eprintln!("  --features advanced-image-formats  Enable JPEG2000 and JBIG2");
        return Ok(());
    }

    let pdf_path = &args[1];
    println!("🖼️  Image Extraction Example");
    println!("📂 File: {}", pdf_path);
    println!();

    // Open PDF document
    let pdf_data = std::fs::read(pdf_path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    let page_count = doc.page_count()?;
    println!("📊 PDF has {} pages", page_count);
    println!();

    // Analyze each page for images
    let mut total_images = 0;
    let mut total_decoded = 0;
    let mut total_data_size = 0;

    for page_index in 0..page_count.min(5) { // Limit to first 5 pages for demo
        println!("📄 Page {}/{}:", page_index + 1, page_count);
        println!();

        let page = doc.get_page(page_index as usize)?;

        // Extract image metadata (fast, no full decoding)
        println!("  🔍 Scanning for images...");
        let images = page.get_image_metadata(&mut doc.xref_mut())?;

        if images.is_empty() {
            println!("  ✅ No images found on this page");
            println!();
            continue;
        }

        total_images += images.len();
        println!("  ✅ Found {} images", images.len());
        println!();

        for (i, image) in images.iter().enumerate() {
            println!("  Image {}:", i + 1);
            println!("    Name: {}", image.name);
            println!("    Format: {:?}", image.format);
            println!("    Size: {}x{} pixels", image.width, image.height);
            println!("    Bits per component: {}", image.bits_per_component);
            println!("    Color space: {}", image.color_space);
            println!("    Has alpha: {}", image.has_alpha);
            if let Some(size) = image.data_length {
                total_data_size += size;
                println!("    Data size: {} bytes ({:.1} KB)", size, size as f64 / 1024.0);
            }
            println!();
        }

        // Extract complete images with pixel data
        println!("  🎨 Decoding images...");
        let decoded_images = page.extract_images(&mut doc.xref_mut())?;

        if decoded_images.is_empty() {
            println!("  ⚠️  No images decoded successfully");
            println!("     (This may be due to unsupported formats or missing feature flags)");
        } else {
            total_decoded += decoded_images.len();
            println!("  ✅ Decoded {} images", decoded_images.len());
            println!();

            for (i, image) in decoded_images.iter().enumerate() {
                println!("  Decoded Image {}:", i + 1);
                println!("    Dimensions: {}x{} pixels", image.width, image.height);
                println!("    Channels: {}", image.channels);
                println!("    Color space: {:?}", image.color_space);
                println!("    Pixel data: {} bytes ({:.1} MB)",
                         image.data.len(),
                         image.data.len() as f64 / (1024.0 * 1024.0));

                // Example: Calculate total pixels
                let total_pixels = (image.width as usize) * (image.height as usize);
                println!("    Total pixels: {}", total_pixels);
                println!();

                // You can save the image here
                // Example: fs::write(format!("page{}_image{}.raw", page_index + 1, i + 1), &image.data)?;
            }
        }

        println!();
    }

    // Summary
    println!("═══════════════════════════════════════");
    println!("📋 Summary");
    println!("═══════════════════════════════════════");
    println!("Total images found: {}", total_images);
    println!("Total images decoded: {}", total_decoded);
    println!("Total image data: {:.1} KB", total_data_size as f64 / 1024.0);

    if total_images > 0 {
        let decode_rate = (total_decoded as f64 / total_images as f64) * 100.0;
        println!("Decode success rate: {:.1}%", decode_rate);
    }

    println!();
    println!("💡 Tips:");
    println!("  - Use metadata extraction (get_image_metadata) for fast scanning");
    println!("  - Use full extraction (extract_images) when you need pixel data");
    println!("  - Enable feature flags for more format support:");
    println!("    --features jpeg-decoding,png-decoding,advanced-image-formats");
    println!();

    #[cfg(feature = "jpeg-decoding")]
    println!("✅ JPEG decoding enabled");

    #[cfg(not(feature = "jpeg-decoding"))]
    println!("⚠️  JPEG decoding disabled (enable with --features jpeg-decoding)");

    #[cfg(feature = "png-decoding")]
    println!("✅ PNG decoding enabled");

    #[cfg(not(feature = "png-decoding"))]
    println!("⚠️  PNG decoding disabled (enable with --features png-decoding)");

    #[cfg(feature = "advanced-image-formats")]
    println!("✅ Advanced formats (JPEG2000, JBIG2) enabled");

    #[cfg(not(feature = "advanced-image-formats"))]
    println!("⚠️  Advanced formats disabled (enable with --features advanced-image-formats)");

    Ok(())
}


