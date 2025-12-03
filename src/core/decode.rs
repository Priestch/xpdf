/// Stream decoding and decompression utilities.
///
/// PDF streams can be compressed using various filters like FlateDecode.
/// This module provides utilities to decompress stream data.
///
/// Based on PDF.js src/core/flate_stream.js and decode_stream.js

use super::error::{PDFError, PDFResult};
use flate2::read::ZlibDecoder;
use std::io::Read;

/// Decodes a FlateDecode (zlib/deflate) compressed stream.
///
/// FlateDecode is the most common compression filter in PDF files.
/// It uses the zlib/deflate compression algorithm.
///
/// # Arguments
/// * `compressed_data` - The compressed stream data
///
/// # Returns
/// The decompressed data as a Vec<u8>
///
/// # Example
/// ```no_run
/// use pdf_x::core::decode::decode_flate;
///
/// let compressed = vec![/* compressed data */];
/// let decompressed = decode_flate(&compressed).unwrap();
/// ```
pub fn decode_flate(compressed_data: &[u8]) -> PDFResult<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(compressed_data);
    let mut decompressed = Vec::new();

    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| PDFError::Generic(format!("FlateDecode error: {}", e)))?;

    Ok(decompressed)
}

/// Decodes a stream based on its Filter entry.
///
/// PDF streams can have a /Filter entry specifying the compression algorithm.
/// This function checks the filter and applies the appropriate decompression.
///
/// Supported filters:
/// - /FlateDecode - zlib/deflate compression
///
/// # Arguments
/// * `data` - The stream data (potentially compressed)
/// * `filter_name` - The filter name from the stream dictionary (e.g., "FlateDecode")
///
/// # Returns
/// The decoded/decompressed data
pub fn decode_stream(data: &[u8], filter_name: Option<&str>) -> PDFResult<Vec<u8>> {
    match filter_name {
        Some("FlateDecode") => decode_flate(data),
        Some(filter) => Err(PDFError::Generic(format!(
            "Unsupported filter: {}",
            filter
        ))),
        None => {
            // No filter - return data as-is
            Ok(data.to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_flate_simple() {
        // Create some test data and compress it
        let original = b"Hello, PDF world! This is test data.";

        // Compress it using flate2
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        // Now decompress it using our function
        let decompressed = decode_flate(&compressed).unwrap();

        assert_eq!(&decompressed[..], original);
    }

    #[test]
    fn test_decode_stream_with_flate() {
        let original = b"Test data for stream decoding";

        // Compress
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        // Decompress using decode_stream
        let decompressed = decode_stream(&compressed, Some("FlateDecode")).unwrap();

        assert_eq!(&decompressed[..], original);
    }

    #[test]
    fn test_decode_stream_no_filter() {
        let original = b"Uncompressed data";

        let result = decode_stream(original, None).unwrap();

        assert_eq!(&result[..], original);
    }

    #[test]
    fn test_decode_stream_unsupported_filter() {
        let data = b"some data";

        let result = decode_stream(data, Some("UnsupportedFilter"));

        assert!(result.is_err());
    }
}
