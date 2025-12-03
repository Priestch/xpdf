# PDF-X

A Rust implementation of PDF.js, bringing high-performance PDF rendering with progressive loading capabilities to native applications.

## Overview

PDF-X is a Rust port of Mozilla's popular [PDF.js](https://github.com/mozilla/pdf.js) library. This project aims to provide a memory-efficient, performant PDF parser and renderer while maintaining the proven architecture and progressive loading features that made PDF.js successful.

## Features

- **Progressive Loading**: Load and render PDF documents incrementally, allowing users to view pages while the document is still being downloaded or parsed
- **Lazy Loading**: Pages are parsed and rendered on-demand, reducing memory footprint for large documents
- **PDF.js Architecture**: Follows the battle-tested design patterns and architecture of PDF.js
- **Rust Performance**: Leverages Rust's memory safety and zero-cost abstractions for improved performance and reliability
- **Streaming Support**: Process PDF files without loading the entire document into memory

## Architecture

PDF-X follows the layered architecture of PDF.js:

### Core Layers

1. **Network Layer**: Handles progressive data fetching and range requests
2. **Parser Layer**: Incremental PDF object parsing and cross-reference table processing
3. **Document Structure Layer**: Manages page tree, metadata, and document catalog
4. **Rendering Layer**: Converts PDF content streams to displayable graphics

### Progressive Loading Pipeline

```
┌─────────────┐
│  Data Source│ (HTTP, File, Stream)
└──────┬──────┘
       │ Chunks
       ▼
┌─────────────┐
│   Parser    │ Progressive parsing
└──────┬──────┘
       │ Objects
       ▼
┌─────────────┐
│  Document   │ Build page tree
└──────┬──────┘
       │ Pages
       ▼
┌─────────────┐
│   Renderer  │ On-demand rendering
└─────────────┘
```

## Project Status

**Current Phase:** Page tree traversal and lazy page loading complete ✅

### Completed Layers

1. ✅ **Data Source Layer** - Chunked streaming from multiple sources
   - In-memory streams with Arc-based sharing
   - File chunked streams with LRU caching
   - HTTP chunked streams with range requests
   - Sub-stream abstraction for efficient slicing

2. ✅ **Lexer Layer** - Complete PDF tokenization
   - All PDF primitive types (numbers, strings, names, booleans, null)
   - Hex strings and literal strings with escape sequences
   - Array and dictionary delimiters
   - Command/operator tokens
   - 39 comprehensive tests

3. ✅ **Parser Layer** - PDF object construction
   - Recursive parsing of arrays and dictionaries
   - Indirect object reference detection (N G R pattern)
   - Nested structure support
   - 22 comprehensive tests

4. ✅ **XRef Layer** - Cross-reference table parsing
   - XRef table parsing (free/uncompressed entries)
   - Indirect object resolution and caching with Rc<PDFObject>
   - Trailer dictionary extraction
   - 6 comprehensive tests

5. ✅ **Document Layer** - High-level PDF interface
   - PDF document opening and parsing
   - Catalog (root) dictionary access
   - Page count extraction
   - Pages dictionary access
   - 4 comprehensive tests

6. ✅ **Page Layer** - Page tree traversal and lazy loading
   - Hierarchical page tree traversal (depth-first search)
   - Lazy page loading with caching
   - Support for flat and multi-level page trees
   - Circular reference detection
   - Page dictionary access (MediaBox, Resources, Contents)
   - **Inheritable properties**: Automatic resolution of inherited properties (MediaBox, Resources, CropBox, Rotate) from parent Pages nodes
   - 10 comprehensive tests including hierarchical page trees and property inheritance

7. ✅ **Stream Parsing & Compression** - FlateDecode and object streams
   - Stream object parsing (dictionary + binary data)
   - FlateDecode (zlib/deflate) decompression
   - **Compressed object streams (ObjStm)**: Support for PDFs that compress multiple objects into a single stream
   - Automatic decompression and object extraction
   - 4 comprehensive tests for stream decompression

**Test Coverage:** 120 tests passing (all green ✅)

### In Progress / Next Steps

- [x] Compressed object streams (ObjStm) - Basic implementation complete
- [ ] XRef streams (compressed cross-reference tables)
- [ ] Linearized PDF optimization
- [ ] Content stream parsing
- [ ] Text extraction
- [ ] Image rendering
- [ ] Font handling
- [ ] Annotation support

