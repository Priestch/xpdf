# Async HTTP Loading API Documentation

## Overview

PDF-X provides comprehensive HTTP loading capabilities for loading PDFs from URLs using HTTP range requests. This enables progressive loading over the network, allowing you to work with PDFs without downloading the entire file first.

## Features

- **Async/Await Support** - Full async implementation with Tokio
- **HTTP Range Requests** - Efficient chunked loading using HTTP Range headers
- **Progress Callbacks** - Track download progress in real-time
- **LRU Caching** - Automatic chunk caching to minimize redundant requests
- **Synchronous API** - Blocking wrapper for non-async contexts

## Feature Flag

All async HTTP functionality requires the `async` feature:

```toml
[dependencies]
pdf-x = { version = "0.1.0", features = ["async"] }
```

## API Reference

### `AsyncHttpChunkedStream`

The primary async HTTP loading API.

#### Constructor

```rust
pub async fn open(
    url: impl Into<String>,
    chunk_size: Option<usize>,
    max_cached_chunks: Option<usize>,
    progress_callback: Option<ProgressCallback>,
) -> PDFResult<Self>
```

**Parameters:**
- `url` - The URL of the PDF file
- `chunk_size` - Size of each chunk in bytes (default: 65536 / 64KB)
- `max_cached_chunks` - Maximum number of chunks to keep in LRU cache (default: 20)
- `progress_callback` - Optional callback for download progress

**Returns:** `PDFResult<AsyncHttpChunkedStream>`

**Errors:**
- `PDFError::StreamError` - Server doesn't support range requests or invalid response
- `PDFError::IOError` - Network connection failure

#### Example

```rust
use pdf_x::core::AsyncHttpChunkedStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Simple usage
    let stream = AsyncHttpChunkedStream::open(
        "https://example.com/document.pdf",
        None,  // Use default chunk size (64KB)
        None,  // Use default cache size (20 chunks)
        None,  // No progress callback
    ).await?;

    println!("PDF size: {} bytes", stream.length());
    Ok(())
}
```

#### With Progress Callback

```rust
use pdf_x::core::AsyncHttpChunkedStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create progress callback
    let progress = Box::new(|loaded: usize, total: usize| {
        let percent = if total > 0 {
            (loaded * 100) / total
        } else {
            0
        };
        print!("\rProgress: {}% ({}/{} bytes)", percent, loaded, total);
        use std::io::Write;
        std::io::stdout().flush().unwrap();
    });

    // Load with progress tracking
    let stream = AsyncHttpChunkedStream::open(
        "https://example.com/document.pdf",
        Some(65536),   // 64KB chunks
        Some(10),      // Cache 10 chunks (640KB memory)
        Some(progress),
    ).await?;

    println!("\n✓ PDF loaded successfully!");
    println!("  File size: {} bytes", stream.length());
    println!("  Total chunks: {}", stream.num_chunks());
    println!("  Chunks loaded: {}", stream.num_chunks_loaded().await);

    Ok(())
}
```

### `HttpChunkedStream`

Synchronous wrapper around `AsyncHttpChunkedStream` for non-async contexts.

#### Constructor

```rust
pub fn open(
    url: impl Into<String>,
    chunk_size: Option<usize>,
    max_cached_chunks: Option<usize>,
) -> PDFResult<Self>
```

**Parameters:**
- `url` - The URL of the PDF file
- `chunk_size` - Size of each chunk in bytes (default: 65536 / 64KB)
- `max_cached_chunks` - Maximum number of chunks to keep in LRU cache (default: 20)

**Returns:** `PDFResult<HttpChunkedStream>`

**Note:** Progress callbacks are not supported in the synchronous API.

#### Example

```rust
use pdf_x::core::HttpChunkedStream;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Synchronous HTTP loading
    let stream = HttpChunkedStream::open(
        "https://example.com/document.pdf",
        Some(65536),  // 64KB chunks
        Some(10),     // Cache 10 chunks
    )?;

    println!("PDF size: {} bytes", stream.length());
    Ok(())
}
```

### `ProgressCallback`

Type alias for progress callback functions.

```rust
pub type ProgressCallback = Box<dyn Fn(usize, usize) + Send + Sync>;
```

**Parameters:**
- First `usize` - Bytes loaded so far
- Second `usize` - Total file size in bytes

**Requirements:**
- Must be `Send + Sync` for thread safety
- Should be non-blocking (avoid heavy computation)

## BaseStream Implementation

Both `AsyncHttpChunkedStream` and `HttpChunkedStream` implement the `BaseStream` trait, providing these methods:

