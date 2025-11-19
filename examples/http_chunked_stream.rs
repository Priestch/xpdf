use pdf_x::core::{BaseStream, HttpChunkedStream};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Example: Open a PDF file from HTTP with progressive loading
    // Uses HTTP range requests to fetch chunks on-demand
    let url = "
https://mozilla.github.io/pdf.js/legacy/web/compressed.tracemonkey-pldi-09.pdf";

    let mut stream = HttpChunkedStream::open(
        url,
        Some(65536),  // 64KB chunks
        Some(10),     // Keep max 10 chunks in memory (640KB cache)
    )?;

    println!("URL: {}", stream.url());
    println!("File length: {} bytes", stream.length());
    println!("Total chunks: {}", stream.num_chunks());
    println!("Chunk size: 64KB");
    println!("Max memory: ~640KB\n");

    // Read first byte (downloads first chunk automatically via HTTP range request)
    let first_byte = stream.get_byte()?;
    println!("First byte: {:#X}", first_byte);
    println!("Chunks loaded: {}", stream.num_chunks_loaded());

    // Read more bytes
    stream.set_pos(0)?;
    let header = stream.get_bytes(8)?;
    println!("\nPDF Header: {:?}", String::from_utf8_lossy(&header));

    // Check if it's a valid PDF
    if header.starts_with(b"%PDF") {
        println!("Valid PDF file detected!");
    }

    // Read at different position (may download another chunk)
    if stream.length() > 1000 {
        stream.set_pos(1000)?;
        let byte_at_1000 = stream.get_byte()?;
        println!("\nByte at position 1000: {:#X}", byte_at_1000);
        println!("Chunks loaded: {}", stream.num_chunks_loaded());
    }

    Ok(())
}
