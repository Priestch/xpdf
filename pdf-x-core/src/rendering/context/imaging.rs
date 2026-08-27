//! Image color-space handling and decode sinks (JPX options, CCITT bitmap sink).

use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) struct ImageColorSpaceInfo {
    pub(super) name: Option<&'static str>,
    pub(super) components: Option<u8>,
}

#[cfg(feature = "advanced-image-formats")]
#[derive(Debug, Clone, Copy)]
pub(super) struct JpxDecodeOptions {
    pub(super) pdf_components: Option<u8>,
    pub(super) smask_in_data: bool,
}

#[cfg(feature = "advanced-image-formats")]
pub(super) fn decode_jpx(
    data: &[u8],
    options: JpxDecodeOptions,
) -> Option<(Vec<u8>, u32, u32, u8, bool)> {
    use hayro_jpeg2000::ColorSpace;
    use image::ImageDecoder;

    let settings = hayro_jpeg2000::DecodeSettings {
        resolve_palette_indices: false,
        strict: false,
        target_resolution: None,
    };
    let image = hayro_jpeg2000::Image::new(data, &settings).ok()?;
    let width = image.width();
    let height = image.height();
    let color_components = match image.color_space() {
        ColorSpace::Gray => 1,
        ColorSpace::RGB => 3,
        ColorSpace::CMYK => 4,
        ColorSpace::Unknown { num_channels } => *num_channels,
        ColorSpace::Icc { num_channels, .. } => *num_channels,
    };

    if let Some(pdf_components) = options.pdf_components {
        let bpc = image.original_bit_depth();
        let has_alpha = image.has_alpha();
        let decoded = image.decode().ok()?;

        if pdf_components != color_components
            && !(pdf_components == 3 && color_components == 3 && has_alpha)
        {
            return None;
        }

        let decoded = if has_alpha && pdf_components == 3 && !options.smask_in_data {
            let channels = color_components as usize + 1;
            let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
            for sample in decoded.chunks_exact(channels) {
                rgb.extend_from_slice(&sample[..3]);
            }
            rgb
        } else if bpc > 8 {
            decoded.chunks_exact(2).map(|pair| pair[0]).collect()
        } else {
            decoded
        };

        return Some((
            decoded,
            width,
            height,
            8,
            has_alpha && options.smask_in_data,
        ));
    }

    let has_alpha = matches!(
        image.color_type(),
        image::ColorType::La8 | image::ColorType::Rgba8
    );
    let total_bytes = image.total_bytes().try_into().ok()?;
    let mut decoded = vec![0; total_bytes];
    image.read_image(&mut decoded).ok()?;

    Some((decoded, width, height, 8, has_alpha))
}

/// Decode a PDF-embedded JBIG2 image stream.
///
/// Uses hayro-jbig2's PDF.js-style decoder (bare segments + optional globals,
/// including SymbolDictionary / TextRegion). Falls back to the standalone
/// file decoder when the input carries a JBIG2 file header.
///
/// Source: hayro-jbig2 `pdf_stream.rs` ← PDF.js `core/jbig2.js`.
#[cfg(feature = "advanced-image-formats")]
#[cfg(feature = "hayro-ccitt")]
pub(super) struct BitmapSink {
    pub(super) bytes: Vec<u8>,
}

#[cfg(feature = "hayro-ccitt")]
impl BitmapSink {
    pub(super) fn new() -> Self {
        Self { bytes: Vec::new() }
    }
    pub(super) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(feature = "hayro-ccitt")]
impl hayro_ccitt::Decoder for BitmapSink {
    fn push_byte(&mut self, byte: u8) {
        self.bytes.push(byte);
    }
    fn push_bytes(&mut self, byte: u8, count: usize) {
        self.bytes.extend(std::iter::repeat_n(byte, count));
    }
    fn next_line(&mut self) {}
}
