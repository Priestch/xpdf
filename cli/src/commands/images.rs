//! `images` command: image XObject metadata per page.

use crate::commands::Ctx;
use serde_json::{Value, json};

/// `images`: list image XObjects on the selected pages.
pub fn run(ctx: &mut Ctx) -> Result<Value, String> {
    let indices = ctx.page_indices();
    let doc = &mut *ctx.doc;

    let mut pages = Vec::new();
    let mut total = 0usize;

    for idx in indices {
        let page = doc
            .get_page(idx)
            .map_err(|e| format!("page {} unavailable: {e:?}", idx + 1))?;
        let images = page
            .get_image_metadata(doc.doc_store_mut())
            .map_err(|e| format!("page {} images unavailable: {e:?}", idx + 1))?;

        let count = images.len();
        let items: Vec<Value> = images
            .iter()
            .map(|img| {
                json!({
                    "name": img.name,
                    "format": format!("{:?}", img.format),
                    "width": img.width,
                    "height": img.height,
                    "bits_per_component": img.bits_per_component,
                    "color_space": img.color_space,
                    "has_alpha": img.has_alpha,
                    "data_length": img.data_length,
                })
            })
            .collect();

        total += count;
        pages.push(json!({ "page": idx + 1, "images": items }));
    }

    Ok(json!({ "pages": pages, "total": total }))
}
