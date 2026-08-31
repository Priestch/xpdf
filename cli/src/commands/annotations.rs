//! `annotations` command: per-page annotation listing.

use crate::commands::Ctx;
use pdf_x_core::core::annotation::{AnnotationData, LinkAction};
use serde_json::{Value, json};

/// `annotations`: list annotations on the selected pages.
pub fn run(ctx: &mut Ctx) -> Result<Value, String> {
    let indices = ctx.page_indices();
    let doc = &mut *ctx.doc;

    let mut pages = Vec::new();
    let mut total = 0usize;

    for idx in indices {
        let page = doc
            .get_page(idx)
            .map_err(|e| format!("page {} unavailable: {e:?}", idx + 1))?;
        let annotations = page
            .extract_annotations(doc.doc_store_mut())
            .map_err(|e| format!("page {} annotations unavailable: {e:?}", idx + 1))?;

        let count = annotations.len();
        let items: Vec<Value> = annotations
            .iter()
            .map(|annot| {
                json!({
                    "type": format!("{:?}", annot.annotation_type),
                    "rect": annot.rect,
                    "contents": annot.contents,
                    "color": annot.color,
                    "data": data_to_value(&annot.data),
                })
            })
            .collect();

        total += count;
        pages.push(json!({ "page": idx + 1, "annotations": items }));
    }

    Ok(json!({ "pages": pages, "total": total }))
}

fn data_to_value(data: &AnnotationData) -> Value {
    match data {
        AnnotationData::None => Value::Null,
        AnnotationData::Link(link) => match &link.action {
            LinkAction::URI { url, is_map } => json!({
                "kind": "link",
                "action": "uri",
                "url": url,
                "is_map": is_map,
            }),
            LinkAction::GoTo { page_index, dest } => json!({
                "kind": "link",
                "action": "goto",
                "page": page_index + 1,
                "page_index": page_index,
                "dest": format!("{dest:?}"),
            }),
            LinkAction::GoToNamed { name } => json!({
                "kind": "link",
                "action": "goto_named",
                "name": name,
            }),
            other => json!({
                "kind": "link",
                "action": format!("{other:?}"),
            }),
        },
        AnnotationData::Text(text) => json!({
            "kind": "text",
            "open": text.open,
            "icon": text.icon,
            "state": text.state,
            "state_model": text.state_model,
        }),
        AnnotationData::Widget(widget) => json!({
            "kind": "widget",
            "field_type": format!("{:?}", widget.field_type),
            "field_name": widget.field_name,
            "field_value": widget.field_value,
            "default_value": widget.default_value,
            "export_value": widget.export_value,
        }),
        other => json!({ "kind": format!("{other:?}") }),
    }
}
