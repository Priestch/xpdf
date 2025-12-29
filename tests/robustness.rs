//! Robustness test: Try to open all PDFs from pdf.js test suite
//!
//! This test checks how many PDFs from the pdf.js test suite we can successfully parse.
//! It helps identify edge cases and areas that need better error handling.

use pdf_x::core::PDFDocument;
use std::path::Path;

#[test]
#[ignore] // Run manually: cargo test --test robustness -- --ignored --nocapture
fn test_pdf_js_test_suite() {
    let test_dir = Path::new("pdf.js/test/pdfs");

    if !test_dir.exists() {
        eprintln!("Test directory not found: {:?}", test_dir);
        eprintln!("Make sure pdf.js submodule is initialized");
        return;
    }

    let mut total = 0;
    let mut success = 0;
    let mut failures = Vec::new();

    println!("\n📊 Testing PDF.js test suite robustness...\n");

    // Get all PDF files
    let entries = std::fs::read_dir(test_dir).expect("Failed to read test directory");

    for entry in entries {
        let entry = entry.expect("Failed to read entry");
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("pdf") {
            continue;
        }

        total += 1;
        let filename = path.file_name().unwrap().to_str().unwrap();

        // Try to open the PDF
        match std::fs::read(&path) {
            Ok(data) => {
                match PDFDocument::open(data) {
                    Ok(mut doc) => {
                        success += 1;
                        let pages = doc.page_count().unwrap_or(0);
                        println!("✓ {} ({} pages)", filename, pages);
                    }
                    Err(e) => {
                        failures.push((filename.to_string(), format!("Parse error: {}", e)));
                        println!("✗ {} - {}", filename, e);
                    }
                }
            }
            Err(e) => {
                failures.push((filename.to_string(), format!("Read error: {}", e)));
                println!("✗ {} - {}", filename, e);
            }
        }
    }

    println!("\n📈 Results:");
    println!("  Total PDFs:    {}", total);
    println!("  Successful:    {} ({:.1}%)", success, (success as f64 / total as f64) * 100.0);
    println!("  Failed:        {} ({:.1}%)", failures.len(), (failures.len() as f64 / total as f64) * 100.0);

    if !failures.is_empty() {
        println!("\n❌ Failed PDFs:");
        for (name, error) in &failures {
            println!("  - {}: {}", name, error);
        }
    }

    // For now, we just report - don't fail the test
    // As we improve robustness, we can increase the success threshold
    println!("\n✅ Robustness test complete");
}

#[test]
#[ignore] // Run manually: cargo test --test robustness -- --ignored --nocapture
fn test_specific_problematic_pdfs() {
    println!("\n🔍 Testing specific known-problematic PDFs...\n");

    let test_cases = vec![
        "pdf.js/test/pdfs/tracemonkey.pdf",  // Large academic paper
        "pdf.js/test/pdfs/issue7872.pdf",    // Known edge case
        "pdf.js/test/pdfs/bug1065245.pdf",   // Known bug case
        "pdf.js/test/pdfs/TAMReview.pdf",    // Complex formatting
    ];

    for pdf_path in test_cases {
        let path = Path::new(pdf_path);
        let filename = path.file_name().unwrap().to_str().unwrap();

        println!("Testing: {}", filename);

        if !path.exists() {
            println!("  ⚠️  File not found (skipped)");
            continue;
        }

        match std::fs::read(path) {
            Ok(data) => {
                match PDFDocument::open(data) {
                    Ok(mut doc) => {
                        println!("  ✓ Opened successfully");
                        match doc.page_count() {
                            Ok(pages) => println!("    Pages: {}", pages),
                            Err(e) => println!("    Pages: Error - {}", e),
                        }
                    }
                    Err(e) => {
                        println!("  ✗ Parse error: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("  ✗ Read error: {}", e);
            }
        }
        println!();
    }
}
