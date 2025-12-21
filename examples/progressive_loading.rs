//! Progressive Loading Example
//!
//! This example demonstrates PDF-X's progressive loading capabilities:
//! - Loading large PDFs without reading entire files
//! - Working with chunked data streams
//! - Exception-driven data loading pattern
//! - Memory-efficient PDF processing
//!
//! Run with: cargo run --example progressive_loading <pdf_file>

use pdf_x::core::{PDFDocument, FileChunkedStream, HttpChunkedStream};
use std::env;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: cargo run --example progressive_loading <pdf_file>");
        eprintln!("\nThis example demonstrates progressive loading capabilities.");
        return Ok(());
    }

    let pdf_path = &args[1];
    println!("📄 Progressive loading example: {}", pdf_path);

    // Method 1: Standard loading (for comparison)
    println!("\n🔄 Method 1: Standard Loading");
    let start = Instant::now();
    let pdf_data = std::fs::read(pdf_path)?;
    let parse_time = Instant::now() - start;
    println!("  File read time: {:?}", parse_time);
    println!("  File size: {} bytes", pdf_data.len());

    let start = Instant::now();
    let _doc = PDFDocument::open(pdf_data)?;
    let standard_parse_time = Instant::now() - start;
    println!("  Parse time: {:?}", standard_parse_time);

    // Method 2: Progressive loading with FileChunkedStream
    println!("\n⚡ Method 2: Progressive Loading");

    let start = Instant::now();
    let mut stream = FileChunkedStream::open(pdf_path)?;
    let open_time = Instant::now() - start;
    println!("  Stream open time: {:?}", open_time);

    // Note: PDFDocument::open() internally handles progressive loading
    // but we could also use the stream directly for more control
    let start = Instant::now();
    let mut doc = PDFDocument::open(Box::new(stream))?;
    let progressive_parse_time = Instant::now() - start;
    println!("  Progressive parse time: {:?}", progressive_parse_time);

    // Demonstrate on-demand page loading
    println!("\n📖 On-Demand Page Loading:");
    let page_count = doc.page_count()?;
    println!("  Total pages: {}", page_count);

    // Load first page (triggers progressive loading if needed)
    let start = Instant::now();
    let _page1 = doc.get_page(0)?;
    let page1_time = start.elapsed();
    println!("  Page 1 load time: {:?}", page1_time);

    // Load middle page (demonstrates caching)
    if page_count > 1 {
        let start = Instant::now();
        let _page_mid = doc.get_page(page_count / 2)?;
        let page_mid_time = start.elapsed();
        println!("  Page {} load time: {:?}", page_count / 2 + 1, page_mid_time);
    }

    // Load last page
    if page_count > 2 {
        let start = Instant::now();
        let _page_last = doc.get_page(page_count - 1)?;
        let page_last_time = start.elapsed();
        println!("  Page {} load time: {:?}", page_count, page_last_time);
    }

    // Extract text from pages as needed
    println!("\n📝 Text Extraction (on-demand):");
    for i in 0..std::cmp::min(3, page_count) {
        let start = Instant::now();
        let page = doc.get_page(i)?;
        let text_items = page.extract_text(&mut doc.xref_mut())?;
        let extract_time = start.elapsed();

        println!("  Page {}: {} text items in {:?}", i + 1, text_items.len(), extract_time);
    }

    // Compare memory usage
    println!("\n💾 Memory Efficiency:");
    println!("  Standard: Full file loaded into memory ({} bytes)", pdf_data.len());
    println!("  Progressive: Only chunks loaded as needed");
    println!("  Pages: Loaded on-demand when accessed");
    println!("  Content streams: Processed incrementally");

    // Demonstrate HTTP loading capability (just show the API)
    println!("\n🌐 HTTP Progressive Loading:");
    println!("  For remote PDFs, use HttpChunkedStream:");
    println!("  ```rust");
    println!("  let mut stream = HttpChunkedStream::new(\"https://example.com/doc.pdf\")?;");
    println!("  let mut doc = PDFDocument::open(Box::new(stream))?;");
    println!("  ```");

    // Performance comparison
    println!("\n📊 Performance Comparison:");
    let improvement = if standard_parse_time > progressive_parse_time {
        let percent = ((standard_parse_time - progressive_parse_time).as_nanos() as f64
                      / standard_parse_time.as_nanos() as f64) * 100.0;
        format!("Progressive is {:.1}% faster", percent)
    } else {
        "Times are similar for this small PDF".to_string()
    };
    println!("  {}", improvement);

    Ok(())
}

/// Example of memory-efficient PDF processing
#[allow(dead_code)]
fn memory_efficient_processing() -> Result<(), Box<dyn std::error::Error>> {
    println!("💡 Memory-Efficient Processing Tips:");
    println!("  1. Use FileChunkedStream for large files");
    println!("  2. Load pages only when needed");
    println!("  3. Process content streams incrementally");
    println!("  4. Drop PDFDocument when done to free memory");
    println!("  5. Use chunk size appropriate to your use case");
    println!("");
    println!("  Example:");
    println!("  ```rust");
    println!("  {");
    println!("      let mut doc = PDFDocument::open(Box::new(FileChunkedStream::open(path)?))?;");
    println!("");
    println!("      // Only process pages you need");
    println!("      for page_index in pages_of_interest {{");
    println!("          let page = doc.get_page(page_index)?;");
    println!("          let text = page.extract_text(&mut doc.xref_mut())?;");
    println!("          // Process text...");
    println!("      }}");
    println!("  }}");
    println!("  ```");

    Ok(())
}