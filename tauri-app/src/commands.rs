use crate::state::AppState;
use crate::types::*;
use std::fs;
use std::path::PathBuf;
use tauri::State;

/// Open a PDF file and extract its metadata
#[tauri::command]
pub async fn open_pdf_file(
    file_path: String,
    state: State<'_, AppState>,
) -> Result<DocumentMetadata, String> {
    // Check file exists
    let path = PathBuf::from(&file_path);
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }

    // Load PDF using progressive loading
    let mut doc = pdf_x_core::PDFDocument::open_file(&file_path, None, None)
        .map_err(|e| e.to_string())?;

    // Get file size
    let file_size = fs::metadata(&file_path).map_err(|e| e.to_string())?.len();

    // Extract basic metadata
    let page_count = doc.page_count().map_err(|e| e.to_string())?;
    let pdf_version = doc.pdf_version().unwrap_or_else(|_| "Unknown".to_string());
    let is_linearized = doc.is_linearized();

    // Extract document info
    let (title, author, subject, keywords, creator, producer) = if let Ok(Some(info)) = doc.document_info() {
        extract_info_fields(&info)
    } else {
        (None, None, None, None, None, None)
    };

    // Store document in state
    {
        let mut doc_guard = state.document.lock().map_err(|e| e.to_string())?;
        *doc_guard = Some(doc);

        let mut path_guard = state.file_path.lock().map_err(|e| e.to_string())?;
        *path_guard = Some(path);
    }

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
    })
}

/// Close the current document
#[tauri::command]
pub fn close_document(state: State<'_, AppState>) -> Result<(), String> {
    let mut doc_guard = state.document.lock().map_err(|e| e.to_string())?;
    *doc_guard = None;

    let mut path_guard = state.file_path.lock().map_err(|e| e.to_string())?;
    *path_guard = None;

    Ok(())
}

/// Get document outline (bookmarks)
#[tauri::command]
pub async fn get_document_outline(
    state: State<'_, AppState>,
) -> Result<Vec<OutlineItem>, String> {
    let doc_guard = state.document.lock().map_err(|e| e.to_string())?;
    let doc = doc_guard.as_ref().ok_or("No document loaded")?;

    match doc.document_outline().map_err(|e| e.to_string())? {
        Some(outline) => {
            // For MVP, return empty list - will implement recursively later
            Ok(vec![])
        }
        None => Ok(vec![]),
    }
}

/// Get page sizes
#[tauri::command]
pub async fn get_page_sizes(
    state: State<'_, AppState>,
) -> Result<Vec<PageInfo>, String> {
    let mut doc_guard = state.document.lock().map_err(|e| e.to_string())?;
    let doc = doc_guard.as_mut().ok_or("No document loaded")?;

    let page_count = doc.page_count().map_err(|e| e.to_string())?;
    let mut pages = Vec::new();

    for i in 0..page_count {
        let page = doc.get_page(i as usize).map_err(|e| e.to_string())?;

        // Get media box
        let media_box = page.media_box();
        let (width, height) = if let Some(mediabox) = media_box {
            extract_media_box_dimensions(mediabox).unwrap_or((595.0, 842.0)) // Default A4
        } else {
            (595.0, 842.0) // Default A4 size
        };

        pages.push(PageInfo {
            index: i as usize,
            width,
            height,
            rotation: 0,
        });
    }

    Ok(pages)
}

/// Helper function to extract info fields from document info dictionary
fn extract_info_fields(info: &pdf_x_core::PDFObject) -> (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>) {
    if let pdf_x_core::PDFObject::Dictionary(dict) = info {
        let title = dict.get("Title").and_then(|v| extract_string_value(v));
        let author = dict.get("Author").and_then(|v| extract_string_value(v));
        let subject = dict.get("Subject").and_then(|v| extract_string_value(v));
        let keywords = dict.get("Keywords").and_then(|v| extract_string_value(v));
        let creator = dict.get("Creator").and_then(|v| extract_string_value(v));
        let producer = dict.get("Producer").and_then(|v| extract_string_value(v));

        (title, author, subject, keywords, creator, producer)
    } else {
        (None, None, None, None, None, None)
    }
}

/// Helper function to extract string value from PDF object
fn extract_string_value(obj: &pdf_x_core::PDFObject) -> Option<String> {
    match obj {
        pdf_x_core::PDFObject::String(s) => Some(String::from_utf8_lossy(s).to_string()),
        pdf_x_core::PDFObject::HexString(s) => {
            let hex_str: String = s.iter().map(|b| format!("{:02x}", b)).collect();
            Some(String::from_utf8_lossy(hex_str.as_bytes()).to_string())
        }
        _ => None,
    }
}

/// Helper function to extract width and height from MediaBox
fn extract_media_box_dimensions(mediabox: &pdf_x_core::PDFObject) -> Option<(f64, f64)> {
    if let pdf_x_core::PDFObject::Array(arr) = mediabox {
        if arr.len() >= 4 {
            let values: Vec<f64> = arr.iter()
                .take(4)
                .filter_map(|v| {
                    if let pdf_x_core::PDFObject::Number(n) = v.as_ref() {
                        Some(*n)
                    } else {
                        None
                    }
                })
                .collect();

            if values.len() == 4 {
                let width = values[2] - values[0];
                let height = values[3] - values[1];
                return Some((width, height));
            }
        }
    }

    None
}
