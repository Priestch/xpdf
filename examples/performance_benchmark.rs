use pdf_x::core::{BaseStream, FileChunkedStream};
use std::io::Write;
use std::time::Instant;
use tempfile::NamedTempFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 1 Performance Improvement Benchmark ===\n");

    // Create a test file with 10MB of data
    let size = 10 * 1024 * 1024; // 10 MB
    println!("Creating test file: {} MB", size / 1024 / 1024);

    let mut temp_file = NamedTempFile::new()?;
    let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
    temp_file.write_all(&data)?;
    temp_file.flush()?;

    // Open with chunked stream
    let mut stream = FileChunkedStream::open(
        temp_file.path(),
        Some(65536), // 64KB chunks
        Some(20),    // Cache 20 chunks (1.25 MB)
    )?;

    println!("File size: {} MB", stream.length() / 1024 / 1024);
    println!("Chunk size: 64 KB");
    println!("Total chunks: {}", stream.num_chunks());
    println!("Max cached chunks: 20 (~1.25 MB)\n");

    // Test 1: Small read (100 bytes)
    println!("Test 1: Reading 100 bytes");
    stream.set_pos(0)?;
    let start = Instant::now();
    let bytes = stream.get_bytes(100)?;
    let duration = start.elapsed();
    println!("  Time: {:?}", duration);
    println!("  Bytes read: {}", bytes.len());
    assert_eq!(bytes.len(), 100);

    // Test 2: Medium read (10 KB)
    println!("\nTest 2: Reading 10 KB");
    stream.set_pos(1000)?;
    let start = Instant::now();
    let bytes = stream.get_bytes(10 * 1024)?;
    let duration = start.elapsed();
    println!("  Time: {:?}", duration);
    println!("  Bytes read: {}", bytes.len());
    assert_eq!(bytes.len(), 10 * 1024);

    // Test 3: Large read (1 MB) - This is where the optimization shines
    println!("\nTest 3: Reading 1 MB (key performance test)");
    stream.set_pos(100_000)?;
    let start = Instant::now();
    let bytes = stream.get_bytes(1024 * 1024)?;
    let duration = start.elapsed();
    println!("  Time: {:?}", duration);
    println!("  Bytes read: {} KB", bytes.len() / 1024);
    println!("  Chunks spanned: ~16");
    assert_eq!(bytes.len(), 1024 * 1024);

    // Test 4: Multi-chunk read spanning boundaries
    println!("\nTest 4: Reading across 5 chunk boundaries");
    stream.set_pos(65000)?; // Near end of first chunk
    let start = Instant::now();
    let bytes = stream.get_bytes(320_000)?; // ~5 chunks
    let duration = start.elapsed();
    println!("  Time: {:?}", duration);
    println!("  Bytes read: {} KB", bytes.len() / 1024);
    assert_eq!(bytes.len(), 320_000);

    // Test 5: Sequential reads (realistic PDF parsing scenario)
    println!("\nTest 5: Sequential small reads (100x 4KB reads)");
    stream.set_pos(200_000)?;
    let start = Instant::now();
    for _ in 0..100 {
        let _bytes = stream.get_bytes(4096)?;
    }
    let duration = start.elapsed();
    println!("  Time: {:?}", duration);
    println!("  Total bytes read: {} KB", (100 * 4096) / 1024);

    println!("\n=== Performance Summary ===");
    println!("✓ All tests passed");
    println!("✓ Optimization: Chunk-wise copying instead of byte-by-byte");
    println!("✓ Expected improvement: 10-100x faster for large reads");
    println!("\nBefore optimization:");
    println!("  - 1 MB read: ~1,048,576 iterations");
    println!("After optimization:");
    println!("  - 1 MB read: ~16 chunk slice copies");
    println!("  - Improvement: ~65,000x fewer iterations!");

    Ok(())
}
