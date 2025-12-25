/// Example demonstrating text extraction from PDF pages.

use pdf_x::PDFDocument;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Text Extraction Demo");
        eprintln!("Usage: {} <pdf-file> [page-number]", args[0]);
        eprintln!("\nExtracts text from the specified page (default: page 1)");
        std::process::exit(1);
    }

    let pdf_path = &args[1];
    let page_num = if args.len() > 2 {
        args[2].parse::<usize>().unwrap_or(0)
    } else {
        0  // First page
    };

    println!("═══════════════════════════════════════════════════════");
    println!("  PDF Text Extraction Demo");
    println!("═══════════════════════════════════════════════════════\n");

    println!("📄 Opening: {}", pdf_path);
    println!("📖 Extracting text from page {}...\n", page_num + 1);

    // Open PDF with progressive loading
    let mut doc = match PDFDocument::open_file(pdf_path, None, None) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("\n❌ Error opening PDF: {:?}", e);
            std::process::exit(1);
        }
    };

    // Get the page
    let page = match doc.get_page(page_num) {
        Ok(page) => page,
        Err(e) => {
            eprintln!("\n❌ Error getting page {}: {:?}", page_num + 1, e);
            std::process::exit(1);
        }
    };

    // Extract text
    let text_items = match page.extract_text(doc.xref_mut()) {
        Ok(items) => items,
        Err(e) => {
            eprintln!("\n❌ Error extracting text: {:?}", e);
            std::process::exit(1);
        }
    };

    println!("═══════════════ EXTRACTED TEXT ═══════════════");
    println!("Found {} text items\n", text_items.len());

    if text_items.is_empty() {
        println!("ℹ️  No text found on this page");
        println!("   (Page may contain only images or be blank)");
    } else {
        for (i, item) in text_items.iter().enumerate() {
            println!("─── Text Item #{} ───", i + 1);
            println!("  Content: \"{}\"", item.text);
            
            if let Some(font) = &item.font_name {
                println!("  Font: {}", font);
            }
            
            if let Some(size) = item.font_size {
                println!("  Size: {:.2} pt", size);
            }
            
            if let Some((x, y)) = item.position {
                println!("  Position: ({:.2}, {:.2})", x, y);
            }
            
            println!();
        }

        // Show combined text
        println!("═══════════════ COMBINED TEXT ═══════════════");
        let combined: String = text_items.iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<&str>>()
            .join(" ");
        
        println!("{}", combined);
    }

    println!("\n═══════════════════════════════════════════════════════");
}
