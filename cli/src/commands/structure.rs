//! Structure commands: `xref`, `catalog`, `trailer`, `pages`, `object`, `stream`.

use crate::commands::Ctx;
use crate::jsonutil::pdf_object_to_value;
use pdf_x_core::{PDFObject, XRefEntry};
use serde_json::{Value, json};

/// `xref`: cross-reference table entries.
pub fn run_xref(ctx: &mut Ctx) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    let xref = doc.xref();

    let mut entries = Vec::with_capacity(xref.len());
    for num in 0..xref.len() {
        let Some(entry) = xref.get_entry(num as u32) else {
            continue;
        };
        let value = match entry {
            XRefEntry::Free {
                next_free,
                generation,
            } => json!({
                "num": num,
                "type": "free",
                "next_free": next_free,
                "generation": generation,
            }),
            XRefEntry::Uncompressed { offset, generation } => json!({
                "num": num,
                "type": "uncompressed",
                "offset": offset,
                "generation": generation,
            }),
            XRefEntry::Compressed {
                obj_stream_num,
                index,
            } => json!({
                "num": num,
                "type": "compressed",
                "obj_stream_num": obj_stream_num,
                "index": index,
            }),
        };
        entries.push(value);
    }

    let total = entries.len();
    Ok(json!({ "total": total, "entries": entries }))
}

/// `catalog`: the document catalog object.
pub fn run_catalog(ctx: &mut Ctx) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    match doc.catalog() {
        Some(catalog) => Ok(json!({ "catalog": pdf_object_to_value(catalog) })),
        None => Err("no catalog found".to_string()),
    }
}

/// `trailer`: the trailer dictionary.
pub fn run_trailer(ctx: &mut Ctx) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    match doc.xref().trailer() {
        Some(trailer) => Ok(json!({ "trailer": pdf_object_to_value(trailer) })),
        None => Err("no trailer found".to_string()),
    }
}

/// `pages`: the /Pages dictionary.
pub fn run_pages(ctx: &mut Ctx) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    let pages = doc
        .pages_dict()
        .map_err(|e| format!("pages dictionary unavailable: {e:?}"))?;
    Ok(json!({ "pages": pdf_object_to_value(&pages) }))
}

/// `object <N>`: a specific indirect object.
pub fn run_object(ctx: &mut Ctx, num: u32) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    let obj = doc
        .xref_mut()
        .fetch(num, 0)
        .map_err(|e| format!("cannot fetch object {num}: {e:?}"))?;
    Ok(json!({
        "object": num,
        "generation": 0,
        "value": pdf_object_to_value(&obj),
    }))
}

/// `stream <N>`: raw (still filter-encoded) stream bytes, for byte-exact diffs.
pub fn run_stream(ctx: &mut Ctx, num: u32) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    let obj = doc
        .xref_mut()
        .fetch(num, 0)
        .map_err(|e| format!("cannot fetch object {num}: {e:?}"))?;

    match &*obj {
        PDFObject::Stream { dict, data } => Ok(json!({
            "object": num,
            "generation": 0,
            "length": data.len(),
            "dict": pdf_object_to_value(&PDFObject::Dictionary(dict.clone())),
            "bytes_hex": data.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        })),
        _ => Err(format!("object {num} is not a stream")),
    }
}
