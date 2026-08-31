//! `text` command: text extraction, optionally with per-item positions.

use crate::commands::Ctx;
use serde_json::{Value, json};

/// `text [--positions]`: extract text from the selected pages.
pub fn run(ctx: &mut Ctx, positions: bool) -> Result<Value, String> {
    let indices = ctx.page_indices();
    let doc = &mut *ctx.doc;

    let mut pages = Vec::new();
    let mut total_chars = 0usize;

    for idx in indices {
        if positions {
            let items = doc
                .extract_text_from_page(idx)
                .map_err(|e| format!("page {} text extraction failed: {e:?}", idx + 1))?;
            let item_count = items.len();
            let items_json: Vec<Value> = items
                .iter()
                .map(|item| {
                    json!({
                        "text": item.text,
                        "font_name": item.font_name,
                        "font_size": item.font_size,
                        "position": item.position.map(|(x, y)| json!([x, y])),
                        "rendering_mode": item.rendering_mode,
                    })
                })
                .collect();
            total_chars += items_json.len();
            pages.push(json!({
                "page": idx + 1,
                "item_count": item_count,
                "items": items_json,
            }));
        } else {
            let text = doc
                .extract_text_from_page_as_string(idx)
                .map_err(|e| format!("page {} text extraction failed: {e:?}", idx + 1))?;
            total_chars += text.len();
            pages.push(json!({ "page": idx + 1, "text": text }));
        }
    }

    Ok(json!({ "pages": pages, "total_characters": total_chars }))
}
