# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

PDF-X is a Rust port of Mozilla's PDF.js library. The goal is to replicate PDF.js's proven architecture and progressive/lazy loading features while leveraging Rust's performance and memory safety.

**Key Design Principle**: Maintain architectural fidelity to PDF.js while implementing idiomatic Rust.

## PDF.js Reference Submodule

The `pdf.js/` directory contains the original PDF.js codebase as a git submodule. This serves as the **primary reference implementation** for:

- Architecture patterns and module structure
- Progressive loading algorithms (see `pdf.js/src/core/`)
- Chunked data handling from various sources
- Incremental parsing logic
- Page tree traversal and lazy rendering

**When implementing new features**: Always reference the corresponding JavaScript implementation in `pdf.js/src/` to understand the algorithm and data flow before writing Rust code.

### PDF.js Source Structure

- `pdf.js/src/core/`: Core parsing layer (stream handling, PDF object parsing, xref tables, fonts, images)
- `pdf.js/src/display/`: Rendering layer (canvas API, page rendering)
- `pdf.js/src/shared/`: Shared utilities between worker and main thread

## Architecture Layers

The implementation follows a four-layer architecture:

1. **Data Source Layer**: Abstract chunked data loading from multiple sources
   - Provides uniform chunk-based interface regardless of source
   - **Network Loader**: HTTP/HTTPS with range request support for progressive loading
   - **Filesystem Loader**: Local file reading with chunked streaming
   - **Memory Loader**: In-memory buffers (for testing or embedded data)
   - All loaders support asynchronous chunk delivery to enable progressive parsing

2. **Parser Layer**: Incremental PDF parsing
   - Stream-based object parsing (PDF objects parsed as data arrives)
   - Cross-reference (xref) table processing
   - Handle linearized PDFs for fast first-page display
   - Source-agnostic: works with any data source loader

3. **Document Structure Layer**: Page tree and metadata management
   - Build page tree without loading all pages
   - Catalog and metadata extraction
   - Resource dictionaries

4. **Rendering Layer**: Content stream interpretation
   - On-demand page rendering
   - Graphics state management
   - Text and image extraction

## Development Commands

Since this is an early-stage Rust project, the standard Cargo workflow applies:

```bash
# Build
cargo build
cargo build --release

# Test
cargo test
cargo test --lib          # Library tests only
cargo test <test_name>    # Single test

# Check without building
cargo check

# Format and lint
cargo fmt
cargo clippy
```

## Progressive Loading Implementation Notes

Progressive loading is the **core differentiator** of this project. Key concepts from PDF.js to replicate:

1. **Chunked Loading**: Load PDF data in chunks (typically 64KB), regardless of source
2. **Source Abstraction**: Unified interface for network, filesystem, and other sources
3. **Range Requests**: Network loader uses HTTP Range headers; filesystem loader uses seek + read
4. **Incremental Parsing**: Parse PDF objects as chunks arrive, don't wait for complete file
5. **Lazy Page Loading**: Parse page content streams only when requested, not during initial document load
6. **Linearized PDF Support**: Recognize and optimize for linearized PDFs (fast web view)

The Data Source Layer must provide a trait/interface that all loaders implement, enabling the parser to work identically whether loading from network, disk, or memory.

Reference `pdf.js/src/core/stream.js` and `pdf.js/src/core/chunked_stream.js` for implementation patterns.

### **CRITICAL RULE: Exception-Driven Progressive Loading**

This is a **NON-NEGOTIABLE** architectural principle that MUST be followed in all code:

**Load as little PDF data as possible. Operations should attempt to proceed with available data and raise exceptions when data is missing. The caller catches these exceptions, loads the required chunks, and retries the operation.**

This follows PDF.js's exception-driven data loading pattern:

```rust
// ✅ CORRECT APPROACH - Exception-driven loading
loop {
    match parser.parse_xref() {
        Ok(result) => break result,
        Err(PDFError::DataMissing { position, length }) => {
            // Load the missing chunk
            stream.ensure_range(position, length)?;
            // Retry the operation - it will now succeed or fail with a different missing range
            continue;
        }
        Err(e) => return Err(e), // Other errors propagate
    }
}
```

```rust
// ❌ WRONG APPROACH - Loading all data upfront (NEVER DO THIS)
let all_data = stream.get_all_bytes()?; // Violates progressive loading
parser.parse_xref(&all_data)?;
```

**Implementation Requirements:**