### Core Methods

```rust
// Async version
pub async fn get_byte(&mut self) -> PDFResult<u8>
pub async fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>>
pub async fn peek_byte(&mut self) -> PDFResult<u8>
pub async fn peek_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>>
pub async fn skip(&mut self, n: usize) -> PDFResult<()>
pub async fn set_pos(&mut self, pos: usize) -> PDFResult<()>

// Synchronous version (same names, no async)
pub fn get_byte(&mut self) -> PDFResult<u8>
pub fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>>
// ... etc
```

### Informational Methods

```rust
pub fn pos(&self) -> usize
pub fn length(&self) -> usize
pub fn is_eof(&self) -> bool
pub fn num_chunks(&self) -> usize
pub async fn num_chunks_loaded(&self) -> usize  // Async only
```

## Usage Patterns

### Pattern 1: Direct Stream Reading

```rust
use pdf_x::core::AsyncHttpChunkedStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = AsyncHttpChunkedStream::open(
        "https://example.com/document.pdf",
        None, None, None,
    ).await?;

    // Read PDF header
    let header = stream.get_bytes(8).await?;
    println!("Header: {:?}", String::from_utf8_lossy(&header));

    // Check version
    if header.starts_with(b"%PDF-1.") {
        println!("Valid PDF file");
    }

    Ok(())
}
```

### Pattern 2: Integration with PDFDocument

```rust
use pdf_x::core::{AsyncHttpChunkedStream, PDFDocument};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load stream from URL
    let stream = AsyncHttpChunkedStream::open(
        "https://example.com/document.pdf",
        None, None, None,
    ).await?;

    // Convert to bytes (for PDFDocument::open)
    // Note: This loads the entire PDF into memory
    let mut stream = stream;
    let total = stream.length();
    let data = stream.get_bytes(total).await?;

    // Parse PDF
    let mut doc = PDFDocument::open(data)?;
    let page_count = doc.page_count()?;
    println!("PDF has {} pages", page_count);

    Ok(())
}
```

**Note:** For true progressive loading with PDFDocument, you would use:

```rust
// Future API (not yet implemented)
let doc = PDFDocument::open_stream(Box::new(stream)).await?;
```

### Pattern 3: Chunked Processing

```rust
use pdf_x::core::AsyncHttpChunkedStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = AsyncHttpChunkedStream::open(
        "https://example.com/document.pdf",
        Some(65536),  // 64KB chunks
        Some(5),      // Small cache for streaming
        None,
    ).await?;

    let chunk_size = 65536;
    let total_size = stream.length();
    let mut bytes_read = 0;

    while bytes_read < total_size {
        let to_read = std::cmp::min(chunk_size, total_size - bytes_read);
        let chunk = stream.get_bytes(to_read).await?;

        // Process chunk
        println!("Read {} bytes (total: {})", chunk.len(), bytes_read);

        bytes_read += chunk.len();
    }

    Ok(())
}
```

## Performance Considerations

### Chunk Size Selection

| Chunk Size | Use Case | Pros | Cons |
|------------|----------|------|------|
| 8-16 KB | Slow networks, mobile | Responsive, low latency | More HTTP requests |
| 64 KB (default) | General purpose | Balanced | Standard choice |
| 256 KB - 1 MB | Fast networks, large files | Fewer requests | Higher latency per request |

**Recommendation:** Use 64KB (default) for most cases. Increase for fast networks, decrease for slow/unreliable connections.

### Cache Size Selection

The cache stores recently accessed chunks in memory.

| Cache Size | Memory Usage | Use Case |
|------------|--------------|----------|
| 5-10 chunks | ~320-640 KB | Memory-constrained environments |
| 20 chunks (default) | ~1.3 MB | General purpose |
| 50-100 chunks | ~3-6 MB | Fast local processing |

**Formula:** `Memory = cache_size × chunk_size`

**Recommendation:** Use 20 chunks (default) for most cases. Increase for documents with many backward seeks.

### Network Optimization

```rust
// ✓ GOOD - Efficient chunk size and reasonable cache
let stream = AsyncHttpChunkedStream::open(
    url,
    Some(65536),   // 64KB chunks
    Some(20),      // 1.3MB cache
    Some(progress),
).await?;

// ⚠️ SUBOPTIMAL - Too small chunks = many requests
let stream = AsyncHttpChunkedStream::open(
    url,
    Some(4096),    // 4KB chunks - too many HTTP requests
    Some(100),     // 400KB cache
    None,
).await?;

// ⚠️ SUBOPTIMAL - Too large chunks = slow initial response
let stream = AsyncHttpChunkedStream::open(
    url,
    Some(1048576), // 1MB chunks - slow first byte
    Some(5),       // 5MB cache
    None,
).await?;
```

