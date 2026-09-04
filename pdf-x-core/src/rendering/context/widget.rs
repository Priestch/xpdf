//! Limited appearance synthesis for Text form widgets.
//!
//! A Text widget carrying `/V` but **no** `/AP /N` has nothing to paint —
//! this module builds a small Form XObject on the fly (background + value
//! text) so saved form values stay visible in the viewer. Widgets that DO
//! have an appearance keep it; it is authoritative.
//!
//! Scope (v1, deliberately narrow — pdf.js's full
//! `TextWidgetAnnotation._getAppearance` (annotation.js ≈:2860) does real
//! font metrics, multiline layout and rich text):
//! - the text is always drawn with built-in Helvetica; the `/DA` font name is
//!   honored only as the resource name, `/DA` size 0 (auto) becomes 12;
//! - `/Q` alignment uses a Helvetica average-width approximation
//!   (0.6 em/char) instead of measured widths;
//! - multiline fields draw one line; comb fields (`/Ff` bit 25 + `/MaxLen`)
//!   place one character per cell;
//! - `/MK /BG` is painted as the background; `/MK /BC`, `/RC` rich text and
//!   borders are ignored.

use std::collections::HashMap;

use crate::core::parser::PDFObject;

/// Average character width of Helvetica as a fraction of the font size.
/// Used only where real metrics are out of scope (see module docs).
const HELVETICA_AVG_WIDTH: f64 = 0.6;

/// Horizontal padding inside the field rectangle.
const PADDING: f64 = 1.0;

/// Builds a synthesized Form XObject `(dict, data)` for a Text widget
/// without an appearance, or `None` when the annotation is not a text
/// widget with a text value (checkbox/radio values are names and always
/// own their `/AP` state maps).
pub(crate) fn synthesize_text_widget_appearance(
    annot_dict: &HashMap<String, PDFObject>,
) -> Option<(HashMap<String, PDFObject>, Vec<u8>)> {
    // Text widgets only: the value must be a text string.
    let value = match annot_dict.get("V") {
        Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
            String::from_utf8_lossy(bytes).to_string()
        }
        _ => return None,
    };
    if value.is_empty() {
        return None;
    }

    let (rect_w, rect_h) = match annot_dict.get("Rect").and_then(parse_four_numbers) {
        Some([llx, lly, urx, ury]) => ((urx - llx).abs(), (ury - lly).abs()),
        _ => return None,
    };

    // Field flags: comb layout is /Ff bit 25 (1 << 24), spec §12.7.3.1.
    let field_flags = match annot_dict.get("Ff") {
        Some(PDFObject::Number(n)) => *n as u32,
        _ => 0,
    };
    let max_len = match annot_dict.get("MaxLen") {
        Some(PDFObject::Number(n)) => *n as usize,
        _ => 0,
    };

    // /DA "<font name> <size> Tf" — size 0 (auto) and missing /DA → 12.
    let (font_name, font_size) = parse_da(annot_dict.get("DA"));
    let font_size = if font_size > 0.0 { font_size } else { 12.0 };

    let mut ops = String::new();

    // /MK /BG background fill.
    if let Some(color) = mk_color(annot_dict.get("MK").and_then(|mk| mk.as_dictionary())) {
        ops.push_str(&format!(
            "q {} {} {} rg 0 0 {rect_w} {rect_h} re f Q\n",
            color[0], color[1], color[2]
        ));
    }

    // Text layout: baseline vertically centered; horizontal placement by /Q.
    let text = if field_flags & (1 << 24) != 0 && max_len > 0 {
        // Comb: one character per evenly divided cell.
        let cell = rect_w / max_len as f64;
        let chars: Vec<char> = value.chars().take(max_len).collect();
        chars
            .iter()
            .enumerate()
            .map(|(i, ch)| {
                let x = PADDING + i as f64 * cell;
                format!(
                    "BT /{font_name} {font_size} Tf {x} {} Td ({}) Tj ET\n",
                    baseline_y(rect_h, font_size),
                    escape(ch.to_string())
                )
            })
            .collect()
    } else {
        let width = value.chars().count() as f64 * font_size * HELVETICA_AVG_WIDTH;
        let quadding = match annot_dict.get("Q") {
            Some(PDFObject::Number(n)) => *n as i64,
            _ => 0,
        };
        let x = match quadding {
            1 => ((rect_w - width) / 2.0).max(PADDING),
            2 => (rect_w - width - PADDING).max(PADDING),
            _ => PADDING,
        };
        format!(
            "BT /{font_name} {font_size} Tf {x} {} Td ({}) Tj ET\n",
            baseline_y(rect_h, font_size),
            escape(value)
        )
    };
    ops.push_str(&text);

    let mut dict = HashMap::new();
    dict.insert("Type".into(), PDFObject::Name("XObject".into()));
    dict.insert("Subtype".into(), PDFObject::Name("Form".into()));
    dict.insert(
        "BBox".into(),
        PDFObject::Array(
            [0.0, 0.0, rect_w, rect_h]
                .iter()
                .map(|n| Box::new(PDFObject::Number(*n)))
                .collect(),
        ),
    );
    // The synthesized resource maps the /DA name to built-in Helvetica.
    let mut helv = HashMap::new();
    helv.insert("Type".into(), PDFObject::Name("Font".into()));
    helv.insert("Subtype".into(), PDFObject::Name("Type1".into()));
    helv.insert("BaseFont".into(), PDFObject::Name("Helvetica".into()));
    let mut fonts = HashMap::new();
    fonts.insert(font_name, PDFObject::Dictionary(helv));
    let mut resources = HashMap::new();
    resources.insert("Font".into(), PDFObject::Dictionary(fonts));
    dict.insert("Resources".into(), PDFObject::Dictionary(resources));

    Some((dict, ops.into_bytes()))
}

