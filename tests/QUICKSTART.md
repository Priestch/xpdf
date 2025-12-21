# Test Suite Quick Start

## What's Been Created

A comprehensive test suite with ~240 test functions across 6 test files, based on PDF.js's testing methodology.

## Quick Verification

```bash
# Check tests compile
cargo test --no-run

# Run existing tests (should see 126 passed)
cargo test --lib

# Run test utils tests (3 should pass)
cargo test --test test_utils
```

## Test Files Overview

| File | Tests | Purpose | Status |
|------|-------|---------|--------|
| `test_utils.rs` | 3 | Common utilities | ✅ Working |
| `progressive_loading_tests.rs` | 25 | Progressive/chunked loading | 🔄 Skeletons |
| `parser_tests.rs` | 60 | Lexer and parser | 🔄 Skeletons |
| `stream_tests.rs` | 55 | Streams and filters | 🔄 Skeletons |
| `document_tests.rs` | 70 | Document structure | 🔄 Skeletons |
| `xref_tests.rs` | 30 | Cross-reference tables | 🔄 Skeletons |

## Test PDFs Available

8 test PDFs in `tests/fixtures/pdfs/`:
- `basicapi.pdf` (104KB) - Basic functionality
- `tracemonkey.pdf` (993KB) - Complex document
- `empty.pdf` (5KB) - Minimal document
- `rotation.pdf` (7KB) - Rotated pages
- `asciihexdecode.pdf` (743B) - Filter testing
- `simpletype3font.pdf` (2KB) - Font testing
- `TrueType_without_cmap.pdf` (4KB) - Font edge case
- `annotation-border-styles.pdf` (88KB) - Annotations

## Using Test Utilities

```rust
use mod test_utils;
use test_utils::*;

#[test]
fn my_test() {
    // Check if PDF exists
    assert!(test_pdf_exists("basicapi.pdf"));

    // Load PDF bytes
    let bytes = load_test_pdf_bytes("basicapi.pdf").unwrap();

    // Create file stream
    let stream = create_file_stream("basicapi.pdf").unwrap();

    // Load and parse document
    let doc = assert_pdf_loads("basicapi.pdf").unwrap();
}
```

## Development Workflow

1. **Implement a feature** (e.g., xref parsing)

2. **Find relevant tests** in test files:
   ```bash
   grep -r "test_xref" tests/
   ```

3. **Implement the test**:
   ```rust
   #[test]
   fn test_xref_basic() {
       let doc = assert_pdf_loads("basicapi.pdf").unwrap();
       // Add your test implementation
   }
   ```

4. **Run tests**:
   ```bash
   cargo test test_xref_basic
   ```

5. **Fix until tests pass**

6. **Repeat** for next feature

## Priority Order

Based on CLAUDE.md architecture principles:

1. **Progressive Loading Tests** ⭐ CRITICAL
   - Exception-driven loading pattern
   - Chunked data loading
   - Minimal loading validation

2. **Parser Tests**
   - Lexer (tokens, strings, numbers)
   - Basic objects
   - Complex objects

3. **Stream Tests**
   - File chunked streaming
   - Filters and decoders

4. **XRef Tests**
   - Table parsing
   - Progressive xref loading

5. **Document Tests**
   - Catalog and page tree
   - Metadata extraction

## Documentation

- **README.md**: Comprehensive test suite guide (320+ lines)
- **SUMMARY.md**: What was created and why (180+ lines)
- **STATUS.md**: Implementation status
- **This file**: Quick start guide

## Key Commands

```bash
# Build without running
cargo test --no-run

# Run all tests
cargo test

# Run specific test file
cargo test --test parser_tests

# Run specific test
cargo test test_xref_basic

# Run with output
cargo test -- --nocapture

# Run ignored tests (network, etc.)
cargo test -- --ignored

# Run in release (faster)
cargo test --release
```

## Example: Implementing a Test

Before (skeleton):
```rust
#[test]
fn test_parse_header() {
    // Parse PDF header: %PDF-1.x
    // Verify version is valid
}
```

After implementation:
```rust
#[test]
fn test_parse_header() {
    let bytes = load_test_pdf_bytes("basicapi.pdf").unwrap();

    // Check PDF magic bytes
    assert!(bytes.starts_with(b"%PDF-"));

    // Extract version
    let version = &bytes[5..8]; // e.g., "1.4"
    assert!(version[0] == b'1');
    assert!(version[1] == b'.');
    assert!(version[2] >= b'0' && version[2] <= b'9');
}
```

## Next Steps

1. Review `tests/progressive_loading_tests.rs` - these are CRITICAL
2. Implement progressive loading infrastructure with DataMissing errors
3. Implement corresponding tests as you build features
4. Run `cargo test` frequently during development
5. Add more test PDFs as needed for edge cases

## Getting Help

- See `tests/README.md` for comprehensive documentation
- Check PDF.js tests in `pdf.js/test/` for examples
- Review CLAUDE.md for architecture principles
- Look at test skeletons for what needs to be validated

## Current Status

✅ **Done**: Test infrastructure, test PDFs, ~240 test skeletons, documentation

🔄 **Next**: Implement tests as features are built, starting with progressive loading

---

**The test suite is ready!** Start implementing features and their corresponding tests.