## Error Handling

### Server Requirements

The server MUST support HTTP Range requests. PDF-X checks for this during `open()`:

```rust
match AsyncHttpChunkedStream::open(url, None, None, None).await {
    Ok(stream) => {
        // Server supports ranges
        println!("✓ Ready to load PDF");
    }
    Err(PDFError::StreamError(msg)) if msg.contains("range requests") => {
        eprintln!("✗ Server doesn't support HTTP Range requests");
        eprintln!("  Cannot use progressive loading");
    }
    Err(e) => {
        eprintln!("✗ Error: {}", e);
    }
}
```

### Network Errors

```rust
use pdf_x::core::{AsyncHttpChunkedStream, PDFError};

match AsyncHttpChunkedStream::open(url, None, None, None).await {
    Ok(stream) => { /* success */ }
    Err(PDFError::IOError { message }) => {
        eprintln!("Network error: {}", message);
        // Handle: DNS failure, connection timeout, SSL error
    }
    Err(PDFError::StreamError(msg)) => {
        eprintln!("Stream error: {}", msg);
        // Handle: Invalid HTTP response, no range support
    }
    Err(e) => {
        eprintln!("Unexpected error: {}", e);
    }
}
```

### Reading Errors

```rust
let mut stream = AsyncHttpChunkedStream::open(url, None, None, None).await?;

match stream.get_bytes(100).await {
    Ok(data) => {
        // Successfully read 100 bytes
        println!("Read: {:?}", data);
    }
    Err(PDFError::UnexpectedEndOfStream) => {
        eprintln!("Tried to read past end of file");
    }
    Err(PDFError::IOError { message }) => {
        eprintln!("Network error during read: {}", message);
        // Handle: Connection lost, timeout
    }
    Err(e) => {
        eprintln!("Read error: {}", e);
    }
}
```

## Examples

### Complete Example: Download and Parse

```rust
use pdf_x::core::{AsyncHttpChunkedStream, PDFDocument};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf";

    println!("Loading PDF from: {}", url);

    // Progress callback
    let progress = Box::new(|loaded: usize, total: usize| {
        let percent = (loaded * 100) / total;
        print!("\rProgress: {}%", percent);
        use std::io::Write;
        std::io::stdout().flush().unwrap();
    });

    // Load PDF
    let mut stream = AsyncHttpChunkedStream::open(
        url,
        Some(65536),
        Some(10),
        Some(progress),
    ).await?;

    println!("\n✓ PDF loaded");
    println!("  Size: {} bytes", stream.length());
    println!("  Chunks: {}", stream.num_chunks());

    // Read entire PDF
    let data = stream.get_bytes(stream.length()).await?;

    // Parse PDF
    let mut doc = PDFDocument::open(data)?;
    println!("  Pages: {}", doc.page_count()?);

    // Extract text from first page
    if doc.page_count()? > 0 {
        let page = doc.get_page(0)?;
        let text_items = page.extract_text(&mut doc.xref_mut())?;
        println!("\nFirst page text:");
        for item in text_items.iter().take(5) {
            println!("  {}", item.text);
        }
    }

    Ok(())
}
```

### Example: Streaming Validation

```rust
use pdf_x::core::AsyncHttpChunkedStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = "https://example.com/document.pdf";

    let mut stream = AsyncHttpChunkedStream::open(url, None, None, None).await?;

    // Validate PDF header without downloading entire file
    let header = stream.get_bytes(8).await?;

    if !header.starts_with(b"%PDF-") {
        eprintln!("✗ Not a valid PDF file");
        return Ok(());
    }

    let version = &header[5..8];
    println!("✓ Valid PDF (version: {})", String::from_utf8_lossy(version));

    // Read last 1KB to find trailer
    stream.set_pos(stream.length() - 1024).await?;
    let trailer_area = stream.get_bytes(1024).await?;

    if let Some(pos) = trailer_area.windows(9).position(|w| w == b"startxref") {
        println!("✓ Found startxref at offset {}", stream.length() - 1024 + pos);
    }

    println!("✓ Validation complete (only downloaded ~1KB)");

    Ok(())
}
```

## See Also

- [Progressive Loading Guide](progressive-loading.md) - General progressive loading concepts
- [Error Handling Guide](error-handling.md) - Comprehensive error handling
- [examples/http_loading.rs](../examples/http_loading.rs) - Full working example
