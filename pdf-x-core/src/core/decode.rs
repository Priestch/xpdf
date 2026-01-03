/// Stream decoding and decompression utilities.
///
/// PDF streams can be compressed using various filters like FlateDecode.
/// This module provides utilities to decompress stream data.
///
/// Based on PDF.js src/core/flate_stream.js, decode_stream.js, and predictor_stream.js

use super::error::{PDFError, PDFResult};
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
                )))
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
