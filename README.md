# PDF-X

[![Rust](https://img.shields.io/badge/rust-1.70+-blue.svg)](https://www.rust-lang.org)
[![Crates.io](https://img.shields.io/crates/v/pdf-x.svg)](https://crates.io/crates/pdf-x)
[![Documentation](https://docs.rs/pdf-x/badge.svg)](https://docs.rs/pdf-x)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**PDF-X** is a high-performance, **composable PDF core library** for Rust, ported from Mozilla's PDF.js. The core is synchronous, runtime-free, and WASM-clean, and is embedded by native (Tauri), web, and WASM viewer/editor shells.

- **Progressive loading on every source** — bounded memory for local *and* network PDFs, via a pull-based data protocol owned by the shell (no whole-file loads).
- **Viewer-first, editor-ready by construction** — the `delta → base` object-resolution path is wired in from day one, so editing is added by populating the delta, with no viewer refactor.
- **Resolve-then-interpret** — the resolver owns all progressive-loading complexity; interpretation is a pure synchronous function over an immutable, `Send + Sync` page model, enabling parallel rendering.

## 🚀 Features

- **Composable Core** - One sync, runtime-free library used by native/web/wasm shells
- **Progressive Loading** - Bounded-memory loading on every source (file, HTTP, memory)
- **Async HTTP Loading** - Range requests from URLs (optional `async` feature)
- **Rendering** - Feature `rendering` (pulls image formats + `crypto`): page → RGBA via a
  `Device` backend (`pdf-x-device-tinyskia`)
- **Encryption** - RC4 / AES-128 / AES-256; empty-password auto-auth; `open_with_password`
- **Text Extraction** - Extract text with position and font information
- **Linearized PDF Support** - Fast first-page display for web-optimized PDFs
- **Robust recovery** - Broken xref / partial flate streams recover like pdf.js/hayro
- **Editor-Ready** - Delta layer + incremental-update writer built into object resolution
- **High Performance** - Optimized with LRU caching, FxHashMap, and SmallVec
- **Memory Safe** - Built with Rust's safety guarantees
- **Cross-Platform** - Works on Windows, macOS, Linux, and WebAssembly
- **CLI Tool** - Command-line utility for PDF inspection

## 📦 Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
pdf-x = "0.1.0"

# Optional features:
#   rendering — Device surface + ICC + advanced image formats + crypto (AES-256)
#   crypto    — SHA-2 only (also pulled in by `rendering`)
#   async     — HTTP range loading
pdf-x = { version = "0.1.0", features = ["rendering"] }
```

## 🎯 Quick Start

```rust
use pdf_x::core::PDFDocument;

// Open a PDF file (empty-password encrypted docs auto-authenticate)
let pdf_data = std::fs::read("document.pdf")?;
let mut doc = PDFDocument::open(pdf_data)?;

// Or open a password-protected PDF:
// let mut doc = PDFDocument::open_with_password(pdf_data, b"user-password")?;

// Get page count
let page_count = doc.page_count()?;
println!("PDF has {} pages", page_count);

// Extract text from first page
let page = doc.get_page(0)?;
let text_items = page.extract_text(&mut doc.xref_mut())?;

for item in text_items {
    println!("Text: '{}' at {:?}", item.text, item.position);
}
```

### Render a page (native)

```bash
# Build the raw RGBA example (rendering feature includes crypto + JPX/JBIG2)
cargo build --release -p pdf-x-core --example render_page_png --features pdf-x-core/rendering

# Usage: render_page_png <pdf> <page0> <scale> <out.raw> [password]
# out.raw = u32le width, u32le height, RGBA8 pixels
target/release/examples/render_page_png document.pdf 0 1.0 /tmp/page.raw
target/release/examples/render_page_png secret.pdf 0 1.0 /tmp/page.raw mypassword
# or: PDFX_PASSWORD=mypassword target/release/examples/render_page_png secret.pdf 0 1.0 /tmp/page.raw
```

### Visual dual-lag harness

```bash
# Compare pdf-x vs mutool (default) or hayro at 72 DPI
python3 scripts/visual_diff.py hayro/hayro-tests/pdfs/custom/some.pdf
python3 scripts/visual_diff.py --oracle hayro hayro/hayro-tests/pdfs/custom/some.pdf
```

### Quick page preview (real PDFs)

```bash
# Render page 0 to PNG (optional mutool side-by-side)
python3 scripts/view_page.py document.pdf --scale 1.25 --compare --out-dir /tmp/views

# Smoke-scan a folder for crash / blank / dark-collapse vs mutool
python3 scripts/scan_real_pdfs.py ~/Downloads --limit 40 --compare-dark
```

## 📚 Examples

The `examples/` directory contains comprehensive examples:

```bash
# Basic PDF processing
cargo run --example basic_usage document.pdf

# Advanced text extraction
cargo run --example text_extraction document.pdf

# Progressive loading
cargo run --example progressive_loading large_document.pdf

# Async HTTP loading (requires 'async' feature)
cargo run --example http_loading --features async -- https://example.com/document.pdf

# Error handling
cargo run --example error_handling
```

## 🏗️ Architecture

PDF-X is a **composable core + shells** architecture:

- **`pdf-x-core`** (sync, runtime-free, WASM-clean): `DocStore` (single `delta → base` resolution point) → `Resolver` (owns progressive loading) → `Interpreter` over immutable `ResolvedPage` → `Device` trait. No tokio, no threads, no rendering backend baked in.
- **Shells** (`tauri-app`, future web/wasm): provide I/O, async, threading, and the `Device` backend; drive the pull-based `Need::Range` data protocol.
- Standard font fallback in core is deterministic and bundled; native shells may later inject platform fonts through an adapter hook without adding filesystem scanning to core.

Core phases (ported from PDF.js): data source → parser → document → content/interpretation, with the delta editor layer wired into object resolution.

### Progressive Loading

PDF-X supports memory-efficient progressive loading:

```rust
use pdf_x::core::{PDFDocument, FileChunkedStream};

// For large files, use chunked loading
let mut stream = FileChunkedStream::open("large.pdf")?;
let mut doc = PDFDocument::open(Box::new(stream))?;

// Pages are loaded on-demand, not all at once
let page = doc.get_page(0)?; // Triggers progressive loading
```

### Async HTTP Loading

Load PDFs from URLs with progress tracking (requires `async` feature):

```rust
use pdf_x::core::AsyncHttpChunkedStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create progress callback
    let progress = Box::new(|loaded: usize, total: usize| {
        println!("Progress: {}%", (loaded * 100) / total);
    });

    // Load PDF from URL
    let stream = AsyncHttpChunkedStream::open(
        "https://example.com/document.pdf",
        Some(65536),  // 64KB chunks
        Some(10),     // Cache 10 chunks
        Some(progress),
    ).await?;

    println!("PDF loaded: {} bytes", stream.length());

    // Read PDF header
    let mut stream = stream;
    let header = stream.get_bytes(8).await?;
    println!("Header: {:?}", String::from_utf8_lossy(&header));

    Ok(())
}
```

For synchronous HTTP loading, use `HttpChunkedStream` (wraps async with blocking runtime):

```rust
use pdf_x::core::HttpChunkedStream;

let stream = HttpChunkedStream::open(
    "https://example.com/document.pdf",
    Some(65536),  // 64KB chunks
    Some(10),     // Cache 10 chunks
)?;

println!("PDF loaded: {} bytes", stream.length());
```

## 📄 Text Extraction

Extract detailed text information:

```rust
let mut doc = PDFDocument::open(pdf_data)?;
let page = doc.get_page(0)?;
let text_items = page.extract_text(&mut doc.xref_mut())?;

for item in text_items {
    println!("Text: {}", item.text);
    println!("Font: {:?}", item.font_name);
    println!("Size: {:?}", item.font_size);
    println!("Position: {:?}", item.position);
}
```

## 🔧 CLI Tool

PDF-X includes a command-line tool for PDF inspection:

```bash
# Analyze PDF structure
cargo run --bin pdf-inspect document.pdf

# Sample output:
# PDF Structure Analysis
# =====================
# File: document.pdf
# Pages: 15
# Size: 1.2 MB
#
# Object 1: Catalog (Root)
#   Type: Catalog
#   Pages: 2 0 R
#
# Object 2: Pages
#   Type: Pages
#   Count: 15
#   Kids: [3 0 R 4 0 R ...]
```

## 🌐 WebAssembly Support

PDF-X works in web browsers via WebAssembly:

```toml
[dependencies]
pdf-x = { version = "0.1.0", features = ["web"] }
```

## 📊 Performance

PDF-X is optimized for performance with multiple techniques:

- **Lazy Loading** - Pages and content loaded only when needed
- **Memory Efficient** - Chunked data processing with LRU caching (1000 object cache)
- **Fast Hash Maps** - FxHashMap for 30% faster object lookups
- **Small Vector Optimization** - SmallVec for 50% faster small arrays
- **Pull-Based Loading** - Progressive loading via a `Need::Range` data protocol owned by the shell
- **Send + Sync** - Core types are owned and thread-safe; shells render snapshots in parallel

Benchmark results on a 10MB PDF:

| Operation | Time | Memory Usage |
|-----------|------|-------------|
| Parse | 45ms | ~5MB |
| First Page | 12ms | ~1MB |
| All Text | 180ms | ~3MB |

## 🔒 Error Handling

PDF-X provides comprehensive error types with graceful degradation for corrupt PDFs:

```rust
use pdf_x::core::{PDFDocument, PDFError};

match PDFDocument::open(pdf_data) {
    Ok(mut doc) => {
        // Success - process PDF
        let text = extract_text(&mut doc)?;
        println!("Extracted {} characters", text.len());
    }
    Err(PDFError::CorruptedPDF { message }) => {
        eprintln!("PDF appears corrupted: {}", message);
        // Gracefully handle corrupt PDFs
    }
    Err(PDFError::ParseError { message, context, position }) => {
        eprintln!("Parse error at byte {:?}: {}", position, message);
        if let Some(ctx) = context {
            eprintln!("Context: {}", ctx);
        }
    }
    Err(PDFError::DataMissing { position, length }) => {
        eprintln!("Missing {} bytes at position {}", length, position);
        // For progressive loading - load the missing chunk and retry
    }
    Err(e) => {
        eprintln!("Error: {}", e);
    }
}
```

**Robustness:** PDF-X achieves **95.9% compatibility** (752/784 PDFs) with the Mozilla PDF.js test suite, gracefully handling corrupt and malformed PDFs without panicking.

## 🧪 Testing

Run the test suite:

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_get_page

# Run robustness tests (requires pdf.js submodule)
cargo test --test robustness -- --ignored --nocapture
```

**Test Coverage:**
- 155 library tests passing (100% success rate)
- 752/784 PDFs from Mozilla PDF.js test suite (95.9% compatibility)
- Zero panics on corrupt input

## 📖 API Documentation

- [Crate Documentation](https://docs.rs/pdf-x)
- [Examples](examples/)
- [Source Code](https://github.com/your-repo/pdf-x)

## 🤝 Contributing

Contributions are welcome! Please read our [Contributing Guide](CONTRIBUTING.md) for details.

### Development Setup

```bash
# Clone the repository
git clone https://github.com/your-repo/pdf-x.git
cd pdf-x

# Run tests
cargo test

# Run examples
cargo run --example basic_usage

# Build with optimizations
cargo build --release
```

## 📋 Roadmap

- [x] Core PDF parsing
- [x] Text extraction
- [x] Progressive loading
- [x] Linearized PDF support
- [x] CLI tool
- [x] Performance optimizations (LRU cache, FxHashMap, SmallVec)
- [x] Async HTTP loading with range requests
- [x] Comprehensive robustness testing (95.9% compatibility)
- [x] Zero-panic error handling
- [ ] Font encoding improvements (ToUnicode CMap)
- [ ] Image rendering
- [ ] Form support
- [ ] Annotation handling
- [ ] Digital signatures
- [ ] WebAssembly bindings

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- **Mozilla PDF.js** - Architecture inspiration and algorithms
- **Rust Community** - For excellent async/streaming patterns
- **PDF Specification** - ISO 32000-1 (PDF 1.7)

## 🔗 Related Projects

- [pdf.js](https://github.com/mozilla/pdf.js) - JavaScript PDF viewer
- [pdf-rs](https://github.com/pdf-rs/pdf) - Another Rust PDF library
- [lopdf](https://github.com/jfbouzac/lopdf) - Rust PDF writer

---

**PDF-X** - High-performance PDF processing in Rust 🦀

Made with ❤️ for the Rust community
