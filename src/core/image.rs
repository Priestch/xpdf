//! Image extraction and decoding support.
//!
//! This module provides functionality to extract and decode images from PDF documents.
//! It follows hayro's approach by using specialized decoders for different image formats:
//! - zune-jpeg for JPEG (DCTDecode) - fast and SIMD-optimized
//! - hayro-jpeg2000 for JPEG2000 (JpxDecode) - complete specification support
//! - hayro-jbig2 for JBIG2 (Jbig2Decode) - document image compression
//!
//! The module provides two main APIs:
//! 1. Image metadata extraction (always available) - get image info without full decoding
//! 2. Complete image decoding (feature-gated) - full image data when requested

use super::error::{PDFResult, PDFError};

/// Image format types supported by PDF-X.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// JPEG format (DCTDecode)
    JPEG,
    /// JPEG2000 format (JpxDecode)
    JPEG2000,
    /// JBIG2 format (Jbig2Decode)
    JBIG2,
    /// PNG format (uncommon in PDFs but supported)
    PNG,
    /// Raw image data (FlateDecode, no compression, etc.)
    Raw,
    /// Unknown format
    Unknown,
}

impl ImageFormat {
    /// Detect image format from header bytes
    pub fn from_header(header: &[u8]) -> Self {
        if header.len() < 4 {
            return ImageFormat::Unknown;
        }

        // JPEG signature: FF D8 FF
        if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return ImageFormat::JPEG;
        }

        // JPEG2000 signature boxes
        if header.len() >= 12 &&
           header.starts_with(&[0x00, 0x00, 0x00, 0x0C, 0x6A, 0x50, 0x20, 0x20]) ||
           (header.starts_with(&[0xFF, 0x4F]) && header.len() > 4 && header[2..4] == [0xFF, 0x51]) {
            return ImageFormat::JPEG2000;
        }

        // PNG signature: 89 50 4E 47
        if header.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            return ImageFormat::PNG;
        }

        ImageFormat::Unknown
    }
}

/// Image metadata extracted from XObject headers.
#[derive(Debug, Clone)]
pub struct ImageMetadata {
    /// Image name/identifier
    pub name: String,
    /// Image format
    pub format: ImageFormat,
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Bits per component
    pub bits_per_component: u8,
    /// Color space information
    pub color_space: String,
    /// Whether the image has an alpha channel
    pub has_alpha: bool,
    /// Approximate size of the image data
    pub data_length: Option<usize>,
}

impl ImageMetadata {
    /// Create a new image metadata entry
    pub fn new(name: String, format: ImageFormat) -> Self {
        Self {
            name,
            format,
            width: 0,
            height: 0,
            bits_per_component: 8,
            color_space: "Unknown".to_string(),
            has_alpha: false,
            data_length: None,
        }
    }
}

/// Decoded image data.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    /// Image metadata
    pub metadata: ImageMetadata,
    /// Raw image data (pixel data)
    pub data: Vec<u8>,
    /// Image width
    pub width: u32,
    /// Image height
    pub height: u32,
    /// Number of color channels
    pub channels: u8,
    /// Color space type
    pub color_space: ImageColorSpace,
}

/// Image color space types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageColorSpace {
    /// Grayscale (1 channel)
    Gray,
    /// RGB (3 channels)
    RGB,
    /// RGBA (4 channels)
    RGBA,
    /// CMYK (4 channels)
    CMYK,
    /// Unknown format
    Unknown(u8),
}

impl DecodedImage {
    /// Create a new decoded image
    pub fn new(metadata: ImageMetadata, data: Vec<u8>, channels: u8) -> Self {
        Self {
            width: metadata.width,
            height: metadata.height,
            data,
            channels,
            color_space: match channels {
                1 => ImageColorSpace::Gray,
                3 => ImageColorSpace::RGB,
                4 => ImageColorSpace::RGBA,
                _ => ImageColorSpace::Unknown(channels),
            },
            metadata,
        }
    }
}

/// Image decoder following hayro's specialized approach.
pub struct ImageDecoder;

impl ImageDecoder {
    /// Detect image format from data header
    pub fn detect_format(data: &[u8]) -> ImageFormat {
        ImageFormat::from_header(data)
    }

    /// Decode image data using the appropriate decoder
    pub fn decode_image(data: &[u8], format: ImageFormat) -> PDFResult<DecodedImage> {
        match format {
            ImageFormat::JPEG => Self::decode_jpeg(data),
            #[cfg(feature = "advanced-image-formats")]
            ImageFormat::JPEG2000 => Self::decode_jpeg2000(data),
            #[cfg(feature = "advanced-image-formats")]
            ImageFormat::JBIG2 => Self::decode_jbig2(data),
            ImageFormat::PNG => Self::decode_png(data),
            ImageFormat::Raw => Err(PDFError::Generic(
                "Raw image decoding requires metadata (width, height, colorspace). Use decode_raw_image() instead.".to_string()
            )),
            #[cfg(not(feature = "advanced-image-formats"))]
            ImageFormat::JPEG2000 => Err(PDFError::Unsupported {
                feature: "JPEG2000 decoding not enabled. Enable the 'advanced-image-formats' feature.".to_string()
            }),
            #[cfg(not(feature = "advanced-image-formats"))]
            ImageFormat::JBIG2 => Err(PDFError::Unsupported {
                feature: "JBIG2 decoding not enabled. Enable the 'advanced-image-formats' feature.".to_string()
            }),
            ImageFormat::Unknown => Err(PDFError::Unsupported {
                feature: "Unknown image format".to_string()
            }),
        }
    }

