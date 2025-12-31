# PDF-X Usage Examples

This document provides comprehensive examples of using PDF-X for various PDF processing tasks.

## Table of Contents

1. [Basic Usage](#basic-usage)
2. [Text Extraction](#text-extraction)
3. [Progressive Loading](#progressive-loading)
4. [Async HTTP Loading](#async-http-loading)
5. [Error Handling](#error-handling)
6. [Advanced Features](#advanced-features)

## Basic Usage

### Opening a PDF File

The simplest way to work with a PDF:

```rust
use pdf_x::core::PDFDocument;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read PDF file into memory
    let pdf_data = fs::read("document.pdf")?;

    // Open the PDF
    let mut doc = PDFDocument::open(pdf_data)?;

    // Get basic information
    let page_count = doc.page_count()?;
    println!("PDF has {} pages", page_count);

    Ok(())
}
```

### Accessing PDF Metadata

```rust
use pdf_x::core::{PDFDocument, PDFObject};

fn print_metadata(doc: &mut PDFDocument) -> Result<(), Box<dyn std::error::Error>> {
    // Get catalog (root dictionary)
    let catalog = doc.catalog()?;

    if let PDFObject::Dictionary(dict) = catalog {
        // Print PDF version
        if let Some(version) = dict.get("Version") {
            println!("PDF Version: {:?}", version);
        }

        // Check for metadata stream
        if let Some(metadata) = dict.get("Metadata") {
            println!("Has XMP metadata: {:?}", metadata);
        }
    }

    Ok(())
}
```

## Text Extraction

### Extract Text from All Pages

```rust
use pdf_x::core::PDFDocument;

fn extract_all_text(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let pdf_data = std::fs::read(path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    let mut full_text = String::new();

    for page_num in 0..doc.page_count()? {
        let page = doc.get_page(page_num)?;
        let text_items = page.extract_text(&mut doc.xref_mut())?;

        for item in text_items {
            full_text.push_str(&item.text);
            full_text.push(' ');
        }
        full_text.push('\n');
    }

    Ok(full_text)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = extract_all_text("document.pdf")?;
    println!("Extracted text:\n{}", text);
    Ok(())
}
```

### Extract Text with Position Information

```rust
use pdf_x::core::PDFDocument;

fn extract_positioned_text(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pdf_data = std::fs::read(path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    let page = doc.get_page(0)?;
    let text_items = page.extract_text(&mut doc.xref_mut())?;

    for item in text_items {
        println!("Text: '{}'", item.text);
        println!("  Position: ({:.2}, {:.2})", item.position.0, item.position.1);

        if let Some(font_name) = item.font_name {
            println!("  Font: {}", font_name);
        }

        if let Some(font_size) = item.font_size {
            println!("  Size: {:.2}pt", font_size);
        }
        println!();
    }

    Ok(())
}
```

### Search for Text in PDF

```rust
use pdf_x::core::PDFDocument;

fn search_text(path: &str, query: &str) -> Result<Vec<(usize, String)>, Box<dyn std::error::Error>> {
    let pdf_data = std::fs::read(path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    let mut results = Vec::new();

    for page_num in 0..doc.page_count()? {
        let page = doc.get_page(page_num)?;
        let text_items = page.extract_text(&mut doc.xref_mut())?;

        // Concatenate all text on page
        let page_text: String = text_items.iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        // Search for query (case-insensitive)
        if page_text.to_lowercase().contains(&query.to_lowercase()) {
            results.push((page_num, page_text));
        }
    }

    Ok(results)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let results = search_text("document.pdf", "important")?;

    println!("Found '{}' on {} pages:", "important", results.len());
    for (page_num, context) in results {
        // Print first 100 chars of context
        let preview = if context.len() > 100 {
            &context[..100]
        } else {
            &context
        };
        println!("  Page {}: {}...", page_num + 1, preview);
    }

    Ok(())
}
```

## Progressive Loading

### Load Large PDF with Chunked Streaming

```rust
use pdf_x::core::{PDFDocument, FileChunkedStream};

fn process_large_pdf(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Open as chunked stream (doesn't load entire file)
    let stream = FileChunkedStream::open(path)?;

    println!("File size: {} bytes", stream.length());
    println!("Chunk size: 64KB");
    println!("Will load on-demand as needed");

    // Parse PDF structure (progressive)
    let mut doc = PDFDocument::open(Box::new(stream))?;

    println!("Pages: {}", doc.page_count()?);

    // Process first page only (only loads necessary chunks)
    let page = doc.get_page(0)?;
    let text = page.extract_text(&mut doc.xref_mut())?;

    println!("First page has {} text items", text.len());

    Ok(())
}
```

### Process PDF Page-by-Page

```rust
use pdf_x::core::{PDFDocument, FileChunkedStream};

fn process_pages_incrementally(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let stream = FileChunkedStream::open(path)?;
    let mut doc = PDFDocument::open(Box::new(stream))?;

    let page_count = doc.page_count()?;

    for page_num in 0..page_count {
        println!("Processing page {}/{}...", page_num + 1, page_count);

        let page = doc.get_page(page_num)?;
        let text_items = page.extract_text(&mut doc.xref_mut())?;

        // Process text items
        let word_count: usize = text_items.iter()
            .map(|item| item.text.split_whitespace().count())
            .sum();

        println!("  Words: {}", word_count);

        // Clear cache periodically to save memory
        if page_num % 10 == 0 {
            doc.xref_mut().cache_clear();
        }
    }

    Ok(())
}
```

## Async HTTP Loading

### Load PDF from URL (Async)

```rust
use pdf_x::core::AsyncHttpChunkedStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf";

    println!("Loading PDF from URL...");

    let stream = AsyncHttpChunkedStream::open(
        url,
        Some(65536),  // 64KB chunks
        Some(10),     // Cache 10 chunks
        None,         // No progress callback
    ).await?;

    println!("✓ PDF loaded: {} bytes", stream.length());

    Ok(())
}
```

### Load PDF with Progress Tracking

```rust
use pdf_x::core::AsyncHttpChunkedStream;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = "https://example.com/large_document.pdf";

    // Create progress callback
    let progress = Box::new(|loaded: usize, total: usize| {
        let percent = (loaded * 100) / total;
        print!("\rDownloading: {}% ({}/{} bytes)", percent, loaded, total);
        std::io::stdout().flush().unwrap();
    });

    println!("Starting download...");

    let stream = AsyncHttpChunkedStream::open(
        url,
        Some(65536),   // 64KB chunks
        Some(20),      // Cache 20 chunks (1.3MB)
        Some(progress),
    ).await?;

    println!("\n✓ Download complete!");
    println!("  Size: {} bytes", stream.length());
    println!("  Chunks: {}", stream.num_chunks());

    Ok(())
}
```

### Validate PDF Before Full Download

```rust
use pdf_x::core::AsyncHttpChunkedStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = "https://example.com/document.pdf";

    let mut stream = AsyncHttpChunkedStream::open(url, None, None, None).await?;

    // Read just the header (8 bytes)
    let header = stream.get_bytes(8).await?;

    if !header.starts_with(b"%PDF-") {
        eprintln!("✗ Not a valid PDF file");
        return Ok(());
    }

    let version = String::from_utf8_lossy(&header[5..8]);
    println!("✓ Valid PDF file (version {})", version);
    println!("  Full size: {} bytes", stream.length());
    println!("  Header check used only 8 bytes");

    Ok(())
}
```

### Synchronous HTTP Loading

```rust
use pdf_x::core::HttpChunkedStream;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = "https://example.com/document.pdf";

    println!("Loading PDF...");

    // Synchronous API (blocks until header is loaded)
    let stream = HttpChunkedStream::open(
        url,
        Some(65536),  // 64KB chunks
        Some(10),     // Cache 10 chunks
    )?;

    println!("✓ PDF loaded: {} bytes", stream.length());

    Ok(())
}
```

## Error Handling

### Comprehensive Error Handling

```rust
use pdf_x::core::{PDFDocument, PDFError};

fn safe_open_pdf(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pdf_data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Failed to read file: {}", e);
            return Err(Box::new(e));
        }
    };

    let mut doc = match PDFDocument::open(pdf_data) {
        Ok(doc) => doc,
        Err(PDFError::CorruptedPDF { message }) => {
            eprintln!("PDF is corrupted: {}", message);
            eprintln!("This PDF may be damaged or incomplete");
            return Ok(()); // Handle gracefully
        }
        Err(PDFError::ParseError { message, context, position }) => {
            eprintln!("Parse error: {}", message);
            if let Some(ctx) = context {
                eprintln!("  Context: {}", ctx);
            }
            if let Some(pos) = position {
                eprintln!("  At byte offset: {}", pos);
            }
            return Ok(()); // Handle gracefully
        }
        Err(PDFError::Unsupported { feature }) => {
            eprintln!("Unsupported PDF feature: {}", feature);
            eprintln!("This PDF uses features not yet implemented");
            return Ok(()); // Handle gracefully
        }
        Err(e) => {
            eprintln!("Unexpected error: {}", e);
            return Err(Box::new(e));
        }
    };

    // Successfully opened - process the PDF
    let page_count = doc.page_count()?;
    println!("✓ Successfully opened PDF with {} pages", page_count);

    Ok(())
}
```

### Retry Logic for Network Loading

```rust
use pdf_x::core::{AsyncHttpChunkedStream, PDFError};

async fn load_with_retry(
    url: &str,
    max_retries: usize,
) -> Result<AsyncHttpChunkedStream, Box<dyn std::error::Error>> {
    let mut attempts = 0;

    loop {
        attempts += 1;

        match AsyncHttpChunkedStream::open(url, None, None, None).await {
            Ok(stream) => {
                println!("✓ Connected on attempt {}", attempts);
                return Ok(stream);
            }
            Err(PDFError::IOError { message }) if attempts < max_retries => {
                eprintln!("✗ Attempt {} failed: {}", attempts, message);
                eprintln!("  Retrying in 2 seconds...");
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
            Err(e) => {
                return Err(Box::new(e));
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stream = load_with_retry(
        "https://example.com/document.pdf",
        3,  // Max 3 retries
    ).await?;

    println!("Successfully loaded PDF: {} bytes", stream.length());
    Ok(())
}
```

### Graceful Degradation

```rust
use pdf_x::core::{PDFDocument, PDFError};

fn extract_text_safely(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let pdf_data = std::fs::read(path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    let mut text = String::new();

    for page_num in 0..doc.page_count()? {
        match doc.get_page(page_num) {
            Ok(page) => {
                match page.extract_text(&mut doc.xref_mut()) {
                    Ok(text_items) => {
                        for item in text_items {
                            text.push_str(&item.text);
                            text.push(' ');
                        }
                    }
                    Err(e) => {
                        // Log error but continue with other pages
                        eprintln!("Warning: Page {} text extraction failed: {}",
                                 page_num, e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: Could not load page {}: {}", page_num, e);
            }
        }
    }

    Ok(text)
}
```

## Advanced Features

### Working with XRef Table Directly

```rust
use pdf_x::core::{PDFDocument, PDFObject};

fn inspect_xref(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pdf_data = std::fs::read(path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    let xref = doc.xref_mut();

    println!("XRef table size: {} entries", xref.len());

    // Inspect specific object
    let obj_num = 1;
    if let Some(entry) = xref.get_entry(obj_num) {
        println!("Object {} info:", obj_num);
        println!("  Free: {}", entry.is_free());
        println!("  Generation: {}", entry.generation());
    }

    // Fetch and print object
    let obj = xref.fetch(1, 0)?;
    println!("Object 1 value: {:?}", obj);

    Ok(())
}
```

### Custom Content Stream Processing

```rust
use pdf_x::core::{PDFDocument, PDFObject};

fn process_content_stream(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pdf_data = std::fs::read(path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    let page = doc.get_page(0)?;

    // Get page dictionary
    let page_dict = page.page_dict();

    if let PDFObject::Dictionary(dict) = page_dict {
        // Access Contents stream
        if let Some(contents) = dict.get("Contents") {
            println!("Contents: {:?}", contents);

            // For advanced use: parse content stream operators
            // This would require accessing the raw stream data
        }

        // Access Resources
        if let Some(resources) = dict.get("Resources") {
            if let PDFObject::Dictionary(res_dict) = resources {
                // List fonts
                if let Some(PDFObject::Dictionary(fonts)) = res_dict.get("Font") {
                    println!("Fonts on page:");
                    for (name, _font_obj) in fonts {
                        println!("  {}", name);
                    }
                }
            }
        }
    }

    Ok(())
}
```

### Batch Processing

```rust
use pdf_x::core::PDFDocument;
use std::path::{Path, PathBuf};
use std::fs;

fn process_directory(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut total = 0;
    let mut success = 0;
    let mut failed = Vec::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("pdf") {
            total += 1;

            match process_single_pdf(&path) {
                Ok(_) => {
                    success += 1;
                    println!("✓ {}", path.display());
                }
                Err(e) => {
                    failed.push(path.clone());
                    eprintln!("✗ {}: {}", path.display(), e);
                }
            }
        }
    }

    println!("\nResults:");
    println!("  Total: {}", total);
    println!("  Success: {} ({:.1}%)", success,
             (success as f64 / total as f64) * 100.0);
    println!("  Failed: {}", failed.len());

    Ok(())
}

fn process_single_pdf(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let pdf_data = fs::read(path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    // Do something with the PDF
    let _page_count = doc.page_count()?;

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    process_directory(Path::new("./pdfs"))?;
    Ok(())
}
```

## Performance Tips

### 1. Use Chunked Loading for Large Files

```rust
// ✓ GOOD - for large files
let stream = FileChunkedStream::open("large.pdf")?;
let doc = PDFDocument::open(Box::new(stream))?;

// ✗ BAD - loads entire file into memory
let data = std::fs::read("large.pdf")?;
let doc = PDFDocument::open(data)?;
```

### 2. Clear Cache Periodically

```rust
for page_num in 0..doc.page_count()? {
    let page = doc.get_page(page_num)?;
    // ... process page ...

    // Clear cache every 10 pages to save memory
    if page_num % 10 == 0 {
        doc.xref_mut().cache_clear();
    }
}
```

### 3. Choose Appropriate Chunk Sizes

```rust
// For fast networks
let stream = AsyncHttpChunkedStream::open(url, Some(256 * 1024), None, None).await?;

// For slow/unreliable networks
let stream = AsyncHttpChunkedStream::open(url, Some(16 * 1024), None, None).await?;
```

## See Also

- [API Documentation](async-http-loading.md) - Complete API reference
- [Robustness Testing](robustness-testing.md) - Error handling details
- [examples/](../examples/) - More code examples
- [README.md](../README.md) - Project overview
