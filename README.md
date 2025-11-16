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

This project is in early development. The goal is to implement:

- [ ] Core PDF parser with streaming support
- [ ] Progressive data loading
- [ ] Page-by-page lazy parsing
- [ ] Content stream interpreter
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

## Usage (Planned)

```rust
use pdf_x::{Document, LoadingConfig};

// Progressive loading from URL
let config = LoadingConfig::new()
    .enable_progressive_loading(true)
    .chunk_size(65536);

let document = Document::from_url("https://example.com/document.pdf", config).await?;

// Render pages as they become available
for page_num in 0..document.page_count() {
    let page = document.get_page(page_num).await?;
    let bitmap = page.render()?;
    // Display bitmap
}
```

## Contributing

Contributions are welcome! This project aims to faithfully implement PDF.js concepts in idiomatic Rust.

## License

[Choose appropriate license - MIT/Apache-2.0 are common for Rust projects]

## Acknowledgments

- The PDF.js team for creating an excellent reference architecture
- The Rust community for powerful parsing and graphics libraries
