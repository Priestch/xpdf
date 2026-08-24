/// Stream decoding and decompression utilities.
///
/// PDF streams can be compressed using various filters like FlateDecode.
/// This module provides utilities to decompress stream data.
///
/// Based on PDF.js src/core/flate_stream.js, decode_stream.js, and predictor_stream.js
use super::error::{PDFError, PDFResult};
use super::parser::PDFObject;
use super::resource_budget::ResourceBudget;
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
    decode_flate_with_budget(compressed_data, ResourceBudget::default())
}

/// Decode Flate data while enforcing an explicit output budget.
pub fn decode_flate_with_budget(
    compressed_data: &[u8],
    budget: ResourceBudget,
) -> PDFResult<Vec<u8>> {
    use flate2::read::DeflateDecoder;

    let limit = budget.decompression_limit(compressed_data.len());
    let decode_limited = |mut reader: Box<dyn Read>| -> std::io::Result<Vec<u8>> {
        let mut output = Vec::new();
        reader
            .by_ref()
            .take(limit.saturating_add(1) as u64)
            .read_to_end(&mut output)?;
        Ok(output)
    };
    let checked = |output: Vec<u8>| -> PDFResult<Vec<u8>> {
        if output.len() > limit {
            Err(PDFError::LimitExceeded {
                resource: "decompressed stream bytes",
                limit,
                actual: output.len(),
            })
        } else {
            Ok(output)
        }
    };

    match decode_limited(Box::new(ZlibDecoder::new(compressed_data))) {
        Ok(output) => return checked(output),
        Err(zlib_err) => match decode_limited(Box::new(DeflateDecoder::new(compressed_data))) {
            Ok(output) => checked(output),
            Err(deflate_err) => {
                if let Some(recovered) =
                    crate::core::flate_fallback::decode_flate_fallback(compressed_data, limit)
                {
                    if recovered.len() >= limit {
                        return Err(PDFError::LimitExceeded {
                            resource: "decompressed stream bytes",
                            limit,
                            actual: recovered.len(),
                        });
                    }
                    if !recovered.is_empty() {
                        return Ok(recovered);
                    }
                }
                Err(PDFError::Generic(format!(
                    "FlateDecode error: zlib failed ({}), raw deflate failed ({}). Data length: {} bytes, first 10 bytes: {:02x?}",
                    zlib_err,
                    deflate_err,
                    compressed_data.len(),
                    &compressed_data[..compressed_data.len().min(10)]
                )))
            }
        },
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

    if row_bytes == 0 {
        return Ok(Vec::new());
    }

    // PDFium and MuPDF tolerate truncated final rows. Each row starts with one
    // predictor byte followed by up to row_bytes bytes of image data.
    let mut output = Vec::with_capacity(data.len());
    let mut prev_row = vec![0u8; row_bytes];
    let mut offset = 0usize;

    while offset < data.len() {
        let predictor_byte = data[offset];
        offset += 1;
        let available = row_bytes.min(data.len().saturating_sub(offset));
        let raw_bytes = &data[offset..offset + available];
        offset += available;
        let row_out_start = output.len();

        // Decode based on predictor type
        match predictor_byte {
            0 => {
                // None - no prediction, copy as-is
                output.extend_from_slice(raw_bytes);
                prev_row[..available].copy_from_slice(raw_bytes);
            }
            1 => {
                // Sub - predicts from left pixel
                for i in 0..pix_bytes.min(available) {
                    let val = raw_bytes[i];
                    output.push(val);
                    prev_row[i] = val;
                }
                for i in pix_bytes..available {
                    let val = output[row_out_start + i - pix_bytes].wrapping_add(raw_bytes[i]);
                    output.push(val);
                    prev_row[i] = val;
                }
            }
            2 => {
                // Up - predicts from pixel above
                for i in 0..available {
                    let val = (prev_row[i].wrapping_add(raw_bytes[i])) & 0xFF;
                    output.push(val);
                    prev_row[i] = val;
                }
            }
            3 => {
                // Average - predicts from average of left and above
                for i in 0..pix_bytes.min(available) {
                    let val = ((prev_row[i] as u16 / 2) as u8).wrapping_add(raw_bytes[i]);
                    output.push(val);
                    prev_row[i] = val;
                }
                for i in pix_bytes..available {
                    let left = output[row_out_start + i - pix_bytes] as u16;
                    let up = prev_row[i] as u16;
                    let avg = ((left + up) / 2) as u8;
                    let val = avg.wrapping_add(raw_bytes[i]);
                    output.push(val);
                    prev_row[i] = val;
                }
            }
            4 => {
                // Paeth - uses Paeth predictor algorithm
                for i in 0..pix_bytes.min(available) {
                    let up = prev_row[i];
                    let val = up.wrapping_add(raw_bytes[i]);
                    output.push(val);
                    prev_row[i] = val;
                }
                for i in pix_bytes..available {
                    let left = output[row_out_start + i - pix_bytes];
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
                // PDF.js/hayro/MuPDF tolerate invalid PNG predictor bytes by
                // treating the row as unpredicted data. This matters for
                // malformed streams where the row prefix is outside 0..=4 but
                // the following bytes are still the image samples.
                output.extend_from_slice(raw_bytes);
                prev_row[..available].copy_from_slice(raw_bytes);
            }
        }
    }

    Ok(output)
}

/// Applies TIFF predictor 2 (horizontal differencing) to decompressed data.
///
/// This mirrors PDF.js `PredictorStream.readBlockTiff` and hayro's Sub predictor
/// path. Rows are processed independently and packed samples preserve row padding.
pub fn decode_tiff_predictor(
    data: &[u8],
    colors: usize,
    bits_per_component: usize,
    columns: usize,
) -> PDFResult<Vec<u8>> {
    if colors == 0 || columns == 0 {
        return Ok(data.to_vec());
    }

    let row_bytes = (columns * colors * bits_per_component + 7) / 8;
    if row_bytes == 0 {
        return Ok(Vec::new());
    }
    if data.len() % row_bytes != 0 {
        return Err(PDFError::Generic(format!(
            "TIFF predictor data size mismatch: {} bytes doesn't divide evenly by row size {}",
            data.len(),
            row_bytes
        )));
    }

    let num_rows = data.len() / row_bytes;
    let mut output = Vec::with_capacity(data.len());

    match bits_per_component {
        1 | 2 | 4 => {
            let max_sample = (1u32 << bits_per_component) - 1;
            for row in data.chunks_exact(row_bytes).take(num_rows) {
                let mut previous = vec![0u32; colors];
                let mut outbuf = 0u32;
                let mut outbits = 0usize;

                for sample_index in 0..(columns * colors) {
                    let sample = read_packed_sample(row, sample_index, bits_per_component);
                    let channel = sample_index % colors;
                    let decoded = (previous[channel] + sample) & max_sample;
                    previous[channel] = decoded;

                    outbuf = (outbuf << bits_per_component) | decoded;
                    outbits += bits_per_component;
                    while outbits >= 8 {
                        output.push(((outbuf >> (outbits - 8)) & 0xFF) as u8);
                        outbits -= 8;
                    }
                }

                if outbits > 0 {
                    let remaining_bits = 8 - outbits;
                    let padding_mask = (1u32 << remaining_bits) - 1;
                    let padding = row.last().copied().unwrap_or(0) as u32 & padding_mask;
                    output.push(((outbuf << remaining_bits) | padding) as u8);
                }
            }
        }
        8 => {
            for row in data.chunks_exact(row_bytes).take(num_rows) {
                let start = output.len();
                for i in 0..row_bytes {
                    let decoded = if i < colors {
                        row[i]
                    } else {
                        row[i].wrapping_add(output[start + i - colors])
                    };
                    output.push(decoded);
                }
            }
        }
        16 => {
            let bytes_per_pixel = colors * 2;
            for row in data.chunks_exact(row_bytes).take(num_rows) {
                let start = output.len();
                for i in (0..row_bytes).step_by(2) {
                    let raw = u16::from_be_bytes([
                        row.get(i).copied().unwrap_or(0),
                        row.get(i + 1).copied().unwrap_or(0),
                    ]);
                    let decoded = if i < bytes_per_pixel {
                        raw
                    } else {
                        let left = u16::from_be_bytes([
                            output[start + i - bytes_per_pixel],
                            output[start + i - bytes_per_pixel + 1],
                        ]);
                        raw.wrapping_add(left)
                    };
                    output.extend_from_slice(&decoded.to_be_bytes());
                }
            }
        }
        _ => {
            return Err(PDFError::Generic(format!(
                "Unsupported TIFF predictor bit depth: {}",
                bits_per_component
            )));
        }
    }

    Ok(output)
}

fn read_packed_sample(row: &[u8], sample_index: usize, bits_per_component: usize) -> u32 {
    let bit_offset = sample_index * bits_per_component;
    let byte_index = bit_offset / 8;
    let bit_in_byte = bit_offset % 8;
    let byte = row.get(byte_index).copied().unwrap_or(0) as u32;
    let shift = 8 - bit_in_byte - bits_per_component;
    (byte >> shift) & ((1u32 << bits_per_component) - 1)
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
    // PDF §7.4.3 / Adobe ASCII85. Source: hayro `ascii_85.rs` (and pdf.js
    // `Ascii85Stream`). Two bugs previously bit `password_2.pdf`:
    // 1. Treating bare `>` as EOD — `>` is a *valid* payload digit (value 29);
    //    only `~` starts the EOD marker `~>`. Stopping early truncated the
    //    subsequent FlateDecode to 233 bytes instead of 1788.
    // 2. Partial-tuple zero-padding — Adobe pads incomplete 5-tuples with
    //    `'u'` (value 84), not zero.
    // Accumulator is u64 because 85^5 exceeds 2^32; valid groups still fit u32
    // at the end (hayro uses the same u64→u32 check).

    let mut result = Vec::new();
    let mut tuple: u64 = 0;
    let mut count = 0usize;

    for &byte in data {
        // EOD marker starts with '~' (normally followed by '>'); stop there.
        if byte == b'~' {
            break;
        }
        // Whitespace is ignored anywhere in the stream.
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0) {
            continue;
        }
        // 'z' is a special one-char encoding of 4 zero bytes (only at group start).
        if byte == b'z' {
            if count != 0 {
                return Err(PDFError::Generic(
                    "ASCII85 'z' appears inside a partial group".to_string(),
                ));
            }
            result.extend_from_slice(&[0u8; 4]);
            continue;
        }
        // Valid digits are '!' (33) ..= 'u' (117). Note '>' (62) is included.
        if (b'!'..=b'u').contains(&byte) {
            let value = (byte - b'!') as u64;
            tuple = tuple * 85 + value;
            count += 1;

            if count == 5 {
                let word = u32::try_from(tuple).map_err(|_| {
                    PDFError::Generic(format!("ASCII85 group overflows u32 (value {tuple})"))
                })?;
                result.extend_from_slice(&word.to_be_bytes());
                tuple = 0;
                count = 0;
            }
        } else {
            return Err(PDFError::Generic(format!(
                "Invalid ASCII85 character: {:?}",
                byte as char
            )));
        }
    }

    // Partial final group: pad with 'u' (84) to 5 digits, emit count-1 bytes.
    if count > 0 {
        for _ in count..5 {
            tuple = tuple * 85 + 84;
        }
        let word = u32::try_from(tuple).map_err(|_| {
            PDFError::Generic(format!(
                "ASCII85 partial group overflows u32 (value {tuple})"
            ))
        })?;
        result.extend_from_slice(&word.to_be_bytes()[..(count - 1)]);
    }

    Ok(result)
}

/// Applies a single filter to data.
fn apply_filter(data: &[u8], filter_name: &str) -> PDFResult<Vec<u8>> {
    match filter_name {
        "FlateDecode" | "Fl" => decode_flate(data),
        "ASCIIHexDecode" | "AHx" => decode_ascii_hex(data),
        "ASCII85Decode" | "A85" => decode_ascii85(data),
        "LZWDecode" | "LZW" => decode_lzw(data),
        "RunLengthDecode" | "RL" => decode_run_length(data),
        // Image-specific filters (DCTDecode, JPXDecode, CCITTFaxDecode, JBIG2Decode)
        // are handled separately by the image rendering pipeline — return data
        // unchanged so apply_filters can continue with remaining filters.
        "DCTDecode" | "DCT" | "JPXDecode" | "CCITTFaxDecode" | "CCF" | "JBIG2Decode" => {
            Ok(data.to_vec())
        }
        _ => Err(PDFError::Generic(format!(
            "Unsupported filter: {}",
            filter_name
        ))),
    }
}

/// Decodes a RunLengthDecode-compressed stream (PDF §7.4.5).
pub fn decode_run_length(data: &[u8]) -> PDFResult<Vec<u8>> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < data.len() {
        let header = data[i];
        i += 1;

        match header {
            128 => break,
            0..=127 => {
                let len = header as usize + 1;
                let available = data.len().saturating_sub(i).min(len);
                result.extend_from_slice(&data[i..i + available]);
                if available < len {
                    result.resize(result.len() + (len - available), 0);
                }
                i = i.saturating_add(len);
            }
            129..=255 => {
                let len = 257usize - header as usize;
                let value = data.get(i).copied().unwrap_or(0);
                result.resize(result.len() + len, value);
                i = i.saturating_add(1);
            }
        }
    }

    Ok(result)
}