    /// Decode raw image data (for FlateDecode and uncompressed images)
    ///
    /// Raw images in PDFs are just pixel data with no format headers.
    /// They need metadata to be interpreted correctly.
    pub fn decode_raw_image(
        data: &[u8],
        width: u32,
        height: u32,
        bits_per_component: u8,
        color_space: ImageColorSpace,
    ) -> PDFResult<DecodedImage> {
        let channels = match color_space {
            ImageColorSpace::Gray => 1,
            ImageColorSpace::RGB => 3,
            ImageColorSpace::RGBA => 4,
            ImageColorSpace::CMYK => 4,
            ImageColorSpace::Unknown(n) => n,
        };

        let expected_size = (width as usize) * (height as usize) * (channels as usize) * (bits_per_component as usize) / 8;

        if data.len() < expected_size {
            return Err(PDFError::Generic(format!(
                "Insufficient raw image data: expected at least {} bytes, got {}",
                expected_size, data.len()
            )));
        }

        let metadata = ImageMetadata {
            name: "Raw".to_string(),
            format: ImageFormat::Raw,
            width,
            height,
            bits_per_component,
            color_space: format!("{:?}", color_space),
            has_alpha: color_space == ImageColorSpace::RGBA,
            data_length: Some(data.len()),
        };

        // For raw data, just take the expected amount
        Ok(DecodedImage::new(metadata, data[..expected_size].to_vec(), channels))
    }

    /// Parse color space from PDF object
    pub fn parse_color_space(color_space_obj: &super::PDFObject) -> ImageColorSpace {
        use super::PDFObject;

        match color_space_obj {
            PDFObject::Name(name) => {
                match name.as_str() {
                    "DeviceGray" | "G" => ImageColorSpace::Gray,
                    "DeviceRGB" | "RGB" => ImageColorSpace::RGB,
                    "DeviceCMYK" | "CMYK" => ImageColorSpace::CMYK,
                    _ => ImageColorSpace::Unknown(3), // Default to RGB if unknown
                }
            }
            // For arrays (CalRGB, CalGray, etc.) and other complex color spaces,
            // we need more sophisticated parsing. For now, default to RGB.
            PDFObject::Array(arr) => {
                if let Some(box_obj) = arr.get(0) {
                    if let PDFObject::Name(name) = &**box_obj {
                        match name.as_str() {
                            "CalGray" | "Separation" => ImageColorSpace::Gray,
                            "CalRGB" | "Lab" => ImageColorSpace::RGB,
                            "ICCBased" => {
                                // ICCBased needs to look at the stream's /N parameter
                                // For now, default to RGB
                                ImageColorSpace::RGB
                            }
                            _ => ImageColorSpace::RGB,
                        }
                    } else {
                        ImageColorSpace::RGB
                    }
                } else {
                    ImageColorSpace::RGB
                }
            }
            _ => ImageColorSpace::Unknown(3),
        }
    }

    /// Decode JPEG image using zune-jpeg (hayro's approach)
    fn decode_jpeg(data: &[u8]) -> PDFResult<DecodedImage> {
        #[cfg(feature = "jpeg-decoding")]
        {
            use zune_jpeg::zune_core::options::DecoderOptions;
            use zune_jpeg::zune_core::colorspace::ColorSpace;
            use std::io::Cursor;

            let options = DecoderOptions::default()
                .set_max_width(u16::MAX as usize)
                .set_max_height(u16::MAX as usize);

            let mut decoder = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(data), options);

            // Try to decode headers first
            decoder.decode_headers()
                .map_err(|e| PDFError::Generic(format!("JPEG header decode error: {:?}", e)))?;

            // Get metadata
            let info = decoder.info()
                .ok_or_else(|| PDFError::Generic("Failed to get JPEG info".to_string()))?;

            let width = info.width as u32;
            let height = info.height as u32;

            // Decode full image
            let decoded_data = decoder.decode()
                .map_err(|e| PDFError::Generic(format!("JPEG decode error: {:?}", e)))?;

            let channels = decoded_data.len() / (width as usize * height as usize);
            let color_space = decoder.input_colorspace()
                .unwrap_or(ColorSpace::RGB);

            let metadata = ImageMetadata {
                name: "JPEG".to_string(),
                format: ImageFormat::JPEG,
                width,
                height,
                bits_per_component: 8,
                color_space: format!("{:?}", color_space),
                has_alpha: channels == 4 || channels == 2,
                data_length: Some(data.len()),
            };

            Ok(DecodedImage::new(metadata, decoded_data, channels as u8))
        }

