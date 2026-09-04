use crate::state::AppState;
use crate::types::*;
use base64::{Engine as _, engine::general_purpose};
use pdf_x_core::core::document_loader::{DocumentLoader, OpenProgress};
use std::fs;
use std::path::PathBuf;
use tauri::State;

/// Rounds the shell drives the open loop before declaring no progress — the
/// command-level counterpart of `drive`'s step cap, sized for the few rounds
/// a local file source needs.
const MAX_OPEN_ROUNDS: usize = 64;

/// Open a PDF file, cache the document, and return its metadata.
#[tauri::command]
pub fn open_pdf_file(
    file_path: String,
    state: State<'_, AppState>,
) -> Result<DocumentMetadata, String> {
    let path = PathBuf::from(&file_path);
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }

    let file_size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
    // Progressive, shell-owned open (Phase D step 5): the command drives the
    // core loader over a chunked file source instead of buffering the whole
    // file — the open reads only the tail + xref ranges (64 KiB chunks,
    // LRU-capped), and later page access pulls further chunks on demand.
    let stream =
        pdf_x_source_file::FileChunkedStream::open(&path, None, None).map_err(|e| e.to_string())?;
    let mut loader = DocumentLoader::new(stream);
    let mut doc = None;
    // Drive the open round by round (the pull protocol of `drive`, split so
    // the shell owns it): each `step` runs as far as locally-available bytes
    // allow, and the chunked file source loads missing ranges synchronously
    // inside `step` — blocking reads live in the shell-side pdf-x-source-file
    // crate, never in core. Bounded rounds: a source that keeps surfacing the
    // same Need without satisfying it is a truncated/changed file, not a
    // reason to spin.
    for _ in 0..MAX_OPEN_ROUNDS {
        match loader.step().map_err(|e| e.to_string())? {
            OpenProgress::Ready(ready) => {
                doc = Some(*ready);
                break;
            }
            OpenProgress::Need(_) => continue, // retry: step restarts its phase
        }
    }
    let mut doc =
        doc.ok_or_else(|| format!("open made no progress after {MAX_OPEN_ROUNDS} rounds"))?;

    let page_count = doc.page_count().map_err(|e| e.to_string())?;
    let pdf_version = doc.pdf_version().unwrap_or_else(|_| "Unknown".to_string());
    let is_linearized = doc.is_linearized();
    let is_encrypted = doc.is_encrypted();
    let requires_password = is_encrypted && doc.get_encryption_key().is_err();

    let (title, author, subject, keywords, creator, producer) =
        if let Ok(Some(info)) = doc.document_info() {
            extract_info_fields(&info)
        } else {
            (None, None, None, None, None, None)
        };

    *state.file_path.lock().unwrap() = Some(path);
    *state.document.lock().unwrap() = Some(doc);

    Ok(DocumentMetadata {
        title,
        author,
        subject,
        keywords,
        creator,
        producer,
        creation_date: None,
        modification_date: None,
        page_count,
        file_size,
        pdf_version,
        is_linearized,
        is_encrypted,
        requires_password,
    })
}

/// Close the current document.
#[tauri::command]
pub fn close_document(state: State<'_, AppState>) -> Result<(), String> {
    state.clear();
    Ok(())
}

/// Extract text from a specific page.
#[tauri::command]
pub fn extract_text_from_page(
    page_index: usize,
    state: State<'_, AppState>,
) -> Result<TextExtractionResult, String> {
    let mut guard = state.document.lock().unwrap();
    let doc = guard.as_mut().ok_or("No document loaded")?;

    let text_items = doc
        .extract_text_from_page(page_index)
        .map_err(|e| e.to_string())?;

    let tauri_items = text_items
        .into_iter()
        .map(|item| TextItem {
            text: item.text,
            font_name: item.font_name,
            font_size: item.font_size,
            x: item.position.map(|p| p.0).unwrap_or(0.0),
            y: item.position.map(|p| p.1).unwrap_or(0.0),
        })
        .collect();

    Ok(TextExtractionResult {
        page: page_index,
        text_items: tauri_items,
    })
}

/// Get document outline (bookmarks).
#[tauri::command]
pub fn get_document_outline(state: State<'_, AppState>) -> Result<Vec<OutlineItem>, String> {
    let mut guard = state.document.lock().unwrap();
    let doc = guard.as_mut().ok_or("No document loaded")?;

    match doc.document_outline_items().map_err(|e| e.to_string())? {
        Some(core_items) => core_items
            .into_iter()
            .map(|item| convert_outline_item(item, doc))
            .collect(),
        None => Ok(vec![]),
    }
}

