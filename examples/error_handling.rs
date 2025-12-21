//! Error Handling Example
//!
//! This example demonstrates comprehensive error handling in PDF-X:
//! - Understanding different error types
//! - Graceful error recovery
//! - Validating PDF files
//! - Common error scenarios
//!
//! Run with: cargo run --example error_handling

use pdf_x::core::{PDFDocument, PDFError};
use std::env;
use std::fmt;

/// Custom error type for application-specific PDF operations
#[derive(Debug)]
enum AppError {
    InvalidFile(String),
    ProcessingError(String),
    ValidationError(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::InvalidFile(msg) => write!(f, "Invalid file: {}", msg),
            AppError::ProcessingError(msg) => write!(f, "Processing error: {}", msg),
            AppError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
        }
    }
}

impl std::error::Error for AppError {}

/// Result type for our application
type AppResult<T> = Result<T, AppError>;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: cargo run --example error_handling <pdf_file>");
        eprintln!("\nThis example demonstrates error handling patterns.");
        demonstrate_error_scenarios();
        return;
    }

    let pdf_path = &args[1];
    println!("🛡️  Error handling example: {}", pdf_path);

    // Demonstrate various error handling scenarios
    demonstrate_file_validation(pdf_path);
    demonstrate_pdf_parsing(pdf_path);
    demonstrate_graceful_degradation(pdf_path);

    println!("\n✅ Error handling examples completed!");
}

fn demonstrate_file_validation(pdf_path: &str) {
    println!("\n📋 File Validation:");

    // Check if file exists
    match std::path::Path::new(pdf_path).exists() {
        true => println!("  ✅ File exists: {}", pdf_path),
        false => {
            println!("  ❌ File not found: {}", pdf_path);
            return;
        }
    }

    // Check file size
    match std::fs::metadata(pdf_path) {
        Ok(metadata) => {
            let size = metadata.len();
            if size == 0 {
                println!("  ❌ File is empty");
                return;
            } else {
                println!("  ✅ File size: {} bytes", size);

                if size > 100_000_000 {
                    println!("  ⚠️  Large file detected ({} MB), consider progressive loading", size / 1_000_000);
                }
            }
        }
        Err(e) => {
            println!("  ❌ Cannot read file metadata: {}", e);
            return;
        }
    }

    // Check file extension
    if let Some(extension) = std::path::Path::new(pdf_path).extension() {
        if extension == "pdf" {
            println!("  ✅ File has .pdf extension");
        } else {
            println!("  ⚠️  File extension is .{}, not .pdf", extension.to_string_lossy());
        }
    } else {
        println!("  ⚠️  No file extension");
    }
}

fn demonstrate_pdf_parsing(pdf_path: &str) {
    println!("\n📖 PDF Parsing:");

    // Read file with error handling
    let pdf_data = match std::fs::read(pdf_path) {
        Ok(data) => {
            println!("  ✅ File read successfully ({} bytes)", data.len());
            data
        }
        Err(e) => {
            println!("  ❌ Failed to read file: {}", e);
            return;
        }
    };

    // Check for PDF header
    if pdf_data.len() < 4 {
        println!("  ❌ File too small to be a PDF");
        return;
    }

    let header = String::from_utf8_lossy(&pdf_data[..4]);
    if !header.starts_with("%PDF") {
        println!("  ❌ Invalid PDF header: '{}'", header);
        return;
    }
    println!("  ✅ PDF header found: '{}'", header);

    // Try to parse PDF with detailed error handling
    match PDFDocument::open(pdf_data) {
        Ok(mut doc) => {
            println!("  ✅ PDF parsed successfully");

            // Validate PDF structure
            match validate_pdf_structure(&mut doc) {
                Ok(()) => println!("  ✅ PDF structure is valid"),
                Err(e) => println!("  ⚠️  PDF structure validation warning: {}", e),
            }

            // Show document info
            if let Ok(page_count) = doc.page_count() {
                println!("  📊 Document has {} pages", page_count);
            }
        }
        Err(e) => {
            println!("  ❌ PDF parsing failed: {}", e);

            // Provide specific guidance based on error type
            match &e {
                PDFError::CorruptedPDF { .. } => {
                    println!("  💡 Tip: File may be corrupted or password protected");
                }
                PDFError::ParseError { .. } => {
                    println!("  💡 Tip: File may not be a valid PDF or has encoding issues");
                }
                PDFError::IOError { .. } => {
                    println!("  💡 Tip: Check file permissions and disk space");
                }
                PDFError::Unsupported { feature } => {
                    println!("  💡 Tip: PDF uses unsupported feature: {}", feature);
                }
                _ => {
                    println!("  💡 Tip: Check if the file is a valid PDF document");
                }
            }
        }
    }
}

fn validate_pdf_structure(doc: &mut PDFDocument) -> AppResult<()> {
    // Validate that we can access pages
    let page_count = doc.page_count()
        .map_err(|e| AppError::ValidationError(format!("Cannot get page count: {}", e)))?;

    if page_count == 0 {
        return Err(AppError::ValidationError("PDF has no pages"));
    }

    // Try to access first page
    doc.get_page(0)
        .map_err(|e| AppError::ValidationError(format!("Cannot access first page: {}", e)))?;

    // Check if it's linearized (this is optional)
    if doc.is_linearized() {
        println!("    📋 Linearized PDF detected for fast web view");
    }

    Ok(())
}

