use pdf_x::core::{BaseStream, FileChunkedStream};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Example: Open a PDF file with progressive loading
    // Only 10 chunks (640KB) will be kept in memory at a time
    let mut stream = FileChunkedStream::open(
        "/home/gp/Books/d2l-en.pdf",
        Some(65536),  // 64KB chunks
        Some(10),     // Keep max 10 chunks in memory (640KB cache)
    )?;

    println!("File length: {} bytes", stream.length());
    println!("Total chunks: {}", stream.num_chunks());
    println!("Chunk size: 64KB");
    println!("Max memory: ~640KB\n");

    // Read first byte (loads first chunk automatically)
    let first_byte = stream.get_byte()?;
    println!("First byte: {:#X}", first_byte);
    println!("Chunks loaded: {}", stream.num_chunks_loaded());

    // Skip ahead and read (loads another chunk)
    stream.set_pos(100_000)?;
    let byte_at_100k = stream.get_byte()?;
    println!("\nByte at 100KB: {:#X}", byte_at_100k);
    println!("Chunks loaded: {}", stream.num_chunks_loaded());

    // Read a range of bytes
    stream.set_pos(0)?;
    let header = stream.get_bytes(8)?;
    println!("\nPDF Header: {:?}", String::from_utf8_lossy(&header));

    // Preload a specific range for better performance
    println!("\nPreloading bytes 200,000-300,000...");
    stream.preload_range(200_000, 300_000)?;
    println!("Chunks loaded: {}", stream.num_chunks_loaded());

    Ok(())
}
