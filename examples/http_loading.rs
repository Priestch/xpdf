//! Example: Load PDF from URL with async HTTP loading
//!
//! This example demonstrates how to load a PDF from a URL using async HTTP range requests.
//! It shows both sync and async APIs with progress tracking.
//!
//! Run with:
//! ```bash
//! cargo run --example http_loading --features async -- <url>
//! ```

#[cfg(feature = "async")]
use pdf_x::core::AsyncHttpChunkedStream;

#[cfg(feature = "async")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::env;

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <url>", args[0]);
        eprintln!();
        eprintln!("Example URLs:");
        eprintln!("  https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf");
        eprintln!("  https://raw.githubusercontent.com/mozilla/pdf.js/master/test/pdfs/tracemonkey.pdf");
        std::process::exit(1);
    }

    let url = &args[1];

    println!("Loading PDF from URL: {}", url);
    println!();

    // Create progress callback
    let progress_callback = Box::new(|loaded: usize, total: usize| {
        let percent = if total > 0 {
            (loaded * 100) / total
        } else {
            0
        };
        print!("\rProgress: {}% ({}/{} bytes)", percent, loaded, total);
        use std::io::Write;
        std::io::stdout().flush().unwrap();
    });

    // Open PDF from URL with progress tracking
    let stream = AsyncHttpChunkedStream::open(
        url,
        Some(65536),  // 64KB chunks
        Some(10),     // Cache 10 chunks (640KB memory)
        Some(progress_callback),
    )
    .await?;

    println!("\n\n✓ PDF loaded successfully!");
    println!("  File size: {} bytes", stream.length());
    println!("  Total chunks: {}", stream.num_chunks());
    println!("  Chunks loaded: {}", stream.num_chunks_loaded().await);
    println!("  Chunk size: 64KB");
    println!("  Cache size: 10 chunks (640KB)");
    println!();

    // Read first few bytes to verify
    let mut stream = stream;
    let header = stream.get_bytes(8).await?;
    println!("PDF header: {:?}", String::from_utf8_lossy(&header));
    println!();

    println!("✓ Phase 3 (Network Loading) complete!");
    println!();
    println!("Next steps:");
    println!("  - Create fully async PDFDocument::open_url() API");
    println!("  - Implement async document parsing");
    println!("  - Test with large PDFs");

    Ok(())
}

#[cfg(not(feature = "async"))]
fn main() {
    eprintln!("This example requires the 'async' feature.");
    eprintln!("Run with: cargo run --example http_loading --features async -- <url>");
    std::process::exit(1);
}
