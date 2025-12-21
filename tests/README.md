# PDF-X Test Suite

This directory contains the comprehensive test suite for PDF-X, based on PDF.js's proven testing approach.

## Overview

The test suite validates the core functionality of PDF-X, with special emphasis on **progressive loading** - the key architectural feature that allows displaying PDF content before the entire file is loaded.

## Test Structure

```
tests/
├── fixtures/
│   ├── pdfs/               # Test PDF files
│   ├── test_manifest.json  # Test registry with metadata
│   └── copy_test_pdfs.sh   # Script to copy PDFs from PDF.js
├── test_utils.rs           # Common test utilities and helpers
├── progressive_loading_tests.rs  # Progressive/chunked loading tests
├── parser_tests.rs         # Parser and lexer tests
├── stream_tests.rs         # Stream and data source tests
├── document_tests.rs       # Document structure tests
└── xref_tests.rs          # Cross-reference table tests
```

## Running Tests

```bash
# Run all tests
cargo test

# Run specific test file
cargo test --test progressive_loading_tests
cargo test --test parser_tests
cargo test --test stream_tests
cargo test --test document_tests
cargo test --test xref_tests

# Run tests matching a pattern
cargo test xref
cargo test progressive

# Run ignored tests (e.g., network tests)
cargo test -- --ignored

# Run with output
cargo test -- --nocapture

# Run in release mode (faster)
cargo test --release
```

## Test Categories

### 1. Progressive Loading Tests (`progressive_loading_tests.rs`)

**Priority: CRITICAL** - These validate the core architectural principle of PDF-X.

Tests include:
- Exception-driven loading pattern (DataMissing errors)
- Chunked data loading from different sources
- Minimal data loading for metadata extraction
- Lazy page loading (content loaded only when needed)
- Chunk boundary handling
- Sequential and random access patterns
- Memory efficiency validation

**Key Principle**: Load as little PDF data as possible. Operations should attempt to proceed with available data and raise `DataMissing` exceptions when data is not available. The caller catches these exceptions, loads the required chunks, and retries.

### 2. Parser Tests (`parser_tests.rs`)

Based on PDF.js's `parser_spec.js` and `primitives_spec.js`.

Tests include:
- **Lexer**: Numbers, strings, names, keywords, comments, operators
- **Parser**: Basic objects (boolean, integer, real, string, name, array, dictionary, null)
- **Complex objects**: Indirect references, indirect objects, stream objects
- **Error handling**: Malformed input, truncated data
- **Real documents**: Headers, trailers, catalogs, page trees
- **Edge cases**: String escapes, hex strings, name encoding, etc.

### 3. Stream Tests (`stream_tests.rs`)

Based on PDF.js's `stream_spec.js`, `fetch_stream_spec.js`, and `node_stream_spec.js`.

Tests include:
- **Base streams**: Creation, reading, seeking, EOF handling
- **FileChunkedStream**: Local file access with chunked loading
- **HttpChunkedStream**: HTTP range requests (marked `#[ignore]` - requires test server)
- **SubStreams**: Views into portions of parent streams
- **Filters**: FlateDecode, ASCIIHexDecode, ASCII85, LZW, etc.
- **Predictors**: TIFF and PNG predictor algorithms
- **Error handling**: Corrupted data, invalid filters, length mismatches
- **ChunkManager**: Chunk tracking, overlapping ranges, memory limits

### 4. Document Structure Tests (`document_tests.rs`)

Based on PDF.js's `document_spec.js` and `api_spec.js`.

Tests include:
- **Document loading**: Basic, empty, complex PDFs
- **Headers**: PDF version detection
- **Cross-reference tables**: Parsing, lookups, free entries
- **Trailers**: Size, Root, Info, ID, Encrypt
- **Catalogs**: Pages reference, PageMode, PageLayout, Metadata
- **Page trees**: Count, structure, hierarchy, inherited attributes
- **Pages**: MediaBox, CropBox, Rotation, Resources, Contents, Annotations
- **Resources**: Fonts, XObjects, ColorSpaces, ExtGState, etc.
- **Metadata**: Info dictionary, XMP metadata
- **Error recovery**: Missing/corrupted structures

### 5. Cross-Reference Tests (`xref_tests.rs`)

Dedicated tests for cross-reference table parsing.

Tests include:
- **Traditional tables**: Format, subsections, free/in-use entries, generation numbers
- **XRef streams**: PDF 1.5+ compressed xref format (when test PDFs available)
- **Incremental updates**: Multiple xref sections, Prev pointers
- **Reconstruction**: Rebuilding corrupted or missing xref by scanning
- **Progressive loading**: Loading xref without full file
- **Lookups**: Finding objects, handling missing objects
- **Error handling**: Malformed entries, invalid offsets, truncation

## Test PDFs

Test PDFs are sourced from PDF.js's comprehensive test collection. We've selected a focused set covering essential features:

### Current Test PDFs