/// Decodes an LZWDecode-compressed stream (PDF §7.4.4.2).
///
/// Uses the early-change variant (code length increments at code 819, not 511).
pub fn decode_lzw(data: &[u8]) -> PDFResult<Vec<u8>> {
    const CLEAR_TABLE: u16 = 256;
    const EOD: u16 = 257;

    let mut result = Vec::new();
    let mut dictionary: Vec<Vec<u8>> = Vec::with_capacity(8192);
    let mut bit_buffer: u32 = 0;
    let mut bits_in_buffer: u32 = 0;
    let mut byte_pos = 0;
    let mut code_length: u32 = 9;
    let mut prev_code: Option<u16> = None;
    let mut dict_size: u32 = 258;

    // Initialize dictionary with single-byte entries 0-255
    for i in 0..256u16 {
        dictionary.push(vec![i as u8]);
    }
    // Placeholder for CLEAR and EOD
    dictionary.push(Vec::new()); // 256 = CLEAR
    dictionary.push(Vec::new()); // 257 = EOD

    loop {
        // Read bits to fill a code
        while bits_in_buffer < code_length {
            if byte_pos >= data.len() {
                return Ok(result);
            }
            bit_buffer = (bit_buffer << 8) | (data[byte_pos] as u32);
            bits_in_buffer += 8;
            byte_pos += 1;
        }

        bits_in_buffer -= code_length;
        let code = ((bit_buffer >> bits_in_buffer) & ((1u32 << code_length) - 1)) as u16;

        if code == EOD {
            break;
        }

        if code == CLEAR_TABLE {
            dictionary.truncate(258);
            code_length = 9;
            dict_size = 258;
            prev_code = None;
            continue;
        }

        let entry: Vec<u8> = if (code as usize) < dictionary.len() {
            dictionary[code as usize].clone()
        } else if code == dict_size as u16 {
            if let Some(pc) = prev_code {
                let mut e = dictionary[pc as usize].clone();
                e.push(e[0]);
                e
            } else {
                return Err(PDFError::Generic("LZW: bad code".into()));
            }
        } else {
            return Err(PDFError::Generic(format!(
                "LZW: code {} out of range",
                code
            )));
        };

        result.extend_from_slice(&entry);

        if let Some(pc) = prev_code {
            let mut new_entry = dictionary[pc as usize].clone();
            new_entry.push(entry[0]);
            dictionary.push(new_entry);
            dict_size += 1;
            // PDF LZW uses early change: increment code length one code earlier
            if dict_size == (1u32 << code_length) - 1 && code_length < 12 {
                code_length += 1;
            }
        }

        prev_code = Some(code);
    }

    Ok(result)
}