/// Vertically centers the baseline for a given font size (approximate:
/// treats the size as the full line box).
fn baseline_y(rect_h: f64, font_size: f64) -> f64 {
    ((rect_h - font_size) / 2.0).max(PADDING)
}

/// Parses `/DA` into `(font name, size)` — the first `name size Tf`
/// sequence. Missing/malformed /DA yields `("Helv", 12.0)`.
fn parse_da(da: Option<&PDFObject>) -> (String, f64) {
    let bytes = match da {
        Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => bytes,
        _ => return ("Helv".to_string(), 12.0),
    };
    let text = String::from_utf8_lossy(bytes);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    for window in tokens.windows(3) {
        if window[2] == "Tf" {
            let name = window[0].trim_start_matches('/');
            if let Ok(size) = window[1].parse::<f64>() {
                return (name.to_string(), size);
            }
        }
    }
    ("Helv".to_string(), 12.0)
}

/// Reads `/MK /BG` as an RGB triple (gray scales to r=g=b; CMYK and the
/// rest fall back to white — i.e. no background beyond paper).
fn mk_color(mk: Option<&HashMap<String, PDFObject>>) -> Option<[f64; 3]> {
    let bg = mk?.get("BG")?;
    let numbers: Vec<f64> = match bg {
        PDFObject::Array(items) => items
            .iter()
            .filter_map(|item| match item.as_ref() {
                PDFObject::Number(n) => Some(*n),
                _ => None,
            })
            .collect(),
        PDFObject::Number(n) => vec![*n],
        _ => Vec::new(),
    };
    match numbers.as_slice() {
        [g] => Some([*g, *g, *g]),
        [r, g, b] => Some([*r, *g, *b]),
        // Naive CMYK → RGB is wrong (see CLAUDE.md: pdf.js colorspace
        // polynomial); skip the background instead of drawing a bad color.
        _ => None,
    }
}

/// Escapes PDF string literals for inclusion in content ops.
fn escape(text: String) -> String {
    text.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Parses a 4-number array (Rect). Local copy to keep this module
/// self-contained (the xobject.rs helper is not exported).
fn parse_four_numbers(obj: &PDFObject) -> Option<[f64; 4]> {
    let PDFObject::Array(items) = obj else {
        return None;
    };
    if items.len() != 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (i, item) in items.iter().enumerate() {
        out[i] = match item.as_ref() {
            PDFObject::Number(n) => *n,
            _ => return None,
        };
    }
    Some(out)
}
