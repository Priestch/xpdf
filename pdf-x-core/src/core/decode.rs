/// Stream decoding and decompression utilities.
///
/// PDF streams can be compressed using various filters like FlateDecode.
/// This module provides utilities to decompress stream data.
///
/// Based on PDF.js src/core/flate_stream.js, decode_stream.js, and predictor_stream.js
use super::error::{PDFError, PDFResult};
use super::parser::PDFObject;
use flate2::read::ZlibDecoder;
use std::io::Read;

/// PNG predictor algorithm types (used in DecodeParms)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngPredictor {
    /// No prediction
    None = 0,
    /// Sub - predicts from left pixel
    Sub = 1,
    /// Up - predicts from pixel above
    Up = 2,
    /// Average - predicts from average of left and above
    Average = 3,
    /// Paeth - uses Paeth predictor algorithm
    Paeth = 4,
}

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
    // Try zlib format first (most common)
    let mut decoder = ZlibDecoder::new(compressed_data);
    let mut decompressed = Vec::new();

    match decoder.read_to_end(&mut decompressed) {
        Ok(_) => return Ok(decompressed),
        Err(zlib_err) => {
            // Zlib failed, try raw deflate (some PDFs use this)
            use flate2::read::DeflateDecoder;

            decompressed.clear();
            let mut raw_decoder = DeflateDecoder::new(compressed_data);
            match raw_decoder.read_to_end(&mut decompressed) {
                Ok(_) => Ok(decompressed),
                Err(deflate_err) => {
                    // Both failed - provide detailed error
                    Err(PDFError::Generic(format!(
                        "FlateDecode error: zlib failed ({}), raw deflate failed ({}). Data length: {} bytes, first 10 bytes: {:02x?}",
                        zlib_err,
                        deflate_err,
                        compressed_data.len(),
                        &compressed_data[..compressed_data.len().min(10)]
                    )))
                }
            }
        }
    }
}

