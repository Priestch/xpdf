//! Form field commands (fields, set-field).

use crate::commands::{Ctx, edit::maybe_save};
use pdf_x_core::core::acroform::{self, FormField};
use pdf_x_core::core::annotation::FormFieldType;
use serde_json::{Value, json};

fn type_name(field_type: &FormFieldType) -> String {
    match field_type {
        FormFieldType::Button => "Button".into(),
        FormFieldType::Text => "Text".into(),
        FormFieldType::Choice => "Choice".into(),
        FormFieldType::Signature => "Signature".into(),
        FormFieldType::Unknown(name) => name.clone(),
    }
}

fn ref_name(r: &pdf_x_core::core::parser::Ref) -> String {
    format!("{} {} R", r.num, r.generation)
}

fn field_to_json(field: &FormField, widget_pages: &Value) -> Value {
    let widgets: Vec<Value> = field
        .widget_refs
        .iter()
        .map(|r| {
            let key = format!("{} {}", r.num, r.generation);
            json!({
                "object": ref_name(r),
                "pages": widget_pages.get(&key).cloned().unwrap_or_else(|| json!([])),
            })
        })
        .collect();
    json!({
        "name": field.fully_qualified_name,
        "type": type_name(&field.field_type),
        "value": field.value,
        "terminal": field.terminal,
        "object": field.field_ref.as_ref().map(ref_name),
        "widgets": widgets,
        "kids": field.kids.iter().map(|k| field_to_json(k, widget_pages)).collect::<Vec<_>>(),
    })
}

/// `fields` — list the AcroForm field tree with widget page positions.
pub fn run_fields(ctx: &mut Ctx) -> Result<Value, String> {
    let fields = ctx
        .doc
        .acroform_fields()
        .map_err(|e| format!("fields failed: {e}"))?;

    // Map each widget ref to its 1-based page numbers so `fields` can show
    // where a field is visible (same join the annotations command does).
    let page_count = ctx
        .doc
        .page_count()
        .map_err(|e| format!("fields failed: {e}"))?;
    let mut widget_pages: std::collections::HashMap<(u32, u32), Vec<usize>> =
        std::collections::HashMap::new();
    for page_index in 0..page_count as usize {
        for (annot_ref, _) in ctx
            .doc
            .annotations_with_refs(page_index)
            .map_err(|e| format!("fields failed: {e}"))?
        {
            if let Some(r) = annot_ref {
                widget_pages
                    .entry((r.num, r.generation))
                    .or_default()
                    .push(page_index + 1);
            }
        }
    }
    let widget_pages = Value::Object(
        widget_pages
            .into_iter()
            .map(|((num, generation), pages)| (format!("{num} {generation}"), json!(pages)))
            .collect(),
    );

    let count = fields.len();
    Ok(json!({
        "count": count,
        "fields": fields.iter().map(|f| field_to_json(f, &widget_pages)).collect::<Vec<_>>(),
    }))
}

/// `set-field <name> <value> [--output <path>]`
pub fn run_set_field(ctx: &mut Ctx, name: &str, value: &str) -> Result<Value, String> {
    ctx.doc
        .set_field_value(name, value)
        .map_err(|e| format!("set-field failed: {e}"))?;
    maybe_save(ctx)?;

    // Report the value the field tree now carries for the name.
    let new_value = ctx
        .doc
        .acroform_fields()
        .ok()
        .and_then(|tree| acroform::find_by_name(&tree, name).and_then(|f| f.value.clone()));
    let (dirty, can_undo, can_redo) = ctx.doc.editor_state();
    Ok(json!({
        "field": name,
        "value": new_value,
        "state": { "dirty": dirty, "can_undo": can_undo, "can_redo": can_redo },
    }))
}
