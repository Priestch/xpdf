//! Edit commands (rotate-page, set-info, delete-page, move-page, save).

use crate::commands::Ctx;
use serde_json::{Value, json};

/// Save the current delta as an incremental update appended to the original file.
fn save_to(ctx: &mut Ctx, output_path: &str) -> Result<(), String> {
    let (dirty, _, _) = ctx.doc.editor_state();
    if !dirty {
        return Err("No edits to save".into());
    }
    let update = ctx
        .doc
        .incremental_update(ctx.original_bytes)
        .map_err(|e| format!("save failed: {e}"))?;
    let mut out = ctx.original_bytes.to_vec();
    out.extend_from_slice(&update);
    std::fs::write(output_path, &out)
        .map_err(|e| format!("failed to write {output_path}: {e}"))?;
    Ok(())
}

fn maybe_save(ctx: &mut Ctx) -> Result<(), String> {
    let Some(path) = ctx.output_path.take() else {
        return Ok(());
    };
    let result = save_to(ctx, &path);
    ctx.output_path = Some(path);
    result
}

/// `rotate-page <page> --angle <degrees> [--output <path>]`
pub fn run_rotate_page(
    ctx: &mut Ctx,
    page_index: usize,
    degrees: u16,
) -> Result<Value, String> {
    ctx.doc
        .rotate_page(page_index, degrees)
        .map_err(|e| format!("rotate failed: {e}"))?;
    let (dirty, can_undo, can_redo) = ctx.doc.editor_state();
    maybe_save(ctx)?;
    Ok(json!({
        "page": page_index + 1,
        "angle": degrees,
        "dirty": dirty,
        "can_undo": can_undo,
        "can_redo": can_redo,
    }))
}

/// `set-info --title "..." --author "..." [--output <path>]`
pub fn run_set_info(
    ctx: &mut Ctx,
    entries: Vec<(String, Option<String>)>,
) -> Result<Value, String> {
    let pdf_entries: Vec<_> = entries
        .into_iter()
        .map(|(key, val)| {
            let pdf_val = match val {
                Some(s) if s.is_empty() => None,
                Some(s) => Some(pdf_x_core::core::delta::SetInfoCommand::encode_info_string(&s)),
                None => None,
            };
            (key, pdf_val)
        })
        .collect();
    ctx.doc
        .set_info(pdf_entries)
        .map_err(|e| format!("set-info failed: {e}"))?;
    let (dirty, can_undo, can_redo) = ctx.doc.editor_state();
    maybe_save(ctx)?;
    Ok(json!({
        "dirty": dirty,
        "can_undo": can_undo,
        "can_redo": can_redo,
    }))
}

/// `delete-page <page> [--output <path>]`
pub fn run_delete_page(
    ctx: &mut Ctx,
    page_index: usize,
) -> Result<Value, String> {
    ctx.doc
        .delete_page(page_index)
        .map_err(|e| format!("delete failed: {e}"))?;
    let (dirty, can_undo, can_redo) = ctx.doc.editor_state();
    maybe_save(ctx)?;
    Ok(json!({
        "page": page_index + 1,
        "dirty": dirty,
        "can_undo": can_undo,
        "can_redo": can_redo,
    }))
}

/// `move-page <page> --to <page> [--output <path>]` — final-position semantics.
pub fn run_move_page(ctx: &mut Ctx, from_index: usize, to_index: usize) -> Result<Value, String> {
    ctx.doc
        .move_page(from_index, to_index)
        .map_err(|e| format!("move failed: {e}"))?;
    let (dirty, can_undo, can_redo) = ctx.doc.editor_state();
    maybe_save(ctx)?;
    let page_count = ctx
        .doc
        .page_count()
        .map_err(|e| format!("page count failed: {e}"))?;
    Ok(json!({
        "from": from_index + 1,
        "to": to_index + 1,
        "page_count": page_count,
        "dirty": dirty,
        "can_undo": can_undo,
        "can_redo": can_redo,
    }))
}

/// `replace-text <page> --find <text> --replace <text> [--output <path>]`
pub fn run_replace_text(
    ctx: &mut Ctx,
    page_index: usize,
    find: &str,
    replace: &str,
) -> Result<Value, String> {
    use pdf_x_core::core::FindOptions;

    let opts = FindOptions::default();
    ctx.doc
        .replace_text(page_index, find, replace, &opts)
        .map_err(|e| format!("replace-text failed: {e}"))?;
    let (dirty, can_undo, can_redo) = ctx.doc.editor_state();
    maybe_save(ctx)?;
    Ok(json!({
        "page": page_index + 1,
        "find": find,
        "replace": replace,
        "dirty": dirty,
        "can_undo": can_undo,
        "can_redo": can_redo,
    }))
}

/// `save --output <path>`
pub fn run_save(ctx: &mut Ctx) -> Result<Value, String> {
    let Some(path) = ctx.output_path.take() else {
        return Err("save requires --output <path>".into());
    };
    let result = save_to(ctx, &path);
    ctx.output_path = Some(path);
    result?;
    let (dirty, can_undo, can_redo) = ctx.doc.editor_state();
    Ok(json!({
        "dirty": dirty,
        "can_undo": can_undo,
        "can_redo": can_redo,
    }))
}
