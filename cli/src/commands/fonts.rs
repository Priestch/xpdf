//! `fonts` command: per-page font details.

use crate::commands::Ctx;
use pdf_x_core::core::font::{Font, FontDict};
use serde_json::{Value, json};

/// `fonts`: list fonts used on the selected pages with type/encoding details.
pub fn run(ctx: &mut Ctx) -> Result<Value, String> {
    let indices = ctx.page_indices();
    let doc = &mut *ctx.doc;

    let mut pages = Vec::new();
    let mut total = 0usize;

    for idx in indices {
        let page = doc
            .get_page(idx)
            .map_err(|e| format!("page {} unavailable: {e:?}", idx + 1))?;
        let fonts = page
            .get_fonts(doc.doc_store_mut())
            .map_err(|e| format!("page {} fonts unavailable: {e:?}", idx + 1))?;

        let mut page_fonts = Vec::new();
        for (name, font_ref) in fonts {
            let font_obj = match doc.xref_mut().fetch_if_ref(&font_ref) {
                Ok(obj) => obj,
                Err(e) => {
                    page_fonts.push(json!({
                        "name": name,
                        "error": format!("unresolved: {e:?}"),
                    }));
                    continue;
                }
            };

            // Full font load (encoding, ToUnicode, embedded program) when possible;
            // fall back to the raw dictionary so the listing still succeeds.
            let loaded = Font::new((*font_obj).clone(), doc.doc_store_mut());
            let (font_type, base_font, has_to_unicode, has_embedded) = match loaded {
                Ok(font) => (
                    format!("{:?}", font.font_type()),
                    font.base_font().to_string(),
                    font.has_to_unicode(),
                    font.has_embedded_font(),
                ),
                Err(_) => match FontDict::from_pdf_object(&font_obj) {
                    Ok(dict) => (
                        format!("{:?}", dict.font_type),
                        dict.base_font.clone(),
                        dict.to_unicode.is_some(),
                        dict.font_descriptor.is_some(),
                    ),
                    Err(e) => {
                        page_fonts.push(json!({
                            "name": name,
                            "error": format!("invalid font dictionary: {e:?}"),
                        }));
                        continue;
                    }
                },
            };

            page_fonts.push(json!({
                "name": name,
                "font_type": font_type,
                "base_font": base_font,
                "has_to_unicode": has_to_unicode,
                "has_embedded_font": has_embedded,
            }));
        }

        total += page_fonts.len();
        pages.push(json!({ "page": idx + 1, "fonts": page_fonts }));
    }

    Ok(json!({ "pages": pages, "total": total }))
}
