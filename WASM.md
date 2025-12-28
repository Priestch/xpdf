# WASM Build Guide for PDF-X

This guide shows how to compile PDF-X to WebAssembly for use in browsers.

## Prerequisites

```bash
# Install wasm32 target
rustup target add wasm32-unknown-unknown

# Install wasm-bindgen (for JS interop)
cargo install wasm-bindgen-cli

# Install wasm-opt (optional, for size optimization)
cargo install wasm-opt
```

## Building for WASM

### Basic WASM Build

```bash
# Build with size optimization
cargo build --release --target wasm32-unknown-unknown --profile release-wasm

# Output: target/wasm32-unknown-unknown/release-wasm/pdf_x.wasm
```

### With wasm-bindgen (Recommended)

Create `src/wasm.rs`:

```rust
use wasm_bindgen::prelude::*;
use crate::PDFDocument;

#[wasm_bindgen]
pub struct WasmPDFDocument {
    inner: PDFDocument,
}

#[wasm_bindgen]
impl WasmPDFDocument {
    #[wasm_bindgen(constructor)]
    pub fn new(data: Vec<u8>) -> Result<WasmPDFDocument, JsValue> {
        let doc = PDFDocument::open(data)
            .map_err(|e| JsValue::from_str(&format!("{:?}", e)))?;

        Ok(WasmPDFDocument { inner: doc })
    }

    #[wasm_bindgen]
    pub fn page_count(&self) -> Result<usize, JsValue> {
        self.inner.page_count()
            .map_err(|e| JsValue::from_str(&format!("{:?}", e)))
    }

    #[wasm_bindgen]
    pub fn extract_text(&mut self, page_index: usize) -> Result<String, JsValue> {
        let page = self.inner.get_page(page_index)
            .map_err(|e| JsValue::from_str(&format!("{:?}", e)))?;

        page.extract_text_as_string(self.inner.xref_mut())
            .map_err(|e| JsValue::from_str(&format!("{:?}", e)))
    }
}
```

Add to `Cargo.toml`:

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
wasm-bindgen = "0.2"
```

Build and generate JS bindings:

```bash
# Build
cargo build --release --target wasm32-unknown-unknown --profile release-wasm

# Generate JS bindings
wasm-bindgen target/wasm32-unknown-unknown/release-wasm/pdf_x.wasm \
    --out-dir pkg \
    --target web
```

### Size Optimization

```bash
# Further optimize with wasm-opt
wasm-opt -Oz pkg/pdf_x_bg.wasm -o pkg/pdf_x_bg_opt.wasm