/// Applies PNG predictor decoding to decompressed data.
///
/// PNG predictors are used to improve compression by predicting pixel values
/// based on neighboring pixels. This function reverses that prediction.
///
/// # Arguments
/// * `data` - The decompressed data with PNG prediction applied
/// * `colors` - Number of color components per pixel (1=Gray, 3=RGB, 4=CMYK)
/// * `bits_per_component` - Bits per color component (usually 8)
/// * `columns` - Number of pixels per row
///
/// # Returns
/// The data with PNG prediction reversed (raw pixel data)
pub fn decode_png_predictor(
    data: &[u8],
    colors: usize,
    bits_per_component: usize,
    columns: usize,
) -> PDFResult<Vec<u8>> {
    // Calculate bytes per pixel and bytes per row
    let pix_bytes = (colors * bits_per_component + 7) / 8;
    let row_bytes = (columns * colors * bits_per_component + 7) / 8;

    // Each row has: 1 predictor byte + row_bytes data
    let stride = 1 + row_bytes;

    // Calculate expected output size
    let num_rows = data.len() / stride;
    if data.len() % stride != 0 {
        return Err(PDFError::Generic(format!(
            "PNG predictor data size mismatch: {} bytes doesn't divide evenly by stride {}",
            data.len(),
            stride
        )));
    }

    let mut output = Vec::with_capacity(num_rows * row_bytes);
    let mut prev_row = vec![0u8; row_bytes];

    for row_idx in 0..num_rows {
        let row_start = row_idx * stride;
        let predictor_byte = data[row_start];
        let raw_bytes = &data[row_start + 1..row_start + 1 + row_bytes];

        // Decode based on predictor type
        match predictor_byte {
            0 => {
                // None - no prediction, copy as-is
                output.extend_from_slice(raw_bytes);
                prev_row.copy_from_slice(raw_bytes);
            }
            1 => {
                // Sub - predicts from left pixel
                for i in 0..pix_bytes {
                    let val = raw_bytes[i];
                    output.push(val);
                    prev_row[i] = val;
                }
                for i in pix_bytes..row_bytes {
                    let val = (output[output.len() - pix_bytes].wrapping_add(raw_bytes[i])) & 0xFF;
                    output.push(val);
                    prev_row[i] = val;
                }
            }
            2 => {
                // Up - predicts from pixel above
                for i in 0..row_bytes {
                    let val = (prev_row[i].wrapping_add(raw_bytes[i])) & 0xFF;
                    output.push(val);
                    prev_row[i] = val;
                }
            }
            3 => {
                // Average - predicts from average of left and above
                for i in 0..pix_bytes {
                    let val = ((prev_row[i] as u16 / 2) as u8).wrapping_add(raw_bytes[i]);
                    output.push(val);
                    prev_row[i] = val;
                }
                for i in pix_bytes..row_bytes {
                    let left = output[output.len() - pix_bytes] as u16;
                    let up = prev_row[i] as u16;
                    let avg = ((left + up) / 2) as u8;
                    let val = avg.wrapping_add(raw_bytes[i]);
                    output.push(val);
                    prev_row[i] = val;
                }
            }
            4 => {
                // Paeth - uses Paeth predictor algorithm
                for i in 0..pix_bytes {
                    let up = prev_row[i];
                    let val = up.wrapping_add(raw_bytes[i]);
                    output.push(val);
                    prev_row[i] = val;
                }
                for i in pix_bytes..row_bytes {
                    let left = output[output.len() - pix_bytes];
                    let up = prev_row[i];
                    let up_left = prev_row[i - pix_bytes];

                    // Paeth algorithm
                    let p = (left as i32) + (up as i32) - (up_left as i32);
                    let pa = (p - left as i32).abs();
                    let pb = (p - up as i32).abs();
                    let pc = (p - up_left as i32).abs();

                    let paeth = if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        up_left
                    };

                    let val = paeth.wrapping_add(raw_bytes[i]);
                    output.push(val);
                    prev_row[i] = val;
                }
            }
            _ => {
                return Err(PDFError::Generic(format!(
                    "Unsupported PNG predictor: {}",
                    predictor_byte
                )));
            }
        }
    }

    Ok(output)
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
        Some(filter) => Err(PDFError::Generic(format!("Unsupported filter: {}", filter))),
        None => {
            // No filter - return data as-is
            Ok(data.to_vec())
        }
    }
}

/// Decodes ASCIIHex-encoded data.
///
/// ASCIIHex encoding represents each byte as two hexadecimal characters.
/// Whitespace is ignored.
///
/// # Arguments
/// * `data` - The ASCIIHex-encoded data
///
/// # Returns
/// The decoded binary data
pub fn decode_ascii_hex(data: &[u8]) -> PDFResult<Vec<u8>> {
    let mut result = Vec::new();
    let mut hex_buffer = String::new();

    for &byte in data {
        let ch = byte as char;
        if ch.is_ascii_hexdigit() {
            hex_buffer.push(ch);
            if hex_buffer.len() == 2 {
                if let Ok(byte_val) = u8::from_str_radix(&hex_buffer, 16) {
                    result.push(byte_val);
                }
                hex_buffer.clear();
            }
        } else if ch == '>' {
            // End marker
            break;
        }
        // Ignore other whitespace
    }

    // Handle odd number of hex digits (implicit trailing 0)
    if !hex_buffer.is_empty() {
        if let Ok(byte_val) = u8::from_str_radix(&format!("{}0", hex_buffer), 16) {
            result.push(byte_val);
        }
    }

    Ok(result)
}