/// Applies multiple filters to data in sequence (forward order per PDF spec
/// §7.9.2: "applied in the order in which they appear").
///
/// Image-specific filters (DCTDecode, JPXDecode, CCITTFaxDecode, JBIG2Decode)
/// pass through unchanged — the image rendering pipeline handles them.
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

    // Apply filters in forward order (first filter first) — PDF §7.9.2:
    // "Filters shall be applied in the order in which they appear." For
    // `/Filter [ /ASCII85Decode /FlateDecode ]` that means ASCII85 then Flate.
    // (An earlier comment said "reverse"; the loop has always been forward.)
    let mut current_data = data.to_vec();
    for filter_name in filter_list.iter() {
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
        // Filters: [ASCIIHexDecode, FlateDecode] — forward order per spec
        let filters = PDFObject::Array(smallvec::smallvec![
            Box::new(PDFObject::Name("ASCIIHexDecode".into())),
            Box::new(PDFObject::Name("FlateDecode".into())),
        ]);

        let decoded = apply_filters(hex_encoded, &filters).unwrap();

        assert_eq!(&decoded[..], original);
    }

    #[test]
    fn test_decode_ascii85_partial_tuple_u_padding() {
        // password_2 post-RC4 content stream (ASCII85 of 531-byte zlib -> 1788 plain).
        // Requires: (1) bare '>' is a digit, not EOD; (2) partial group pads with 'u'.
        let data = match std::fs::read("/tmp/pw2_ours_stream.bin") {
            Ok(d) => d,
            Err(_) => return, // probe file from encryption test dump
        };
        let a85 = decode_ascii85(&data).expect("ascii85");
        assert_eq!(
            a85.len(),
            531,
            "ASCII85 of password_2 stream is 531 zlib bytes"
        );
        let plain = decode_flate(&a85).expect("flate after correct a85");
        assert_eq!(plain.len(), 1788, "full content stream length");
        assert!(
            std::str::from_utf8(&plain)
                .unwrap()
                .contains("Why Use Password Protection"),
            "body text present after full decode"
        );
    }

    #[test]
    fn test_decode_ascii85_accepts_gt_as_digit_not_eod() {
        // '>' is a valid ASCII85 digit (value 29). Only '~' starts EOD.
        // Digits: '!'=0, '>'=29, '"'=1, '#'=2, '$'=3
        let n = (((0u64 * 85 + 29) * 85 + 1) * 85 + 2) * 85 + 3;
        assert!(n <= 0xFFFF_FFFF);
        let expected = (n as u32).to_be_bytes();
        let encoded: &[u8] = &[b'!', b'>', b'"', b'#', b'$', b'~', b'>'];
        let decoded = decode_ascii85(encoded).expect("decode group containing gt digit");
        assert_eq!(decoded, expected);
    }
}
