//! `info`, `stats`, `version`, and `page-sizes` commands.

use crate::commands::Ctx;
use pdf_x_core::PDFObject;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// `info` / `all`: document metadata and basic facts.
pub fn run(ctx: &mut Ctx) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    let page_count = doc
        .page_count()
        .map_err(|e| format!("page count unavailable: {e:?}"))?;
    let version = doc.pdf_version().ok();
    let encrypted = doc.is_encrypted();

    let document_info = match doc.document_info() {
        Ok(Some(info)) => Some(info_dict_to_value(&info)),
        Ok(None) => None,
        // A broken /Info object shouldn't take down the whole listing.
        Err(_) => None,
    };

    Ok(json!({
        "page_count": page_count,
        "pdf_version": version,
        "file_size": ctx.file_size,
        "linearized": doc.is_linearized(),
        "encrypted": encrypted,
        "document_info": document_info,
    }))
}

/// `stats`: summary statistics.
pub fn run_stats(ctx: &mut Ctx) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    let page_count = doc.page_count().unwrap_or(0);
    let xref_entries = doc.xref().len();
    let file_size = ctx.file_size;
    let avg_page_size = if page_count > 0 {
        file_size.map(|s| s as f64 / page_count as f64)
    } else {
        None
    };
    let version = doc.pdf_version().ok();

    let unique_fonts = count_unique_fonts(doc);

    Ok(json!({
        "page_count": page_count,
        "xref_entries": xref_entries,
        "file_size": file_size,
        "avg_page_size": avg_page_size,
        "linearized": doc.is_linearized(),
        "pdf_version": version,
        "unique_fonts": unique_fonts,
    }))
}

/// `version`: PDF header version.
pub fn run_version(ctx: &mut Ctx) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    let version = doc
        .pdf_version()
        .map_err(|e| format!("PDF version unavailable: {e:?}"))?;
    Ok(json!({ "pdf_version": version }))
}

/// `page-sizes`: MediaBox dimensions for the selected pages.
pub fn run_page_sizes(ctx: &mut Ctx) -> Result<Value, String> {
    let indices = ctx.page_indices();
    let doc = &mut *ctx.doc;

    let mut pages = Vec::new();
    for idx in indices {
        let page = doc
            .get_page(idx)
            .map_err(|e| format!("page {} unavailable: {e:?}", idx + 1))?;
        let media_box = doc
            .get_media_box(&page)
            .map_err(|e| format!("page {} MediaBox unavailable: {e:?}", idx + 1))?;

        let values: Vec<f64> = match &*media_box {
            PDFObject::Array(arr) => arr
                .iter()
                .filter_map(|o| {
                    if let PDFObject::Number(n) = o.as_ref() {
                        Some(*n)
                    } else {
                        None
                    }
                })
                .collect(),
            _ => return Err(format!("page {} MediaBox is not a numeric array", idx + 1)),
        };

        if values.len() < 4 {
            return Err(format!("page {} has an invalid MediaBox", idx + 1));
        }

        let (width, height) = (values[2] - values[0], values[3] - values[1]);
        pages.push(json!({
            "page": idx + 1,
            "width": width,
            "height": height,
            "width_in": width / 72.0,
            "height_in": height / 72.0,
            "media_box": values,
        }));
    }

    Ok(json!({ "pages": pages, "count": pages.len() }))
}

/// Decode PDF string bytes (literal or hex) to a displayable String.
///
/// Per PDF spec §7.9.2, string values in the /Info dictionary may be
/// UTF-16BE with a BOM (\xfe\xff). Hex strings with that BOM are decoded
/// as UTF-16BE; everything else falls back to lossy UTF-8.
fn decode_pdf_string_bytes(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xfe && bytes[1] == 0xff {
        // UTF-16BE BOM — decode 16-bit code units, dropping unpaired surrogates.
        let u16_iter = bytes[2..].chunks_exact(2).filter_map(|chunk| {
            Some(u16::from_be_bytes([chunk[0], chunk[1]]))
        });
        char::decode_utf16(u16_iter)
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Flatten an /Info dictionary into plain JSON strings.
fn info_dict_to_value(info: &PDFObject) -> Value {
    match info {
        PDFObject::Dictionary(dict) => {
            let mut map = serde_json::Map::new();
            let mut keys: Vec<_> = dict.keys().collect();
            keys.sort();
            for key in keys {
                let value = &dict[key];
                let rendered = match value {
                    PDFObject::String(s) => Value::String(decode_pdf_string_bytes(s)),
                    PDFObject::HexString(s) => {
                        Value::String(decode_pdf_string_bytes(s))
                    }
                    PDFObject::Name(n) => Value::String(format!("/{n}")),
                    other => crate::jsonutil::pdf_object_to_value(other),
                };
                map.insert(key.clone(), rendered);
            }
            Value::Object(map)
        }
        other => crate::jsonutil::pdf_object_to_value(other),
    }
}

/// Union of font resource names across all pages.
fn count_unique_fonts(doc: &mut pdf_x_core::PDFDocument) -> usize {
    let page_count = match doc.page_count() {
        Ok(count) => count,
        Err(_) => return 0,
    };

    let mut names: BTreeSet<String> = BTreeSet::new();
    for page_num in 0..page_count {
        let Ok(page) = doc.get_page(page_num as usize) else {
            continue;
        };
        let Ok(fonts) = page.get_fonts(doc.doc_store_mut()) else {
            continue;
        };
        for (name, _) in fonts {
            names.insert(name);
        }
    }
    names.len()
}