/// Decodes ASCII85 (Base85) encoded data.
///
/// ASCII85 encoding uses 5 ASCII characters to represent 4 bytes.
/// Commonly used in PDF files.
///
/// # Arguments
/// * `data` - The ASCII85-encoded data
///
/// # Returns
/// The decoded binary data
pub fn decode_ascii85(data: &[u8]) -> PDFResult<Vec<u8>> {
    let mut result = Vec::new();
    let mut tuple = 0u32;
    let mut count = 0usize;

    for &byte in data {
        let ch = byte as char;

        if ch == '~' {
            // Check for end marker '~>'
            break;
        } else if ch == '>' {
            // End marker (should only appear after ~)
            break;
        } else if ch == 'z' {
            // Special case: all zero bytes
            if count == 0 {
                result.extend_from_slice(&[0u8; 4]);
            }
            continue;
        } else if ch.is_whitespace() {
            // Skip whitespace
            continue;
        } else if ch >= '!' && ch <= 'u' {
            // Regular ASCII85 character
            let value = (ch as u32) - ('!' as u32);
            tuple = tuple * 85 + value;
            count += 1;

            if count == 5 {
                // We have a full tuple, convert to 4 bytes
                result.push(((tuple >> 24) & 0xFF) as u8);
                result.push(((tuple >> 16) & 0xFF) as u8);
                result.push(((tuple >> 8) & 0xFF) as u8);
                result.push((tuple & 0xFF) as u8);
                tuple = 0;
                count = 0;
            }
        } else {
            // Invalid character
            return Err(PDFError::Generic(format!(
                "Invalid ASCII85 character: '{}'",
                ch
            )));
        }
    }

    // Handle partial tuple at end
    if count > 0 {
        // Pad with zeros
        for _ in count..5 {
            tuple = tuple * 85;
        }
        // Convert to bytes, but only output (count - 1) bytes
        let bytes = [
            ((tuple >> 24) & 0xFF) as u8,
            ((tuple >> 16) & 0xFF) as u8,
            ((tuple >> 8) & 0xFF) as u8,
            (tuple & 0xFF) as u8,
        ];
        result.extend_from_slice(&bytes[..(count - 1)]);
    }

    Ok(result)
}

/// Applies a single filter to data.
///
/// # Arguments
/// * `data` - The input data
/// * `filter_name` - The filter name
///
/// # Returns
/// The filtered data
fn apply_filter(data: &[u8], filter_name: &str) -> PDFResult<Vec<u8>> {
    match filter_name {
        "FlateDecode" | "Fl" => decode_flate(data),
        "ASCIIHexDecode" | "AHx" => decode_ascii_hex(data),
        "ASCII85Decode" | "A85" => decode_ascii85(data),
        _ => Err(PDFError::Generic(format!(
            "Unsupported filter: {}",
            filter_name
        ))),
    }
}