        #[cfg(not(feature = "jpeg-decoding"))]
        {
            Err(PDFError::Unsupported {
                feature: "JPEG decoding not enabled. Enable the 'jpeg-decoding' feature.".to_string()
            })
        }
    }

    /// Decode JPEG2000 image using hayro-jpeg2000
    #[cfg(feature = "advanced-image-formats")]
    fn decode_jpeg2000(data: &[u8]) -> PDFResult<DecodedImage> {
        use hayro_jpeg2000::{Image, DecodeSettings};

        // Try to create and decode the image
        match Image::new(data, &DecodeSettings::default()) {
            Ok(image) => {
                let pixel_data = image.decode()
                    .map_err(|e| PDFError::Generic(format!("JPEG2000 pixel decode error: {:?}", e)))?;

                let metadata = ImageMetadata {
                    name: "JPEG2000".to_string(),
                    format: ImageFormat::JPEG2000,
                    width: image.width(),
                    height: image.height(),
                    bits_per_component: image.original_bit_depth(),
                    color_space: format!("{:?}", image.color_space()),
                    has_alpha: image.has_alpha(),
                    data_length: Some(data.len()),
                };

                let channels = image.color_space().num_channels() + if image.has_alpha() { 1 } else { 0 };
                Ok(DecodedImage::new(metadata, pixel_data, channels))
            }
            Err(e) => Err(PDFError::Generic(format!("JPEG2000 decode error: {:?}", e)))
        }
    }

    /// Decode JBIG2 image using hayro-jbig2
    #[cfg(feature = "advanced-image-formats")]
    fn decode_jbig2(data: &[u8]) -> PDFResult<DecodedImage> {
        use hayro_jbig2::decode;

        // Try to decode the image
        match decode(data) {
            Ok(image) => {
                let metadata = ImageMetadata {
                    name: "JBIG2".to_string(),
                    format: ImageFormat::JBIG2,
                    width: image.width,
                    height: image.height,
                    bits_per_component: 1, // JBIG2 is 1-bit per pixel
                    color_space: "Bi-level".to_string(),
                    has_alpha: false, // JBIG2 doesn't have alpha
                    data_length: Some(data.len()),
                };

                // Convert bool array to bytes (1 bit per pixel)
                let mut pixel_data = Vec::new();
                for chunk in image.data.chunks(8) {
                    let mut byte = 0u8;
                    for (i, &pixel) in chunk.iter().enumerate() {
                        if pixel {
                            byte |= 1 << (7 - i);
                        }
                    }
                    pixel_data.push(byte);
                }

                Ok(DecodedImage::new(metadata, pixel_data, 1))
            }
            Err(e) => Err(PDFError::Generic(format!("JBIG2 decode error: {:?}", e)))
        }
    }

    /// Decode PNG image using the image crate
    #[cfg(feature = "png-decoding")]
    fn decode_png(data: &[u8]) -> PDFResult<DecodedImage> {
        use image::ImageDecoder;

        let cursor = std::io::Cursor::new(data);
        let decoder = image::codecs::png::PngDecoder::new(cursor)
            .map_err(|e| PDFError::Generic(format!("PNG decoder error: {:?}", e)))?;

        let (width, height) = decoder.dimensions();
        let color_type = decoder.color_type();

        let image_data = decoder.decode()
            .map_err(|e| PDFError::Generic(format!("PNG decode error: {:?}", e)))?;

        let channels = color_type.channel_count() as u8;

        let metadata = ImageMetadata {
            name: "PNG".to_string(),
            format: ImageFormat::PNG,
            width: width as u32,
            height: height as u32,
            bits_per_component: 8,
            color_space: format!("{:?}", color_type),
            has_alpha: color_type.has_alpha(),
            data_length: Some(data.len()),
        };

        Ok(DecodedImage::new(metadata, image_data.to_vec(), channels))
    }

    /// Decode PNG image when JPEG decoding is not enabled
    #[cfg(not(feature = "png-decoding"))]
    fn decode_png(_data: &[u8]) -> PDFResult<DecodedImage> {
        Err(PDFError::Unsupported {
            feature: "PNG decoding not enabled. Enable the 'jpeg-decoding' feature.".to_string()
        })
    }

}

/// Extension trait for PDF pages to add image extraction capabilities.
pub trait ImageExtraction {
    /// Extract image metadata without full decoding.
    fn get_image_metadata(&self) -> PDFResult<Vec<ImageMetadata>>;

    /// Extract complete images with full decoding.
    fn extract_images(&self) -> PDFResult<Vec<DecodedImage>>;
}