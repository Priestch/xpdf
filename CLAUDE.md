# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

PDF-X is a Rust port of Mozilla's PDF.js library with a hybrid viewer-editor architecture. The goal is to replicate PDF.js's proven architecture and progressive/lazy loading features while leveraging Rust's performance and memory safety, plus add editing capabilities through a delta-based modification layer.

**Key Design Principles**:
- Maintain architectural fidelity to PDF.js while implementing idiomatic Rust
- **Hybrid Architecture**: Immutable progressive viewer + mutable delta layer for editing
- Preserve the viewer's performance (lazy loading, low memory) while enabling document modifications

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

The implementation follows a **five-layer hybrid architecture** that combines an efficient viewer with a lightweight editing layer:

### Viewer Core (Immutable, Progressive)

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
   - **Immutable by design**: Never modifies the base PDF

3. **Document Structure Layer**: Page tree and metadata management
   - Build page tree without loading all pages
   - Catalog and metadata extraction
   - Resource dictionaries
   - Lazy page object resolution

4. **Rendering Layer**: Content stream interpretation
   - On-demand page rendering
   - Graphics state management
   - Text and image extraction

### Editor Layer (Mutable, Delta-Based)

5. **Delta Layer**: Document modification tracking
   - **Modified objects**: Overrides for specific PDF objects (by object number)
   - **New objects**: Annotations, added pages, form field values
   - **Deletion markers**: Pages or objects marked for removal
   - **Command history**: Stack of reversible operations for undo/redo
   - **Incremental serialization**: Writes changes as PDF incremental updates

**Key Insight**: The viewer layers (1-4) remain completely unchanged and read-only. The delta layer (5) sits on top, intercepting object requests and applying modifications before rendering.

```
User/Editor Interface
        ↓
   Delta Layer (modifications, command history)
        ↓ merges with
   Rendering Engine (applies delta to base objects)
        ↓
   Document/Parser Layers (immutable base PDF)
        ↓
   Data Source Layer (progressive, chunked loading)
```

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

## **CRITICAL RULE: Always Check References Before Implementing**

This is a **NON-NEGOTIABLE** development practice that MUST be followed for ALL features:

**Before implementing ANY feature, algorithm, or conversion, you MUST:**

1. **Search PDF.js source code** (`pdf.js/src/`) for the corresponding implementation
2. **Check hayro libraries** (`../hayro/`) for existing implementations
3. **Copy the exact algorithm** from the reference, adapting it to Rust
4. **Add code comments** with the reference file path

**Why This Matters:**

When implementing features without consulting references, you introduce bugs that could have been avoided. For example:

❌ **WRONG**: Implementing CMYK to RGB conversion using a naive formula
```rust
// Naive, incorrect implementation
let c = 255.0 - chunk[0] as f32;
let r = (c * k / 255.0) as u8;  // Produces wrong colors
```

✅ **CORRECT**: Using PDF.js's proven polynomial coefficients
```rust
// Reference: pdf.js/src/core/colorspace.js - DeviceCmykCS.#toRgb
// Uses polynomial coefficients derived from US Web Coated (SWOP) colorspace
let r = 255.0 +
    c * (-4.387332384609988 * c + 54.48615194189176 * m + ...) +
    m * (1.7149763477362134 * m - 5.6096736904047315 * y + ...) +
    y * (-2.5217340131683033 * y - 21.248923337353073 * k + ...) +
    k * (-21.86122147463605 * k - 189.48180835922747);
```

**Reference Checking Workflow:**

```bash
# 1. Search PDF.js for the feature
grep -r "CMYK\|cmyk" ../pdf.js/src/core/

# 2. Find the specific implementation
#    - ../pdf.js/src/core/colorspace.js for color conversions
#    - ../pdf.js/src/core/image.js for image handling
#    - ../pdf.js/src/core/evaluator.js for content stream operators

# 3. Check hayro libraries
ls ../hayro/hayro-*/src/
```

**What to Reference:**

| Feature | PDF.js Location | Hayro Library |
|---------|-----------------|---------------|
| Color spaces (CMYK, Lab, etc.) | `core/colorspace.js` | N/A |
| Image decoding | `core/image.js`, `core/jpg.js` | N/A |
| Content stream operators | `core/evaluator.js` | N/A |
| Font handling | `core/font.js`, `core/cff_parser.js` | `hayro-font/` |
| JPEG2000 decoding | N/A | `hayro-jpeg2000/` |
| JBIG2 decoding | N/A | `hayro-jbig2/` |

**Consequences of Not Checking References:**

- **Incorrect implementations** that produce wrong results
- **Wasted time** debugging issues already solved in PDF.js
- **Incompatibility** with PDF files that work in PDF.js
- **Technical debt** that must be fixed later

