//! Type3 glyph programs: char procs, encodings, and per-code metrics.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct Type3Glyph {
    pub(super) data: Vec<u8>,
    pub(super) is_shape_glyph: bool,
    pub(super) resources: Option<PDFObject>,
}

#[derive(Debug, Clone)]
pub(super) struct Type3Font {
    pub(super) encoding: Type3Encoding,
    pub(super) char_procs: HashMap<String, Type3Glyph>,
    pub(super) resources: Option<PDFObject>,
    pub(super) font_matrix: [f64; 6],
    pub(super) first_char: u32,
    pub(super) widths: Vec<f64>,
    pub(super) default_width: f64,
}

#[derive(Debug, Clone)]
pub(super) enum Type3Encoding {
    Names(Vec<Option<String>>),
    Text(Encoding),
}

impl Type3Encoding {
    pub(super) fn glyph_name(&self, code: u8) -> Option<String> {
        match self {
            Type3Encoding::Names(names) => names.get(code as usize).and_then(Clone::clone),
            Type3Encoding::Text(encoding) => Some(encoding.char_to_unicode(code).to_string()),
        }
    }
}

/// Runtime toggle for verbose text spacing traces (Tw/Tc/TJ/Tj).
///
/// This is intentionally global so frontends (like Tauri) can enable tracing
/// around a render call without rebuilding with special features.
impl Type3Font {
    pub(super) fn width_for_code(&self, code: u8) -> f64 {
        let code = code as u32;
        let width = if code < self.first_char {
            self.default_width
        } else {
            let idx = (code - self.first_char) as usize;
            self.widths.get(idx).copied().unwrap_or(self.default_width)
        };

        width * self.font_matrix[0]
    }
}
