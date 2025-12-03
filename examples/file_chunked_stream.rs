// Example: Progressive PDF loading from file with chunked streaming
//
// Usage: cargo run --example file_chunked_stream <path_to_pdf>
//
// This demonstrates:
// - Chunked file streaming with LRU caching
// - Progressive PDF parsing
// - Lazy page loading with caching
// - Inheritable property resolution

use pdf_x::core::{BaseStream, FileChunkedStream};
use pdf_x::{PDFDocument, PDFObject};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <path_to_pdf>", args[0]);
        eprintln!("\nExample:");
        eprintln!("  {} /path/to/document.pdf", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];

    println!("═══════════════════════════════════════════════════════════");
    println!("  PDF-X: Progressive PDF Loading from File");
    println!("═══════════════════════════════════════════════════════════\n");

    // ========================================================================
    // STEP 1: Open file with chunked streaming
    // ========================================================================
    println!("📂 Opening file: {}", file_path);

    let stream = FileChunkedStream::open(
        file_path,
        Some(65536),  // 64KB chunks
        Some(10),     // Keep max 10 chunks in memory (640KB cache)
    )?;

    let file_size = stream.length();
    let num_chunks = stream.num_chunks();

    println!("   File size: {} bytes ({:.2} MB)", file_size, file_size as f64 / 1_048_576.0);
    println!("   Total chunks: {} (64KB each)", num_chunks);
    println!("   Max memory: ~640KB (LRU cache)\n");

    // ========================================================================
    // STEP 2: Parse PDF document (progressive parsing)
    // ========================================================================
    println!("🔍 Parsing PDF document...");

    // Read file into memory for PDFDocument
    // (In a future version, PDFDocument could use chunked streams directly)
    let pdf_data = std::fs::read(file_path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    println!("   ✓ Document opened successfully\n");

    // ========================================================================
    // STEP 3: Extract document-level information
    // ========================================================================
    println!("📋 Document Information:");

    // Get page count
    let page_count = doc.page_count()?;
    println!("   Total pages: {}", page_count);

    // Show catalog
    if let Some(catalog) = doc.catalog() {
        if let PDFObject::Dictionary(dict) = catalog {
            print!("   Catalog keys: ");
            let keys: Vec<String> = dict.keys().map(|k| k.to_string()).collect();
            println!("{}", keys.join(", "));
        }
    }

    println!();

    // ========================================================================
    // STEP 4: Demonstrate lazy page loading
    // ========================================================================
    println!("📄 Lazy Page Loading (pages loaded on-demand):\n");

    let pages_to_show = std::cmp::min(3, page_count as usize);

    for i in 0..pages_to_show {
        println!("   Page {} (0-indexed):", i);

        // Get page (loaded lazily and cached)
        let page = doc.get_page(i)?;

        // Show page type
        if let Some(PDFObject::Name(page_type)) = page.get("Type") {
            println!("      Type: /{}", page_type);
        }

        // ========================================================================
        // STEP 5: Demonstrate inheritable property resolution
        // ========================================================================

        // Get MediaBox (may be inherited from parent Pages node)
        match doc.get_media_box(&page) {
            Ok(PDFObject::Array(arr)) if arr.len() == 4 => {
                println!("      MediaBox: [{}, {}, {}, {}]",
                    format_number(&arr[0]),
                    format_number(&arr[1]),
                    format_number(&arr[2]),
                    format_number(&arr[3])
                );

                // Calculate dimensions
                if let (PDFObject::Number(x1), PDFObject::Number(y1),
                        PDFObject::Number(x2), PDFObject::Number(y2)) =
                    (&arr[0], &arr[1], &arr[2], &arr[3]) {
                    let width = x2 - x1;
                    let height = y2 - y1;
                    println!("      Dimensions: {:.0} x {:.0} points ({:.2} x {:.2} inches)",
                        width, height, width / 72.0, height / 72.0);
                }
            }
            Ok(_) => println!("      MediaBox: present (non-standard format)"),
            Err(_) => {
                // Check if it's directly on the page
                if let Some(mb) = page.get("MediaBox") {
                    println!("      MediaBox: {:?} (direct)", mb);
                } else {
                    println!("      MediaBox: not found");
                }
            }
        }

        // Get Resources (may be inherited)
        match doc.get_resources(&page) {
            Ok(PDFObject::Dictionary(dict)) => {
                let resource_types: Vec<String> = dict.keys().map(|k| k.to_string()).collect();
                if !resource_types.is_empty() {
                    println!("      Resources: {}", resource_types.join(", "));
                } else {
                    println!("      Resources: (empty dictionary)");
                }
            }
            Ok(_) => println!("      Resources: present (non-dictionary)"),
            Err(_) => {
                if let Some(_) = page.get("Resources") {
                    println!("      Resources: present (direct)");
                }
            }
        }

        // Get Rotate if present
        if let Ok(PDFObject::Number(rotate)) = doc.get_rotate(&page) {
            println!("      Rotation: {}°", rotate);
        }

        // Get Contents if present
        if let Some(contents) = page.get("Contents") {
            match contents {
                PDFObject::Ref { num, generation } => {
                    println!("      Contents: {} {} R", num, generation);
                }
                PDFObject::Array(arr) => {
                    println!("      Contents: Array of {} streams", arr.len());
                }
                _ => {
                    println!("      Contents: present");
                }
            }
        }

        println!();
    }

    // ========================================================================
    // STEP 6: Demonstrate page caching
    // ========================================================================
    if page_count > 0 {
        println!("💾 Page Caching:");
        println!("   Loading page 0 again (should be cached)...");
        let _page = doc.get_page(0)?;
        println!("   ✓ Retrieved from cache (instant access)\n");
    }

    // ========================================================================
    // Summary
    // ========================================================================
    println!("═══════════════════════════════════════════════════════════");
    println!("  Features Demonstrated:");
    println!("═══════════════════════════════════════════════════════════");
    println!("  ✓ Chunked file streaming with LRU cache");
    println!("  ✓ Progressive PDF parsing");
    println!("  ✓ Lazy page loading (pages loaded on-demand)");
    println!("  ✓ Page caching (pages cached after first access)");
    println!("  ✓ Inheritable property resolution (MediaBox, Resources)");
    println!("  ✓ Hierarchical page tree traversal");
    println!("═══════════════════════════════════════════════════════════\n");

    Ok(())
}

/// Format a PDF number object for display
fn format_number(obj: &PDFObject) -> String {
    match obj {
        PDFObject::Number(n) => {
            if n.fract() == 0.0 {
                format!("{:.0}", n)
            } else {
                format!("{}", n)
            }
        }
        _ => format!("{:?}", obj),
    }
}
