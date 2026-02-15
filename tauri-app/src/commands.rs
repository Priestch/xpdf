use crate::state::AppState;
use crate::types::*;
use base64::{Engine as _, engine::general_purpose};
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

    // Read PDF file data into memory (for fast access during rendering)
    let pdf_data = fs::read(&file_path).map_err(|e| e.to_string())?;

    // Cache the file data in state
    {
        let mut data_guard = state.inner().pdf_data.lock().unwrap();
        *data_guard = Some(pdf_data);
    }

    // Load PDF using progressive loading
    let mut doc =
        pdf_x_core::PDFDocument::open_file(&file_path, None, None).map_err(|e| e.to_string())?;

    // Get file size
    let file_size = fs::metadata(&file_path).map_err(|e| e.to_string())?.len();

    // Extract basic metadata
    let page_count = doc.page_count().map_err(|e| e.to_string())?;
    let pdf_version = doc.pdf_version().unwrap_or_else(|_| "Unknown".to_string());
    let is_linearized = doc.is_linearized();

    // Check encryption status
    let is_encrypted = doc.is_encrypted();
    let requires_password = is_encrypted; // If encrypted, password is required

    // Extract document info
    let (title, author, subject, keywords, creator, producer) =
        if let Ok(Some(info)) = doc.document_info() {
            extract_info_fields(&info)
        } else {
            (None, None, None, None, None, None)
        };

    // Store file path in state
    {
        let mut path_guard = state.inner().file_path.lock().unwrap();
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
        is_encrypted,
        requires_password,
    })
}

/// Close the current document
#[tauri::command]
pub fn close_document(state: State<'_, AppState>) -> Result<(), String> {
    state.inner().clear();
    Ok(())
}

/// Extract text from a specific page
#[tauri::command]
pub async fn extract_text_from_page(
    page_index: usize,
    state: State<'_, AppState>,
) -> Result<TextExtractionResult, String> {
    // Get file path from state
    let file_path = {
        let path_guard = state.inner().file_path.lock().unwrap();
        path_guard.as_ref().cloned()
    };

    let file_path = file_path.ok_or("No document loaded")?;

    // Reload document
    let mut doc =
        pdf_x_core::PDFDocument::open_file(&file_path, None, None).map_err(|e| e.to_string())?;

    // Extract text items
    let text_items = doc
        .extract_text_from_page(page_index)
        .map_err(|e| e.to_string())?;

    // Convert core TextItem to Tauri TextItem
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

/// Get document outline (bookmarks)
#[tauri::command]
pub async fn get_document_outline(state: State<'_, AppState>) -> Result<Vec<OutlineItem>, String> {
    // Get file path from state
    let file_path = {
        let path_guard = state.inner().file_path.lock().unwrap();
        path_guard.as_ref().cloned()
    };

    let file_path = file_path.ok_or("No document loaded")?;

    // Reload document
    let mut doc =
        pdf_x_core::PDFDocument::open_file(&file_path, None, None).map_err(|e| e.to_string())?;

    match doc.document_outline_items().map_err(|e| e.to_string())? {
        Some(core_items) => {
            // Convert core library OutlineItem to Tauri OutlineItem
            let items = core_items
                .into_iter()
                .map(|item| convert_outline_item(item, &mut doc))
                .collect::<Result<Vec<_>, String>>()?;
            Ok(items)
        }
        None => Ok(vec![]),
    }
}

/// Converts a core library OutlineItem to a Tauri OutlineItem
fn convert_outline_item(
    core_item: pdf_x_core::OutlineItem,
    doc: &mut pdf_x_core::PDFDocument,
) -> Result<OutlineItem, String> {
    // Resolve destination to page number and other info
    let (page, dest_type, url) = match &core_item.dest {
        Some(pdf_x_core::OutlineDestination::Explicit {
            page_index,
            dest_type,
        }) => {
            let page = Some(*page_index as u32);
            let dest_type = Some(format!("{:?}", dest_type));
            (page, dest_type, None)
        }
        Some(pdf_x_core::OutlineDestination::Named(_name)) => {
            // Named destinations not implemented in MVP
            (None, None, None)
        }
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

    // Recursively convert children
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

/// Get page sizes
#[tauri::command]
pub async fn get_page_sizes(state: State<'_, AppState>) -> Result<Vec<PageInfo>, String> {
    // Get file path from state
    let file_path = {
        let path_guard = state.inner().file_path.lock().unwrap();
        path_guard.as_ref().cloned()
    };

    let file_path = file_path.ok_or("No document loaded")?;

    // Reload document
    let mut doc =
        pdf_x_core::PDFDocument::open_file(&file_path, None, None).map_err(|e| e.to_string())?;

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
            let values: Vec<f64> = arr
                .iter()
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

/// Render a page to PNG image
#[tauri::command]
pub async fn render_page(
    page_index: usize,
    scale: Option<f32>,
    state: State<'_, AppState>,
) -> Result<RenderedPage, String> {
    // Get the cached PDF data
    let pdf_data = {
        let data_guard = state.inner().pdf_data.lock().unwrap();
        data_guard.as_ref().cloned().ok_or("No document loaded")?
    };

    // Parse PDF from cached data (much faster than reading from disk)
    let mut doc = pdf_x_core::PDFDocument::open(pdf_data).map_err(|e| e.to_string())?;

    // Render the page to image (RGBA pixels)
    let (width, height, mut pixels) = doc
        .render_page_to_image(page_index, scale)
        .map_err(|e| e.to_string())?;

    // Debug: Check if we got any non-white pixels
    #[cfg(debug_assertions)]
    {
        let non_white_count = pixels
            .chunks(4)
            .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
            .count();
        let total_pixels = (width * height) as usize;
        eprintln!(
            "DEBUG: Rendered {}x{} ({} pixels), {} non-white pixels ({:.1}%)",
            width,
            height,
            total_pixels,
            non_white_count,
            (non_white_count as f64 / total_pixels as f64) * 100.0
        );
    }

    // Encode RGBA pixels to PNG
    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, width, height);

        // Set color type to RGBA (8 bits per channel)
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);

        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG write header error: {}", e))?;

        // Write the image data
        writer
            .write_image_data(&pixels)
            .map_err(|e| format!("PNG write data error: {}", e))?;
    }

    // Encode PNG data to base64
    let base64_data = general_purpose::STANDARD.encode(&png_data);

    Ok(RenderedPage {
        page: page_index,
        width,
        height,
        image_data: base64_data,
    })
}