| PDF | Size | Description | Purpose |
|-----|------|-------------|---------|
| `basicapi.pdf` | 104KB | Basic PDF with 3 pages | Core functionality, API testing |
| `tracemonkey.pdf` | 993KB | Complex 14-page document | Fonts, images, complex structure |
| `empty.pdf` | 5KB | Empty PDF (no pages) | Edge case: minimal document |
| `rotation.pdf` | 7KB | 4 pages with rotation | Page rotation handling |
| `asciihexdecode.pdf` | 743B | ASCIIHex encoded stream | Filter testing |
| `simpletype3font.pdf` | 2KB | Type 3 font | Font parsing |
| `TrueType_without_cmap.pdf` | 4KB | TrueType font edge case | Font edge cases |
| `annotation-border-styles.pdf` | 88KB | Annotations | Annotation parsing |

### Needed Test PDFs (TODOs)

These test PDFs are referenced in the test manifest but need to be created or found:

- `xref-stream.pdf` - PDF 1.5+ with xref stream format
- `linearized.pdf` - Linearized PDF for fast web view
- `compressed-object-stream.pdf` - Object streams
- `flatedecode.pdf` - FlateDecode compression
- `bad-xref.pdf` - Malformed xref for error recovery testing
- `issue3115.pdf` - PDF with multiple xref sections

## Test Utilities (`test_utils.rs`)

Common helpers for all tests:

```rust
// Path helpers
fixtures_dir() -> PathBuf
pdfs_dir() -> PathBuf
get_test_pdf_path(name: &str) -> PathBuf
test_pdf_exists(name: &str) -> bool

// Loading helpers
load_test_pdf_bytes(name: &str) -> Result<Vec<u8>>
create_file_stream(name: &str) -> Result<FileChunkedStream>
assert_pdf_loads(name: &str) -> Result<PDFDocument>

// Test structures
ByteStream - In-memory stream for unit tests
XRefMock - Mock cross-reference table
TestManifestEntry - Test manifest structure
```

## Writing New Tests

### Guidelines

1. **Test Progressive Loading First**: When adding features, ensure they work with progressive loading
2. **Use Test Utilities**: Don't duplicate path/loading logic
3. **Test Real PDFs**: Unit tests are good, but also test with actual PDF files
4. **Test Error Cases**: Don't just test happy paths
5. **Document Edge Cases**: Add comments explaining tricky test cases
6. **Reference PDF.js**: When in doubt, see how PDF.js tests it

### Example Test

```rust
#[test]
fn test_my_new_feature() {
    // Load a test PDF
    let doc = assert_pdf_loads("basicapi.pdf")
        .expect("Failed to load PDF");

    // Test your feature
    let result = doc.my_new_feature();

    // Verify behavior
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), expected_value);
}
```

### Testing Progressive Loading

```rust
#[test]
fn test_feature_with_progressive_loading() {
    let mut stream = create_file_stream("basicapi.pdf")
        .expect("Failed to create stream");

    // Try operation that may need more data
    loop {
        match parser.parse_something(&mut stream) {
            Ok(result) => {
                // Success - verify result
                assert_eq!(result, expected);
                break;
            }
            Err(PDFError::DataMissing { position, length }) => {
                // Load the missing chunk
                stream.ensure_range(position, length)?;
                // Loop will retry
            }
            Err(e) => {
                panic!("Unexpected error: {:?}", e);
            }
        }
    }
}
```

## Test Status

Current status of test files:

- ✅ **Test infrastructure**: Complete (manifest, fixtures, utilities)
- ✅ **Test PDFs**: Basic set copied from PDF.js (8 PDFs)
- 🚧 **Progressive loading tests**: Test skeletons created, need implementation as features develop
- 🚧 **Parser tests**: Test skeletons created, need implementation
- 🚧 **Stream tests**: Test skeletons created, need implementation
- 🚧 **Document tests**: Test skeletons created, need implementation
- 🚧 **XRef tests**: Test skeletons created, need implementation

**Note**: Many tests are currently skeletons that will be implemented as the corresponding features are built. Tests marked `#[ignore]` require additional setup (network, specific PDFs, etc.).

## Continuous Testing Strategy

As PDF-X develops:

1. **Start with unit tests**: Test individual components in isolation
2. **Add integration tests**: Test components working together
3. **Expand PDF collection**: Add more test PDFs as needed for edge cases
4. **Run tests frequently**: Use `cargo test` during development
5. **Track coverage**: Use `cargo tidy` or similar to track test coverage
6. **Learn from failures**: When bugs are found, add regression tests

## Comparison with PDF.js

PDF.js has accumulated **1,217 regression tests** over many years. This test suite starts with a focused set covering core functionality:

- **~180 test functions** across 5 test files
- **8 core test PDFs** (vs 784 in PDF.js)
- **Focus on architecture**: Progressive loading is critical
- **Room to grow**: Can add more tests as features are implemented

The goal is not to replicate all 1,217 tests immediately, but to:
1. Validate core progressive loading architecture
2. Ensure parser correctness
3. Test error handling
4. Expand based on discovered bugs and new features

## References

- PDF.js test suite: `pdf.js/test/`
- PDF.js test manifest: `pdf.js/test/test_manifest.json`
- PDF 1.7 Specification: Reference for expected behavior
- CLAUDE.md: Architecture guidelines, especially progressive loading
