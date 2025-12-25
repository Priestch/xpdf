use pdf_x::PDFDocument;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Progressive Loading Demo");
        eprintln!("Usage: {} <pdf-file>", args[0]);
        eprintln!("\nThis example demonstrates PDF-X's progressive/chunked loading capabilities.");
        eprintln!("Instead of loading the entire PDF into memory, it loads data in 64KB chunks.");
        std::process::exit(1);
    }

    let pdf_path = &args[1];

    println!("═══════════════════════════════════════════════════════");
    println!("  Progressive Loading Demo - PDF-X");
    println!("═══════════════════════════════════════════════════════\n");

    println!("Opening: {}\n", pdf_path);
    println!("⏳ Loading PDF with 64KB chunks...");

    // Open the PDF using progressive loading
    let mut doc = match PDFDocument::open_file(pdf_path, None, None) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("\n❌ Error: {:?}", e);
            std::process::exit(1);
        }
    };

    println!("✅ PDF loaded successfully!\n");

    // Show basic information
    println!("═══════════════ DOCUMENT INFO ═══════════════");

    if let Ok(page_count) = doc.page_count() {
        println!("📄 Pages: {}", page_count);
    }

    println!("📊 XRef entries: {}", doc.xref().len());

    if let Some(_catalog) = doc.catalog() {
        println!("✓  Catalog loaded");

        // Check for linearization
        if doc.is_linearized() {
            println!("⚡ Linearized: Yes (optimized for fast web viewing)");
        } else {
            println!("📋 Linearized: No");
        }
    }

    println!("\n═══════════════ MEMORY EFFICIENCY ═══════════════");
    println!("✨ Benefits of Progressive Loading:");
    println!("  • Only loads needed chunks (64KB each)");
    println!("  • LRU cache keeps recently-used chunks in memory");
    println!("  • Old chunks automatically evicted to save memory");
    println!("  • Perfect for large PDFs (100MB+)");
    println!("  • Enables fast first-page display for linearized PDFs");

    println!("\n💡 This is the same approach used by PDF.js!");
    println!("═══════════════════════════════════════════════════════");
}
