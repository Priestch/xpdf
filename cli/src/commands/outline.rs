//! `outline` command: document bookmarks.

use crate::commands::Ctx;
use pdf_x_core::core::outline::{OutlineDestination, OutlineItem};
use serde_json::{Value, json};

/// `outline`: the document outline (bookmarks) as a nested tree.
pub fn run(ctx: &mut Ctx) -> Result<Value, String> {
    let doc = &mut *ctx.doc;
    match doc.document_outline_items() {
        Ok(Some(items)) => {
            let children: Vec<Value> = items.iter().map(item_to_value).collect();
            Ok(json!({ "items": children, "count": children.len() }))
        }
        Ok(None) => Ok(json!({ "items": [], "count": 0 })),
        Err(e) => Err(format!("outline unavailable: {e:?}")),
    }
}

fn item_to_value(item: &OutlineItem) -> Value {
    json!({
        "title": item.title,
        "bold": item.bold,
        "italic": item.italic,
        "color": item.color.map(|[r, g, b]| json!([r, g, b])),
        "count": item.count,
        "dest": item.dest.as_ref().map(dest_to_value),
        "children": item.children.iter().map(item_to_value).collect::<Vec<_>>(),
    })
}

fn dest_to_value(dest: &OutlineDestination) -> Value {
    match dest {
        OutlineDestination::Explicit {
            page_index,
            dest_type,
        } => json!({
            "type": "explicit",
            "page": page_index + 1,
            "page_index": page_index,
            "dest_type": format!("{dest_type:?}"),
        }),
        OutlineDestination::Named(name) => json!({
            "type": "named",
            "name": name,
        }),
        OutlineDestination::URL(url) => json!({
            "type": "url",
            "url": url,
        }),
        OutlineDestination::GoToRemote {
            url,
            dest,
            new_window,
        } => json!({
            "type": "goto_remote",
            "url": url,
            "dest": dest,
            "new_window": new_window,
        }),
    }
}
