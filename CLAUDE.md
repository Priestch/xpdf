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