## Why Rust?

- **Memory Safety**: Eliminates entire classes of bugs common in PDF parsers
- **Performance**: Native speed without garbage collection overhead
- **Concurrency**: Safe parallel processing of pages and resources
- **Embedded Systems**: Deploy PDF rendering in resource-constrained environments
- **WebAssembly**: Compile to WASM for browser integration while maintaining native performance

## Inspiration

This project draws inspiration from:
- [PDF.js](https://github.com/mozilla/pdf.js) - Architecture and progressive loading design
- [pdf-rs](https://github.com/pdf-rs/pdf) - Rust PDF parsing patterns
- The PDF 1.7 specification (ISO 32000-1)

## Getting Started

```bash
# Clone the repository
git clone https://github.com/yourusername/pdf-x.git
cd pdf-x

# Initialize the pdf.js submodule (used as reference for implementation)
git submodule update --init --recursive

# Build the project
cargo build

# Run tests
cargo test
```

### PDF.js Reference Submodule

This project includes the original PDF.js repository as a git submodule under `pdf.js/`. This serves as:

- **Implementation reference** for code agents and developers
- **Architecture documentation** through working code examples
- **Algorithm reference** for progressive loading and parsing logic
- **Test case validation** by comparing behavior with the original

The submodule allows AI code agents to analyze the proven PDF.js implementation while writing the Rust port, ensuring architectural fidelity and correctness.

## Usage

```rust
use pdf_x::PDFDocument;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read PDF from file
    let pdf_data = std::fs::read("document.pdf")?;

    // Open the PDF document
    let mut doc = PDFDocument::open(pdf_data)?;

    // Get page count
    let page_count = doc.page_count()?;
    println!("Total pages: {}", page_count);

    // Access the catalog (root dictionary)
    if let Some(catalog) = doc.catalog() {
        println!("Catalog: {:?}", catalog);
    }

    // Get the Pages dictionary
    let pages_dict = doc.pages_dict()?;
    println!("Pages: {:?}", pages_dict);

    // Lazily load a specific page (pages are cached)
    let page = doc.get_page(0)?;  // Get first page (0-indexed)
    println!("Page {}: {:?}", page.index(), page.dict());

    // Access page properties (with automatic inheritance from parent Pages nodes)
    let media_box = doc.get_media_box(&page)?;
    println!("MediaBox: {:?}", media_box);

    // Get Resources (inheritable)
    if let Ok(resources) = doc.get_resources(&page) {
        println!("Resources: {:?}", resources);
    }

    Ok(())
}
```

See `examples/read_pdf.rs` for a complete working example.

### Running Examples

```bash
# Run the basic PDF reader example (parses a minimal test PDF)
cargo run --example read_pdf

# Run the file chunked stream example (demonstrates progressive loading from file)
cargo run --example file_chunked_stream <path_to_pdf>

# Example:
cargo run --example file_chunked_stream ~/Documents/document.pdf

# Run the HTTP chunked stream example (demonstrates progressive loading from HTTP)
cargo run --example http_chunked_stream <pdf_url>

# Example:
cargo run --example http_chunked_stream https://mozilla.github.io/pdf.js/legacy/web/compressed.tracemonkey-pldi-09.pdf

# Run all tests
cargo test

# Run specific test module
cargo test lexer
cargo test parser
cargo test xref
```

The chunked stream examples demonstrate:
- **Progressive PDF loading** from file or HTTP
- **Lazy page loading** (pages loaded on-demand)
- **Page caching** (pages cached after first access)
- **Inheritable property resolution** (MediaBox, Resources automatically inherited from parent Pages nodes)
- **Hierarchical page tree traversal** (DFS through multi-level page trees)
- **Memory-efficient streaming** with LRU caching (max 640KB for chunk cache)

## Contributing

Contributions are welcome! This project aims to faithfully implement PDF.js concepts in idiomatic Rust.

## License

[Choose appropriate license - MIT/Apache-2.0 are common for Rust projects]

## Acknowledgments

- The PDF.js team for creating an excellent reference architecture
- The Rust community for powerful parsing and graphics libraries