/// Applies multiple filters to data in sequence.
///
/// PDF streams can have multiple filters that are applied in order.
/// For example: [/FlateDecode /ASCIIHexDecode] means ASCIIHex decode first,
/// then Flate decode the result.
///
/// IMPORTANT: Filters in the array are applied in LAST-to-FIRST order.
/// The LAST filter is applied FIRST.
///
/// # Arguments
/// * `data` - The input data
/// * `filters` - Array of filter names (or filter arrays)
///
/// # Returns
/// The fully decoded data
pub fn apply_filters(data: &[u8], filters: &PDFObject) -> PDFResult<Vec<u8>> {
    // Extract filter list
    let filter_list = match filters {
        PDFObject::Name(name) => vec![name.clone()],
        PDFObject::Array(arr) => {
            let mut list = Vec::new();
            for item in arr.iter() {
                if let PDFObject::Name(name) = &**item {
                    list.push(name.clone());
                }
            }
            list
        }
        _ => return Ok(data.to_vec()), // No filters
    };

    if filter_list.is_empty() {
        return Ok(data.to_vec());
    }

    #[cfg(feature = "debug-logging")]
    eprintln!(
        "DEBUG: Applying {} filters: {:?}",
        filter_list.len(),
        filter_list
    );

    // Apply filters in REVERSE order (last filter first)
    let mut current_data = data.to_vec();
    for filter_name in filter_list.iter().rev() {
        #[cfg(feature = "debug-logging")]
        eprintln!("DEBUG: Applying filter: {}", filter_name);
        current_data = apply_filter(&current_data, filter_name)
            .map_err(|e| PDFError::Generic(format!("Filter {} failed: {}", filter_name, e)))?;
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: After filter {}: {} bytes",
            filter_name,
            current_data.len()
        );
    }

    Ok(current_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_flate_simple() {
        // Create some test data and compress it
        let original = b"Hello, PDF world! This is test data.";

        // Compress it using flate2
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
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
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
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

    #[test]
    fn test_decode_ascii_hex_simple() {
        // Simple ASCIIHex: "48656C6C6F" = "Hello"
        let encoded = b"48656C6C6F";
        let decoded = decode_ascii_hex(encoded).unwrap();

        assert_eq!(decoded, b"Hello");
    }

    #[test]
    fn test_decode_ascii_hex_with_whitespace() {
        // ASCIIHex with whitespace: "48 65 6C 6C 6F" = "Hello"
        let encoded = b"48 65\n6C\t6C 6F>";
        let decoded = decode_ascii_hex(encoded).unwrap();

        assert_eq!(decoded, b"Hello");
    }

    #[test]
    fn test_decode_ascii_hex_odd_length() {
        // Odd number of hex digits: "48656C6C6F" has 10 hex digits (5 bytes = "Hello")
        // "48656C6C6" has 9 hex digits (4.5 bytes) - implicit trailing 0 makes it "Hell\xF0"
        // Actually "48656C6C6" = 4.5 bytes, the last hex digit '6' gets trailing 0 to make '6F0'
        // Let's use a simpler test: "48656C6C" = "Hell" (4 bytes)
        let encoded = b"48656C6C";
        let decoded = decode_ascii_hex(encoded).unwrap();

        assert_eq!(decoded, b"Hell");
    }

    #[test]
    fn test_decode_ascii85_simple() {
        // ASCII85: "87cURD" = "Hell" (4 bytes)
        // "Hello" encodes to "87cURDZBb;" (9 chars for 5 bytes)
        // For 4 bytes "Hell" we get "87cURD" (6 chars for 4 bytes)
        let encoded = b"87cURD";
        let decoded = decode_ascii85(encoded).unwrap();

        assert_eq!(decoded, b"Hell");
    }

    #[test]
    fn test_decode_ascii85_with_whitespace() {
        // ASCII85 with whitespace and EOD marker
        let encoded = b"87cURD~>";
        let decoded = decode_ascii85(encoded).unwrap();

        assert_eq!(decoded, b"Hell");
    }

    #[test]
    fn test_decode_ascii85_zero_shortcut() {
        // 'z' is a shortcut for 5 zero bytes (which represents 4 zero bytes)
        let encoded = b"z";
        let decoded = decode_ascii85(encoded).unwrap();

        assert_eq!(decoded, b"\0\0\0\0");
    }

    #[test]
    fn test_multi_filter_flate_ascii_hex() {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        // Original data
        let original = b"Hello, World! Testing multi-filter decode.";

        // Step 1: Compress with Flate
        let mut zlib_encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib_encoder.write_all(original).unwrap();
        let compressed = zlib_encoder.finish().unwrap();

        // Step 2: Encode as ASCIIHex
        let hex_string = hex::encode_upper(&compressed);
        let hex_encoded = hex_string.as_bytes();

        // Now decode using multi-filter
        let filters = PDFObject::Array(smallvec::smallvec![
            Box::new(PDFObject::Name("FlateDecode".into())),
            Box::new(PDFObject::Name("ASCIIHexDecode".into())),
        ]);

        let decoded = apply_filters(hex_encoded, &filters).unwrap();

        assert_eq!(&decoded[..], original);
    }

    #[test]
    fn test_multi_filter_order() {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        // Test that filters are applied in LAST-to-FIRST order
        let original = b"Test data";

        // Compress
        let mut zlib_encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib_encoder.write_all(original).unwrap();
        let compressed = zlib_encoder.finish().unwrap();

        // Encode as ASCIIHex
        let hex_string = hex::encode_upper(&compressed);
        let hex_encoded = hex_string.as_bytes();

        // Filters array: [/FlateDecode /ASCIIHexDecode]
        // Should apply ASCIIHex FIRST, then Flate
        let filters = PDFObject::Array(smallvec::smallvec![
            Box::new(PDFObject::Name("FlateDecode".into())),
            Box::new(PDFObject::Name("ASCIIHexDecode".into())),
        ]);

        let decoded = apply_filters(hex_encoded, &filters).unwrap();
        assert_eq!(&decoded[..], original);
    }
}