**This rule applies to:**
- All color space conversions (CMYK, Lab, ICC-based, etc.)
- All image format handling (JPEG, PNG, JPEG2000, JBIG2, etc.)
- All content stream operators (text, paths, images, etc.)
- All font parsing (Type1, TrueType, CFF, etc.)
- All encryption algorithms (RC4, AES-128, AES-256, etc.)
- All compression algorithms (Flate, ASCIIHex, ASCII85, etc.)

**Remember**: PDF.js has 10+ years of bug fixes and real-world testing. Their implementations are proven to work with thousands of PDF files. Always leverage this knowledge.

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

## Delta Layer Architecture

The **Delta Layer** is PDF-X's innovation that enables editing capabilities without sacrificing the viewer's performance. It implements a **command pattern** with **incremental updates** to track and persist document changes.

### Core Design Principles

1. **Immutable Base, Mutable Delta**: The base PDF loaded by the viewer is never modified. All changes are tracked in a separate delta structure.
2. **Override Resolution**: When requesting an object, check delta first (modifications override base), then fall back to viewer.
3. **Command Pattern**: All edits are executed as reversible commands, enabling natural undo/redo.
4. **Incremental Persistence**: Changes are saved as PDF incremental updates (appended to original file, per PDF specification).

### Delta Layer Structure

```rust
pub struct DeltaLayer {
    // Object modifications: object_number -> replacement object
    modified_objects: HashMap<ObjectId, PDFObject>,

    // New objects added to the document (annotations, pages, etc.)
    new_objects: Vec<PDFObject>,

    // Objects/pages marked for deletion
    deleted_objects: HashSet<ObjectId>,

    // Command history for undo/redo
    command_history: Vec<Box<dyn Command>>,
    undo_stack: Vec<Box<dyn Command>>,
}

// All edits are reversible commands
pub trait Command {
    fn execute(&mut self, delta: &mut DeltaLayer) -> Result<(), PDFError>;
    fn undo(&mut self, delta: &mut DeltaLayer) -> Result<(), PDFError>;
    fn redo(&mut self, delta: &mut DeltaLayer) -> Result<(), PDFError>;
}
```

### Object Resolution with Deltas

The rendering engine queries objects through a resolution function that merges base and delta:

```rust
fn resolve_object(object_number: ObjectId) -> Result<PDFObject, PDFError> {
    // 1. Check delta layer first (modifications override base)
    if let Some(obj) = delta_layer.get_modified(object_number) {
        return Ok(obj.clone());
    }

    // 2. Check if deleted
    if delta_layer.is_deleted(object_number) {
        return Err(PDFError::ObjectDeleted(object_number));
    }

    // 3. Fall back to base PDF (progressive loading)
    viewer.get_object(object_number)  // Existing viewer code
}
```

### Supported Edit Operations

The delta layer enables these common PDF operations:

**Easy (Low-Hanging Fruit)**:
- ✅ **Annotations**: Highlight, text markup, sticky notes, freehand drawing
- ✅ **Page operations**: Rotation, deletion, reordering (small delta)
- ✅ **Form filling**: Store field values, checkboxes, signatures
- ✅ **Document merging**: Copy page objects + update catalog
- ✅ **Metadata editing**: Modify document info, properties

**Moderate Complexity**:
- ⚠️ **Text overlay**: Add new text on top of existing content (easy)
- ⚠️ **Image replacement**: Swap content object references
- ⚠️ **Redaction**: Overlay black rectangles + delete underlying text objects
- ⚠️ **Content stream modification**: Edit page graphics operations

**Hard (Significant Challenge)**:
- ❌ **True text editing**: Reposition characters, reflow paragraphs
  - PDF stores character positions absolutely, not as flowing text
  - Requires implementing a text layout engine
  - Many commercial PDF editors avoid this or use crude workarounds

### Saving Changes

The delta layer supports two save strategies:

**1. Incremental Update (Recommended)**
```rust
fn save_incremental() -> Result<(), PDFError> {
    // Append changes to end of original PDF
    // - New xref table
    // - Modified/new objects
    // - Original data untouched
    pdf_writer.append_incremental_update(&delta_layer)?;
}
```
- ✅ Fast (only writes delta)
- ✅ Preserves original file
- ✅ PDF specification compliant
- ❌ File grows with each save

**2. Full Rewrite (Compact)**
```rust
fn save_compact() -> Result<(), PDFError> {
    // Rewrite entire PDF with changes applied
    // - Removes deleted objects
    // - Compacts file size
    // - Single xref table
    pdf_writer.rewrite_pdf(&viewer, &delta_layer)?;
}
```
- ✅ Produces clean, compact file
- ❌ Slower (rewrites everything)
- ❌ Loses incremental history

### Example: Adding an Annotation

