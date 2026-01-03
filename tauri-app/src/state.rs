use pdf_x_core::PDFDocument;
use std::path::PathBuf;
use std::sync::Mutex;

/// Application state
///
/// This holds the currently loaded document and its file path.
/// It uses Mutex for thread-safe access across Tauri commands.
pub struct AppState {
    /// Currently loaded document
    pub document: Mutex<Option<PDFDocument>>,
    /// File path of current document
    pub file_path: Mutex<Option<PathBuf>>,
}

impl AppState {
    /// Create a new empty application state
    pub fn new() -> Self {
        Self {
            document: Mutex::new(None),
            file_path: Mutex::new(None),
        }
    }
}
