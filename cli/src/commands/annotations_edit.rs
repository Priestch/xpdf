//! Annotation edit commands (add-annotation, delete-annotation, edit-annotation).

use crate::commands::{Ctx, edit::maybe_save};
use pdf_x_core::core::delta::{MarkupKind, QuadPoints};
use serde_json::{Value, json};

/// Resolve a `--kind` name, applying pdf.js's per-kind default colors
/// (Highlight yellow, the rest black) when `--color` is absent.
fn resolve_kind(name: &str, color: Option<[f64; 3]>) -> Result<(MarkupKind, [f64; 3]), String> {
    let (kind, default_color) = match name {
        "highlight" => (MarkupKind::Highlight, [1.0, 1.0, 0.0]),
        "underline" => (MarkupKind::Underline, [0.0, 0.0, 0.0]),
        "squiggly" => (MarkupKind::Squiggly, [0.0, 0.0, 0.0]),
        "strikeout" => (MarkupKind::StrikeOut, [0.0, 0.0, 0.0]),
        other => return Err(format!("unknown annotation kind: {other}")),
    };
    Ok((kind, color.unwrap_or(default_color)))
}

fn state_json(ctx: &mut Ctx) -> Result<Value, String> {
    let (dirty, can_undo, can_redo) = ctx.doc.editor_state();
    Ok(json!({ "dirty": dirty, "can_undo": can_undo, "can_redo": can_redo }))
}

/// `add-annotation <P> --kind <K> (--rect x1,y1,x2,y2 | --quadpoints q...) [--color r,g,b]`
pub fn run_add_annotation(
    ctx: &mut Ctx,
    page_index: usize,
    kind_name: &str,
    quads: &[QuadPoints],
    color: Option<[f64; 3]>,
) -> Result<Value, String> {
    let (kind, color) = resolve_kind(kind_name, color)?;
    ctx.doc
        .add_markup(page_index, kind, quads.to_vec(), color)
        .map_err(|e| format!("add-annotation failed: {e}"))?;
    maybe_save(ctx)?;
    Ok(json!({
        "page": page_index + 1,
        "kind": kind.pdf_name(),
        "quads": quads.len(),
        "color": color,
        "state": state_json(ctx)?,
    }))
}

/// `delete-annotation <P> --object <N> [--output <path>]`
pub fn run_delete_annotation(
    ctx: &mut Ctx,
    page_index: usize,
    object: u32,
) -> Result<Value, String> {
    ctx.doc
        .delete_annotation(page_index, object)
        .map_err(|e| format!("delete-annotation failed: {e}"))?;
    maybe_save(ctx)?;
    Ok(json!({
        "page": page_index + 1,
        "object": object,
        "state": state_json(ctx)?,
    }))
}

/// `edit-annotation <P> --object <N> [--color r,g,b] [--contents T] [--output <path>]`
pub fn run_edit_annotation(
    ctx: &mut Ctx,
    page_index: usize,
    object: u32,
    color: Option<[f64; 3]>,
    contents: Option<String>,
) -> Result<Value, String> {
    ctx.doc
        .edit_annotation(page_index, object, color, contents.clone())
        .map_err(|e| format!("edit-annotation failed: {e}"))?;
    maybe_save(ctx)?;
    Ok(json!({
        "page": page_index + 1,
        "object": object,
        "color": color,
        "contents": contents,
        "state": state_json(ctx)?,
    }))
}
