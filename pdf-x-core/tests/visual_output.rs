//! Save rendered pages as PNG for visual inspection
//! This creates output files that can be manually viewed to verify text orientation
use pdf_x_core::PDFDocument;
use std::fs::File;
use std::io::BufWriter;

// Simple PNG writer (based on PNG spec)
fn write_simple_png(path: &str, width: u32, height: u32, data: &[u8]) -> std::io::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);

    // PNG signature
    writer.write_all(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])?;

    // IHDR chunk
    let ihdr = &mut Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // color type (RGB)
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace

    write_chunk(&mut writer, b"IHDR", ihdr)?;

    // IDAT chunk (simplified - just raw RGB data with filter byte 0 per row)
    let idat = &mut Vec::new();
    for y in 0..height {
        idat.push(0); // filter type (none)
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            if idx + 2 < data.len() {
                idat.push(data[idx]); // R
                idat.push(data[idx + 1]); // G
                idat.push(data[idx + 2]); // B
            }
        }
    }

    // Compress IDAT
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(idat)?;
    let compressed = encoder.finish()?;

    write_chunk(&mut writer, b"IDAT", &compressed)?;

    // IEND chunk
    write_chunk(&mut writer, b"IEND", &[])?;

    Ok(())
}

fn write_chunk(writer: &mut BufWriter<File>, name: &[u8; 4], data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let len = (data.len() as u32).to_be_bytes();
    writer.write_all(&len)?;

    writer.write_all(name)?;

    let mut crc = crc32fast::hash(data);
    let name_crc = crc32fast::hash(name);
    crc = crc32fast::hash_combine(name_crc, crc);

    writer.write_all(data)?;
    writer.write_all(&crc.to_be_bytes())?;

    Ok(())
}

#[test]
#[cfg(feature = "rendering")]
fn save_visual_output() {
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
        match doc.render_page_to_image(page_num, Some(1.5)) {
            Ok((width, height, pixels)) => {
                let output_path = format!("/tmp/pdf_page_{}.png", page_num);
                match write_simple_png(&output_path, width, height, &pixels) {
                    Ok(_) => println!("✓ Saved visual output: {}", output_path),
                    Err(e) => println!("✗ Failed to save {}: {:?}", output_path, e),
                }
            }
            Err(e) => {
                println!("✗ Failed to render page {}: {:?}", page_num, e);
            }
        }
    }

    println!("\nVisual outputs saved to /tmp/pdf_page_*.png");
    println!("You can view these images to verify:");
    println!("  - Text is upright (not upside down)");
    println!("  - Text is not overlapping");
    println!("  - Text is properly positioned");
}