# Check size
ls -lh pkg/*.wasm
```

**Expected sizes:**
- Without optimization: ~2-3 MB
- With `opt-level = "z"`: ~800 KB
- With wasm-opt -Oz: ~500 KB
- Gzipped: ~150-200 KB

## JavaScript Usage

```html
<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>PDF-X WASM Demo</title>
</head>
<body>
    <input type="file" id="pdfFile" accept=".pdf">
    <div id="output"></div>

    <script type="module">
        import init, { WasmPDFDocument } from './pkg/pdf_x.js';

        async function loadPDF() {
            // Initialize WASM module
            await init();

            // Load PDF file
            const input = document.getElementById('pdfFile');
            input.addEventListener('change', async (e) => {
                const file = e.target.files[0];
                const arrayBuffer = await file.arrayBuffer();
                const data = new Uint8Array(arrayBuffer);

                try {
                    // Parse PDF
                    const doc = new WasmPDFDocument(data);
                    const pageCount = doc.page_count();

                    console.log(`PDF has ${pageCount} pages`);

                    // Extract text from first page
                    const text = doc.extract_text(0);
                    document.getElementById('output').innerText = text;
                } catch (e) {
                    console.error('Error:', e);
                }
            });
        }

        loadPDF();
    </script>
</body>
</html>
```

## Performance Tips for WASM

### 1. Minimize Allocations

WASM allocator is slower than native:

```rust
// BAD: Many small allocations
let mut parts = Vec::new();
for part in data.split(',') {
    parts.push(part.to_string());  // Allocation per item
}

// GOOD: Single allocation or zero-copy
let parts: Vec<&str> = data.split(',').collect();
```

### 2. Use TypedArrays for Data Transfer

```rust
#[wasm_bindgen]
pub fn process_data(data: &[u8]) -> Vec<u8> {
    // Process without copying
    data.iter().map(|&b| b.wrapping_add(1)).collect()
}
```

```javascript
// Efficient data transfer (zero-copy view)
const data = new Uint8Array(buffer);
const result = wasmModule.process_data(data);
```

### 3. Batch Operations

```rust
// BAD: Many WASM↔JS calls
for i in 0..1000 {
    wasm.process_single(i);  // 1000 boundary crossings
}

// GOOD: Single call
wasm.process_batch(0, 1000);  // 1 boundary crossing
```

### 4. Use wee_alloc for Smaller Binary

Add to `Cargo.toml`:

```toml
[dependencies]
wee_alloc = "0.4"
```

In `src/wasm.rs`:

```rust
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
```

Saves ~10KB in binary size.

## Testing WASM Locally

```bash
# Install a simple HTTP server
cargo install basic-http-server

# Serve the directory
basic-http-server .

# Open http://localhost:4000 in browser
```

## CI/CD for WASM

GitHub Actions example:

```yaml
name: WASM Build

on: [push]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          target: wasm32-unknown-unknown

      - name: Build WASM
        run: |
          cargo build --release --target wasm32-unknown-unknown --profile release-wasm

      - name: Install wasm-bindgen
        run: cargo install wasm-bindgen-cli

      - name: Generate bindings
        run: |
          wasm-bindgen target/wasm32-unknown-unknown/release-wasm/pdf_x.wasm \
            --out-dir pkg --target web

      - name: Upload artifact
        uses: actions/upload-artifact@v2
        with:
          name: wasm-package
          path: pkg/
```

## Browser Compatibility

PDF-X WASM works in all modern browsers:

- ✅ Chrome/Edge 57+
- ✅ Firefox 52+
- ✅ Safari 11+
- ✅ Opera 44+

**Requirements:**
- WebAssembly support
- ES6 modules (for wasm-bindgen)

## Memory Considerations

WASM has a limited memory space (default 16MB, max 4GB):

```rust
// Monitor memory usage in WASM
#[wasm_bindgen]
pub fn get_memory_usage() -> usize {
    // Return approximate memory usage
    // Useful for debugging
}
```

**Tips:**
- Use streaming for large PDFs
- Implement chunk eviction
- Clear caches aggressively
- Use LRU for object cache

## Debugging WASM

### Enable Debug Info

```toml
[profile.release-wasm]
debug = true  # Include debug symbols
```

### Console Logging

```rust
use web_sys::console;

#[wasm_bindgen]
pub fn debug_log(msg: &str) {
    console::log_1(&msg.into());
}
```

### Browser DevTools

```javascript
// Chrome DevTools → Sources → WASM modules
// Set breakpoints in WASM code
// Inspect memory and call stack
```

## Production Deployment

### 1. Serve with Correct MIME Type

```nginx
# nginx.conf
types {
    application/wasm wasm;
}
```

### 2. Enable Compression

```nginx
gzip on;
gzip_types application/wasm;
```

### 3. Cache Headers

```nginx
location ~* \.wasm$ {
    add_header Cache-Control "public, max-age=31536000";
}
```

### 4. CDN Deployment

Upload to CDN with:
- `.wasm` file (binary)
- `.js` file (loader)
- Gzip compression enabled

## Troubleshooting

### Binary Too Large

- Check `opt-level = "z"`
- Run wasm-opt -Oz
- Remove unused features
- Use `strip = true`

### Memory Errors

- Increase WASM memory limit
- Implement chunk eviction
- Use streaming APIs

### Performance Issues

- Profile with browser DevTools
- Reduce JS↔WASM calls
- Batch operations
- Use TypedArrays

## Resources

- [wasm-bindgen documentation](https://rustwasm.github.io/wasm-bindgen/)
- [Rust WASM book](https://rustwasm.github.io/book/)
- [MDN WebAssembly](https://developer.mozilla.org/en-US/docs/WebAssembly)