```rust
// User adds a highlight annotation on page 5
let command = AddAnnotationCommand {
    page_number: 5,
    annotation: Annotation::highlight(100, 200, 300, 50, Color::Yellow),
};

// Execute command
command.execute(&mut delta_layer)?;

// Delta layer now contains:
// 1. New annotation object (e.g., 123 0 obj)
// 2. Modified page object: /Annots [ ... 123 0 R ]
// 3. Command in history for undo

// When rendering page 5:
let base_page = viewer.load_page(5)?;  // Progressive load
let modified_page = delta_layer.apply_to_page(base_page)?;
renderer.render(modified_page);

// User can undo
command.undo(&mut delta_layer)?;
```

### Advantages Over Traditional Editor

| Aspect | Traditional Editor | PDF-X Hybrid |
|--------|-------------------|--------------|
| **Startup time** | Slow (parse all) | Fast (progressive) |
| **Memory usage** | High (all objects) | Low (viewed pages + delta) |
| **Save time** | Slow (serialize all) | Fast (append delta) |
| **Undo/redo** | Complex object tracking | Natural command pattern |
| **Progressive loading** | ❌ Not possible | ✅ Maintained |
| **Network efficiency** | ❌ Full download | ✅ Range requests |

### Collaboration Potential

The delta layer architecture naturally supports collaborative editing:

- Deltas can be serialized independently from base PDF
- Multiple users' changes can be merged (like Git branches)
- Conflict resolution for overlapping edits
- Operational transformation or CRDT patterns applicable

### Implementation Roadmap

**Phase 1: Delta Foundation**
- Core `DeltaLayer` struct with modification tracking
- Object resolution with override logic
- Basic serialization to incremental PDF format

**Phase 2: Command Infrastructure**
- Command trait and common commands
- Undo/redo stacks
- Command history serialization

**Phase 3: Editing Operations**
- Annotation commands (highlight, note, drawing)
- Page manipulation (rotate, delete, reorder)
- Form field value storage

**Phase 4: Advanced Operations**
- Content stream modification
- Image replacement
- Redaction tools

**Phase 5: Collaboration** (Future)
- Delta serialization format
- Merge algorithms
- Conflict resolution



## Current Project Status

The project has **core viewer infrastructure** implemented with progressive loading support. Current capabilities include:

**Implemented**:
- ✅ Core PDF parsing (document structure, xref, objects)
- ✅ Encryption/decryption support
- ✅ Basic annotation parsing
- ✅ Outline (bookmark) extraction with page indices
- ✅ Inspector GUI for document exploration
- ✅ Tauri-based desktop application framework

**In Development**:
- 🔄 Complete progressive/lazy loading (exception-driven data fetching)
- 🔄 Content stream parsing and rendering
- 🔄 Text extraction and search
- 🔄 Delta layer foundation (next major milestone)

**Priorities**:

1. **Complete Viewer Core**: Finish progressive rendering pipeline
   - Content stream interpretation (graphics operators)
   - Font and image handling
   - Text extraction for search

2. **Delta Layer Foundation**: Begin hybrid architecture implementation
   - Core `DeltaLayer` struct with modification tracking
   - Object resolution with override logic
   - Basic incremental PDF writer
   - Command pattern infrastructure

3. **Initial Editing Features**: Low-hanging fruit for delta layer
   - Annotation rendering and creation
   - Page rotation
   - Form field display and filling

**Development Guidelines**:
- Reference PDF.js implementations before writing new components
- Follow Rust idioms (no direct translation of JavaScript patterns)
- Maintain strict separation: viewer core remains immutable, all mutations through delta layer
- Test progressive loading with network resources (not just local files)
- Incremental updates should be the default save mechanism

## Key PDF Concepts

- **Cross-reference (xref) table**: Index mapping object numbers to byte offsets in file
- **Incremental updates**: PDFs can have multiple xref sections (append-only updates)
  - **Critical for delta layer**: Changes can be appended to original file without rewriting it
  - New xref table points to modified objects and original unchanged objects
  - Enables fast saves and preserves original document data
- **Page tree**: Hierarchical structure storing pages (not always flat array)
- **Content streams**: Compressed instruction streams defining page graphics
- **Linearized PDF**: Reorganized PDF with hint tables for progressive display
- **Object graph**: PDF objects can reference each other (direct and indirect references)
  - Delta layer must track and update these references when objects are modified

## Reference Documentation

- PDF 1.7 Specification (ISO 32000-1): Official PDF format specification
  - **Section 7.5.6**: Incremental Updates (crucial for delta layer persistence)
- PDF.js source code in `pdf.js/` submodule: Working reference implementation
- [pdf-rs](https://github.com/pdf-rs/pdf): Existing Rust PDF library for patterns

