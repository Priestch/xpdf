/// Example demonstrating the exception-driven progressive loading pattern.
///
/// This shows how to use retry_on_data_missing! macro to handle DataMissing errors.

use pdf_x::{PDFDocument, retry_on_data_missing};
use pdf_x::core::BaseStream;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Retry Pattern Demo");
        eprintln!("Usage: {} <pdf-file>", args[0]);
        eprintln!("\nThis demonstrates exception-driven progressive loading:");
        eprintln!("1. Try to parse with available data");
        eprintln!("2. If DataMissing error occurs, load the required chunks");
        eprintln!("3. Retry the operation");
        std::process::exit(1);
    }

    let pdf_path = &args[1];

    println!("═══════════════════════════════════════════════════════");
    println!("  Exception-Driven Loading Pattern Demo");
    println!("═══════════════════════════════════════════════════════\n");

    println!("📄 Opening: {}\n", pdf_path);

    // The retry pattern is built into the implementation, but here's how
    // you would use it explicitly if you were implementing a custom parser:
    //
    // retry_on_data_missing!(stream, {
    //     parser.parse_xref()
    // })
    //
    // This will:
    // 1. Try parse_xref()
    // 2. If DataMissing { position, length } is thrown:
    //    - Call stream.ensure_range(position, length)
    //    - Retry parse_xref()
    // 3. Return result or propagate other errors

    let mut doc = match PDFDocument::open_file(pdf_path, None, None) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("\n❌ Error: {:?}", e);
            std::process::exit(1);
        }
    };

    println!("✅ PDF opened successfully!\n");

    println!("═══════════════ PROGRESSIVE LOADING INFO ═══════════════");
    println!("📊 XRef entries: {}", doc.xref().len());

    if let Ok(page_count) = doc.page_count() {
        println!("📄 Pages: {}", page_count);
    }

    println!("\n═══════════════ RETRY PATTERN BENEFITS ═══════════════");
    println!("✨ Exception-Driven Loading Pattern:");
    println!("  1. Minimizes data loading - only loads what's needed");
    println!("  2. Perfect for network sources - HTTP range requests");
    println!("  3. Fast failure detection - errors propagate immediately");
    println!("  4. Automatic retry - transparently retries after loading");
    println!("  5. Same pattern as PDF.js - proven architecture");

    println!("\n💡 Macro Usage:");
    println!("  retry_on_data_missing!(stream, {{");
    println!("      parser.parse_operation()  // Any operation that may need data");
    println!("  }})");

    println!("\n═══════════════════════════════════════════════════════");
}