fn convert_outline_item(
    core_item: pdf_x_core::OutlineItem,
    doc: &mut pdf_x_core::PDFDocument,
) -> Result<OutlineItem, String> {
    let (page, dest_type, url) = match &core_item.dest {
        Some(pdf_x_core::OutlineDestination::Explicit {
            page_index,
            dest_type,
        }) => {
            let page = Some(*page_index as u32);
            let dest_type = Some(format!("{:?}", dest_type));
            (page, dest_type, None)
        }
        Some(pdf_x_core::OutlineDestination::Named(_name)) => (None, None, None),
        Some(pdf_x_core::OutlineDestination::URL(uri)) => (None, None, Some(uri.clone())),
        Some(pdf_x_core::OutlineDestination::GoToRemote {
            url,
            dest,
            new_window: _,
        }) => {
            let dest_type = dest.clone().map(|_| "GoToR".to_string());
            (None, dest_type, Some(url.clone()))
        }
        None => (None, None, None),
    };

    let children = core_item
        .children
        .into_iter()
        .map(|child| convert_outline_item(child, doc))
        .collect::<Result<Vec<_>, String>>()?;

    Ok(OutlineItem {
        title: core_item.title,
        page,
        dest_type,
        url,
        color: core_item.color,
        bold: core_item.bold,
        italic: core_item.italic,
        count: core_item.count,
        children,
    })
}

/// Get page sizes for every page (MediaBox).
#[tauri::command]
pub fn get_page_sizes(state: State<'_, AppState>) -> Result<Vec<PageInfo>, String> {
    let mut guard = state.document.lock().unwrap();
    let doc = guard.as_mut().ok_or("No document loaded")?;

    let page_count = doc.page_count().map_err(|e| e.to_string())?;
    let mut pages = Vec::with_capacity(page_count as usize);

    for i in 0..page_count {
        let page = doc.get_page(i as usize).map_err(|e| e.to_string())?;
        let (width, height) = page
            .media_box()
            .and_then(extract_media_box_dimensions)
            .unwrap_or((595.0, 842.0));

        pages.push(PageInfo {
            index: i as usize,
            width,
            height,
            rotation: 0,
        });
    }

    Ok(pages)
}

fn extract_info_fields(
    info: &pdf_x_core::PDFObject,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    if let pdf_x_core::PDFObject::Dictionary(dict) = info {
        (
            dict.get("Title").and_then(extract_string_value),
            dict.get("Author").and_then(extract_string_value),
            dict.get("Subject").and_then(extract_string_value),
            dict.get("Keywords").and_then(extract_string_value),
            dict.get("Creator").and_then(extract_string_value),
            dict.get("Producer").and_then(extract_string_value),
        )
    } else {
        (None, None, None, None, None, None)
    }
}

fn extract_string_value(obj: &pdf_x_core::PDFObject) -> Option<String> {
    match obj {
        // Both literal and hex strings are raw bytes after parsing.
        pdf_x_core::PDFObject::String(s) | pdf_x_core::PDFObject::HexString(s) => {
            Some(String::from_utf8_lossy(s).to_string())
        }
        _ => None,
    }
}

fn extract_media_box_dimensions(mediabox: &pdf_x_core::PDFObject) -> Option<(f64, f64)> {
    let pdf_x_core::PDFObject::Array(arr) = mediabox else {
        return None;
    };
    if arr.len() < 4 {
        return None;
    }
    let values: Vec<f64> = arr
        .iter()
        .take(4)
        .filter_map(|v| match v.as_ref() {
            pdf_x_core::PDFObject::Number(n) => Some(*n),
            _ => None,
        })
        .collect();
    if values.len() != 4 {
        return None;
    }
    Some((values[2] - values[0], values[3] - values[1]))
}