1. **Never preload data**: Don't use methods like `read_all()`, `load_complete()`, or similar patterns
2. **Define DataMissing error**: Create a `PDFError::DataMissing { position: usize, length: usize }` variant
3. **Throw on missing data**: When a read operation would require unavailable data, immediately throw `DataMissing`
4. **Retry loops at call sites**: Callers implement retry loops that load chunks and retry operations
5. **Minimal chunk requests**: Request only the specific byte range needed, not arbitrary large chunks
6. **No buffering layers**: Don't add caching/buffering that hides the progressive nature from upper layers

**When Evaluating External Crates:**

Before using any external Rust crate for PDF functionality, you must evaluate whether it supports progressive loading:

1. ✅ **Acceptable: Isolated operation crates**
   - Stream decoders (e.g., `flate2` for FlateDecode)
   - Image decoders (e.g., `png`, `jpeg-decoder`)
   - Compression algorithms that work on already-loaded data
   - Utility functions for parsing small data structures
   - These are acceptable because they operate on data you've already explicitly loaded

2. ❌ **Reject: Full-file processing crates**
   - Any crate requiring `Arc<[u8]>`, `Vec<u8>`, or similar for the entire PDF
   - Crates with `load()`, `open(path)`, `from_file()` APIs that load complete files
   - Crates that use `std::fs::read()` or equivalent in their examples
   - Crates with internal buffering that loads large file portions without your control

3. ✅ **Acceptable: Learning from source code**
   - You CAN read external crate source code to understand algorithms
   - You CAN copy/adapt implementation patterns and utility functions
   - You CAN use their approach to solving specific problems
   - You CANNOT use them as dependencies if they violate progressive loading

**Examples:**

```rust
// ✅ GOOD - Using flate2 for stream decompression
// (You already loaded this stream data progressively)
use flate2::read::ZlibDecoder;
let decompressed = decode_flate(&already_loaded_stream_data)?;

// ❌ BAD - Using hypothetical full-file crate
use some_pdf_crate::Pdf;
let data = std::fs::read(path)?;  // Loads entire file
let pdf = Pdf::new(Arc::new(data))?;  // Requires full data

// ✅ GOOD - Learning from external crate source
// Copy algorithm pattern, adapt to progressive loading
fn parse_xref_field(stream: &mut ChunkedStream, offset: usize, width: u8)
    -> Result<u32, PDFError>
{
    // This pattern learned from external crate source
    // But adapted to use YOUR chunked stream with DataMissing errors
    let bytes = stream.get_bytes(offset, width as usize)?;  // Can throw DataMissing
    Ok(match width {
        0 => 0,
        1 => bytes[0] as u32,
        2 => u16::from_be_bytes([bytes[0], bytes[1]]) as u32,
        3 => u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]),
        4 => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        _ => return Err(PDFError::InvalidData("Invalid field width".into())),
    })
}
```

**Progressive Loading is Non-Negotiable:**

This rule exists to enable:
- **Fast first-page display**: Start rendering before full PDF download completes
- **Low memory usage**: Only load needed portions, critical for large PDFs
- **Network efficiency**: HTTP range requests only fetch required data
- **Responsive UI**: No blocking on full file load

If a feature cannot be implemented with progressive loading, it should be redesigned or deferred. **External crates are valuable for learning algorithms and patterns, but the core parsing infrastructure must support progressive loading.**

## Current Project Status

This is an **early-stage project**. No Rust implementation exists yet. When developing:

- Start with core parser infrastructure (streams, lexer, object parser)
- Design the Data Source abstraction layer first (trait for chunk-based loading)
- Implement filesystem loader before network loader (simpler, easier to test)
- Implement progressive loading from the beginning, not as an afterthought
- Reference PDF.js implementations before writing new components
- Follow Rust idioms (no direct translation of JavaScript patterns)

## Key PDF Concepts

- **Cross-reference (xref) table**: Index mapping object numbers to byte offsets in file
- **Incremental updates**: PDFs can have multiple xref sections (append-only updates)
- **Page tree**: Hierarchical structure storing pages (not always flat array)
- **Content streams**: Compressed instruction streams defining page graphics
- **Linearized PDF**: Reorganized PDF with hint tables for progressive display

## Reference Documentation

- PDF 1.7 Specification (ISO 32000-1): Official PDF format specification
- PDF.js source code in `pdf.js/` submodule: Working reference implementation
- [pdf-rs](https://github.com/pdf-rs/pdf): Existing Rust PDF library for patterns