fn demonstrate_graceful_degradation(pdf_path: &str) {
    println!("\n🔄 Graceful Degradation:");

    // Simulate a real-world processing pipeline with fallbacks
    let result = process_pdf_with_fallbacks(pdf_path);

    match result {
        Ok(summary) => {
            println!("  ✅ Processing successful:");
            for line in summary {
                println!("    {}", line);
            }
        }
        Err(e) => {
            println!("  ⚠️  Processing failed, but gracefully handled:");
            println!("    {}", e);
        }
    }
}

/// Process a PDF with multiple fallback strategies
fn process_pdf_with_fallbacks(pdf_path: &str) -> AppResult<Vec<String>> {
    let mut summary = Vec::new();

    // Fallback 1: Try normal processing
    summary.push("Attempting normal PDF processing...".to_string());

    let pdf_data = std::fs::read(pdf_path)
        .map_err(|_| AppError::InvalidFile("Cannot read file".to_string()))?;

    let mut doc = PDFDocument::open(pdf_data)
        .map_err(|e| AppError::ProcessingError(format!("PDF parsing failed: {}", e)))?;

    let page_count = doc.page_count()
        .map_err(|_| AppError::ProcessingError("Cannot get page count".to_string()))?;

    summary.push(format!("Successfully opened {}-page PDF", page_count));

    // Fallback 2: Try to extract text (may fail for certain PDFs)
    match extract_text_safely(&mut doc) {
        Ok(text_count) => {
            summary.push(format!("Extracted {} text items", text_count));
        }
        Err(e) => {
            summary.push(format!("Text extraction failed: {}", e));
        }
    }

    // Fallback 3: Get basic metadata
    if doc.is_linearized() {
        summary.push("Linearized PDF detected".to_string());
    }

    Ok(summary)
}

fn extract_text_safely(doc: &mut PDFDocument) -> AppResult<usize> {
    // Try to extract from first page only (safer than all pages)
    let page = doc.get_page(0)
        .map_err(|e| AppError::ProcessingError(format!("Cannot access page: {}", e)))?;

    let text_items = page.extract_text(&mut doc.xref_mut())
        .map_err(|e| AppError::ProcessingError(format!("Text extraction failed: {}", e)))?;

    Ok(text_items.len())
}

fn demonstrate_error_scenarios() {
    println!("\n🎯 Common Error Scenarios:");

    println!("\n1. File Errors:");
    println!("   - File not found");
    println!("   - Permission denied");
    println!("   - Corrupted files");
    println!("   - Empty files");

    println!("\n2. PDF Format Errors:");
    println!("   - Invalid PDF header");
    println!("   - Corrupted xref table");
    println!("   - Missing trailer");
    println!("   - Encrypted PDFs (not yet supported)");

    println!("\n3. Parsing Errors:");
    println!("   - Invalid object structure");
    println!("   - Malformed content streams");
    println!("   - Unsupported PDF versions");
    println!("   - Missing required objects");

    println!("\n4. Processing Errors:");
    println!("   - Out of memory for large files");
    println!("   - Invalid page numbers");
    println!("   - Corrupted font data");
    println!("   - Text extraction failures");

    println!("\n💡 Error Handling Best Practices:");
    println!("   • Always check return values (Result types)");
    println!("   • Use pattern matching on error types");
    println!("   • Provide meaningful error messages");
    println!("   • Implement graceful fallbacks");
    println!("   • Log errors for debugging");
    println!("   • Validate inputs before processing");
}

/// Example of robust error handling in a real application
#[allow(dead_code)]
fn robust_pdf_processing(pdf_path: &str) -> AppResult<String> {
    // Validate inputs first
    let path = std::path::Path::new(pdf_path);
    if !path.exists() {
        return Err(AppError::InvalidFile(format!("File not found: {}", pdf_path)));
    }

    // Check file size (avoid loading huge files)
    let metadata = std::fs::metadata(path)
        .map_err(|e| AppError::InvalidFile(format!("Cannot read metadata: {}", e)))?;

    if metadata.len() > 100_000_000 {
        return Err(AppError::ValidationError("File too large (>100MB)".to_string()));
    }

    // Read with timeout (simplified)
    let pdf_data = std::fs::read(pdf_path)
        .map_err(|e| AppError::ProcessingError(format!("Failed to read file: {}", e)))?;

    // Validate PDF header
    if pdf_data.len() < 5 || !pdf_data.starts_with(b"%PDF") {
        return Err(AppError::ValidationError("Invalid PDF format".to_string()));
    }

    // Parse with specific error handling
    let mut doc = PDFDocument::open(pdf_data)
        .map_err(|e| match e {
            PDFError::ParseError { .. } => {
                AppError::ProcessingError("PDF parsing error".to_string())
            }
            PDFError::CorruptedPDF { .. } => {
                AppError::ProcessingError("PDF appears corrupted".to_string())
            }
            _ => AppError::ProcessingError(format!("PDF error: {}", e)),
        })?;

    // Process with additional validation
    let page_count = doc.page_count()
        .map_err(|e| AppError::ProcessingError(format!("Cannot get page count: {}", e)))?;

    if page_count == 0 {
        return Err(AppError::ValidationError("PDF has no pages".to_string()));
    }

    format!("Successfully processed {}-page PDF", page_count).into()
}