/// Render a page to a PNG image (base64).
#[tauri::command]
pub fn render_page(
    page_index: usize,
    scale: Option<f32>,
    text_trace: Option<bool>,
    state: State<'_, AppState>,
) -> Result<RenderedPage, String> {
    // Phase E: the document lock is held only to resolve an immutable
    // snapshot (an Arc clone on cache hit); interpretation, rasterization,
    // and PNG encoding run outside it, so editor commands and other renders
    // proceed concurrently. Byte-identical to the former doc-locked render —
    // both paths funnel into the same snapshot renderer (parity-tested in
    // pdf-x-device-tinyskia).
    let resolved = {
        let mut guard = state.document.lock().unwrap();
        let doc = guard.as_mut().ok_or("No document loaded")?;
        doc.resolve_page(page_index).map_err(|e| e.to_string())?
    };

    let text_trace_enabled = text_trace.unwrap_or(false);
    let previous_trace_state = pdf_x_core::rendering::set_text_trace_enabled(text_trace_enabled);
    if text_trace_enabled {
        eprintln!(
            "[text-trace] render_page start: page={} scale={}",
            page_index + 1,
            scale.unwrap_or(1.0)
        );
    }

    let mut session = pdf_x_device_tinyskia::RenderSession::new();
    let render_result = session.render(&resolved, scale).map_err(|e| e.to_string());

    pdf_x_core::rendering::set_text_trace_enabled(previous_trace_state);

    let (width, height, pixels) = render_result?;

    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG write header error: {}", e))?;
        writer
            .write_image_data(&pixels)
            .map_err(|e| format!("PNG write data error: {}", e))?;
    }

    Ok(RenderedPage {
        page: page_index,
        width,
        height,
        image_data: general_purpose::STANDARD.encode(&png_data),
    })
}

fn editor_state(doc: &pdf_x_core::PDFDocument) -> EditorState {
    let (dirty, can_undo, can_redo) = doc.editor_state();
    EditorState {
        dirty,
        can_undo,
        can_redo,
    }
}

/// Add a highlight rectangle expressed in PDF user-space coordinates.
#[tauri::command]
pub fn add_highlight(
    page_index: usize,
    rect: [f64; 4],
    color: Option<[f64; 3]>,
    state: State<'_, AppState>,
) -> Result<EditorState, String> {
    let mut guard = state.document.lock().unwrap();
    let doc = guard.as_mut().ok_or("No document loaded")?;
    doc.add_highlight(page_index, rect, color.unwrap_or([1.0, 0.92, 0.2]))
        .map_err(|e| e.to_string())?;
    Ok(editor_state(doc))
}

/// Move a page to a new position (final-position semantics, 0-based).
#[tauri::command]
pub fn move_page(
    from_index: usize,
    to_index: usize,
    state: State<'_, AppState>,
) -> Result<EditorState, String> {
    let mut guard = state.document.lock().unwrap();
    let doc = guard.as_mut().ok_or("No document loaded")?;
    doc.move_page(from_index, to_index)
        .map_err(|e| e.to_string())?;
    Ok(editor_state(doc))
}

#[tauri::command]
pub fn undo_edit(state: State<'_, AppState>) -> Result<EditorState, String> {
    let mut guard = state.document.lock().unwrap();
    let doc = guard.as_mut().ok_or("No document loaded")?;
    doc.undo().map_err(|e| e.to_string())?;
    Ok(editor_state(doc))
}

#[tauri::command]
pub fn redo_edit(state: State<'_, AppState>) -> Result<EditorState, String> {
    let mut guard = state.document.lock().unwrap();
    let doc = guard.as_mut().ok_or("No document loaded")?;
    doc.redo().map_err(|e| e.to_string())?;
    Ok(editor_state(doc))
}

#[tauri::command]
pub fn get_editor_state(state: State<'_, AppState>) -> Result<EditorState, String> {
    let guard = state.document.lock().unwrap();
    let doc = guard.as_ref().ok_or("No document loaded")?;
    Ok(editor_state(doc))
}

/// Save the original bytes plus one incremental update and reopen the result.
#[tauri::command]
pub fn save_pdf_as(file_path: String, state: State<'_, AppState>) -> Result<EditorState, String> {
    let source_path = state
        .file_path
        .lock()
        .unwrap()
        .clone()
        .ok_or("No document loaded")?;
    let original = fs::read(&source_path).map_err(|e| e.to_string())?;
    let mut guard = state.document.lock().unwrap();
    let doc = guard.as_ref().ok_or("No document loaded")?;
    let update = doc
        .incremental_update(&original)
        .map_err(|e| e.to_string())?;
    let mut output = original;
    output.extend_from_slice(&update);
    fs::write(&file_path, &output).map_err(|e| e.to_string())?;
    let reopened = pdf_x_core::PDFDocument::open(output).map_err(|e| e.to_string())?;
    *guard = Some(reopened);
    *state.file_path.lock().unwrap() = Some(PathBuf::from(file_path));
    Ok(editor_state(guard.as_ref().unwrap()))
}
