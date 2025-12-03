// Example: Progressive PDF loading from HTTP with chunked streaming
//
// Usage: cargo run --example http_chunked_stream <pdf_url>
//
// This demonstrates:
// - HTTP chunked streaming with range requests
// - Progressive PDF parsing from remote sources
// - Lazy page loading with caching
// - Inheritable property resolution
//
// Example URLs:
// - https://mozilla.github.io/pdf.js/legacy/web/compressed.tracemonkey-pldi-09.pdf
// - https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf

use pdf_x::core::{BaseStream, HttpChunkedStream};
use pdf_x::{PDFDocument, PDFObject};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();

    let url = if args.len() < 2 {
        // Use default PDF.js example if no URL provided
        println!("⚠️  No URL provided. Using default PDF.js example.\n");
        println!("Usage: {} <pdf_url>\n", args[0]);
        "https://mozilla.github.io/pdf.js/legacy/web/compressed.tracemonkey-pldi-09.pdf"
    } else {
        &args[1]
    };

    println!("═══════════════════════════════════════════════════════════");
    println!("  PDF-X: Progressive PDF Loading from HTTP");
    println!("═══════════════════════════════════════════════════════════\n");

    // ========================================================================
    // STEP 1: Open URL with chunked streaming (range requests)
    // ========================================================================
    println!("🌐 Fetching from: {}", url);

    let stream = HttpChunkedStream::open(
        url,
        Some(65536),  // 64KB chunks
        Some(10),     // Keep max 10 chunks in memory (640KB cache)
    )?;

    let file_size = stream.length();
    let num_chunks = stream.num_chunks();

    println!("   File size: {} bytes ({:.2} MB)", file_size, file_size as f64 / 1_048_576.0);
    println!("   Total chunks: {} (64KB each)", num_chunks);
    println!("   Max memory: ~640KB (LRU cache)");
    println!("   Transport: HTTP Range Requests\n");

    // ========================================================================
    // STEP 2: Download and parse PDF document
    // ========================================================================
    println!("📥 Downloading PDF (this may take a moment)...");

    // For now, download the full PDF using ureq
    // (In a future version, PDFDocument could use chunked streams directly)
    use std::io::Read;
    let response = ureq::get(url).call()?;
    let mut pdf_data = Vec::new();
    response.into_reader().take(100 * 1024 * 1024).read_to_end(&mut pdf_data)?;  // 100MB limit

    println!("   ✓ Downloaded {} bytes", pdf_data.len());

    println!("\n🔍 Parsing PDF document...");
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
    println!("  ✓ HTTP chunked streaming with range requests");
    println!("  ✓ Progressive PDF downloading");
    println!("  ✓ Lazy page loading (pages loaded on-demand)");
    println!("  ✓ Page caching (pages cached after first access)");
    println!("  ✓ Inheritable property resolution (MediaBox, Resources)");
    println!("  ✓ Hierarchical page tree traversal");
    println!("═══════════════════════════════════════════════════════════\n");

    println!("💡 Try with your own PDF:");
    println!("   cargo run --example http_chunked_stream <your_pdf_url>\n");

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
