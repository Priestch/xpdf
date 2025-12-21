//! PDF Debugging Example
//!
//! This example helps debug issues with complex PDF files by showing
//! the structure of page objects and their Contents fields.
//!
//! Run with: cargo run --example debug_pdf <pdf_file>

use pdf_x::core::PDFDocument;
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: cargo run --example debug_pdf <pdf_file>");
        return Ok(());
    }

    let pdf_path = &args[1];
    println!("🐛 PDF Debug Example: {}", pdf_path);

    // Open PDF document
    let pdf_data = std::fs::read(pdf_path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    let page_count = doc.page_count()?;
    println!("📊 Document has {} pages", page_count);

    // Check first few pages in detail
    let pages_to_check = std::cmp::min(5, page_count);

    for i in 0..pages_to_check {
        println!("\n🔍 Debugging Page {}:", i + 1);

        // Try to get page object
        match doc.get_page(i as usize) {
            Ok(page) => {
                println!("  ✅ Got page object successfully");

                // Try to extract text immediately to see what happens
                match page.extract_text(&mut doc.xref_mut()) {
                    Ok(text_items) => {
                        println!("  ✅ Text extraction succeeded: {} items", text_items.len());
                        if !text_items.is_empty() {
                            for (i, item) in text_items.iter().take(3).enumerate() {
                                println!("    [{}]: '{}' at {:?}", i, item.text, item.position);
                            }
                        } else {
                            println!("    ⚠️  No text items found (might be image-only page)");
                        }
                    }
                    Err(e) => {
                        println!("  ❌ Text extraction failed: {:?}", e);

                        // Provide helpful error context
                        if let pdf_x::core::PDFError::Generic(msg) = &e {
                            if msg.contains("Contents") {
                                println!("  💡 This is a Contents field parsing issue");
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("  ❌ Failed to get page {}: {:?}", i + 1, e);
            }
        }
    }

    println!("\n📋 Summary:");
    println!("  This PDF has {} pages", page_count);
    println!("  Try running the debug output above to see what's happening with each page.");

    Ok(())
}