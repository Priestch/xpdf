//! Basic PDF Processing Example
//!
//! This example demonstrates basic PDF operations including:
//! - Opening PDF files
//! - Getting page count
//! - Extracting text
//! - Navigating pages
//!
//! Run with: cargo run --example basic_usage

use pdf_x::core::PDFDocument;
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get PDF file path from command line or use default
    let args: Vec<String> = env::args().collect();
    let pdf_path = if args.len() > 1 {
        &args[1]
    } else {
        eprintln!("Usage: cargo run --example basic_usage <pdf_file>");
        eprintln!("No file provided, creating a simple PDF for demonstration...");
        return create_and_process_simple_pdf();
    };

    println!("📄 Processing PDF: {}", pdf_path);

    // Read PDF file
    let pdf_data = std::fs::read(pdf_path)?;

    // Open PDF document
    let mut doc = PDFDocument::open(pdf_data)?;

    // Display basic document information
    println!("📊 Document Information:");
    println!("  Page count: {}", doc.page_count()?);

    // Check if it's linearized (optimized for web view)
    if doc.is_linearized() {
        println!("  Linearized: Yes (optimized for fast first-page display)");
        if let Some(info) = doc.linearized_info() {
            println!("  File size: {} bytes", info.file_size);
            println!("  Total pages: {}", info.page_count);
        }
    } else {
        println!("  Linearized: No");
    }

    // Process all pages
    let total_text_items = process_all_pages(&mut doc)?;

    println!("\n✅ Processing completed successfully!");
    println!("   Total text items extracted: {}", total_text_items);

    Ok(())
}

fn process_all_pages(doc: &mut PDFDocument) -> Result<usize, Box<dyn std::error::Error>> {
    let page_count = doc.page_count()?;
    let mut total_items = 0;

    println!("\n📖 Processing {} pages:", page_count);

    for page_index in 0..page_count {
        let page = doc.get_page(page_index as usize)?;
        let text_items = page.extract_text(&mut doc.xref_mut())?;

        total_items += text_items.len();

        println!("  Page {}: {} text items", page_index + 1, text_items.len());

        // Display first few text items as preview
        for (i, item) in text_items.iter().take(3).enumerate() {
            let font_info = item.font_name.as_ref()
                .map(|f| format!(" ({})", f))
                .unwrap_or_default();
            let pos_info = item.position
                .map(|(x, y)| format!("@ ({:.1}, {:.1})", x, y))
                .unwrap_or_default();

            println!("    {}: \"{}\" {} {}", i + 1, item.text, font_info, pos_info);
        }

        if text_items.len() > 3 {
            println!("    ... and {} more items", text_items.len() - 3);
        }
    }

    Ok(total_items)
}

fn create_and_process_simple_pdf() -> Result<(), Box<dyn std::error::Error>> {
    println!("📝 Creating a simple PDF for demonstration...");

    // Create a minimal PDF using the working test format
    let pdf_data = b"%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R >>
endobj
xref
0 4
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000115 00000 n
trailer
<< /Size 4 /Root 1 0 R >>
startxref
162
%%EOF";

    let mut doc = PDFDocument::open(pdf_data.to_vec())?;

    println!("📊 Created PDF Document:");
    println!("  Page count: {}", doc.page_count()?);

    process_all_pages(&mut doc)?;

    println!("\n💡 Tip: Try this example with a real PDF file:");
    println!("   cargo run --example basic_usage /path/to/your/document.pdf");

    Ok(())
}