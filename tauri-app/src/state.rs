use std::path::PathBuf;
use std::sync::Mutex;

/// Application state
///
/// Since PDFDocument uses Rc (not Send), we can't store it in global state.
/// Instead, we store just the file path and reload the document as needed.
pub struct AppState {
    /// File path of current document
    pub file_path: Mutex<Option<PathBuf>>,
}

impl AppState {
    /// Create a new empty application state
    pub fn new() -> Self {
        Self {
            file_path: Mutex::new(None),
        }
    }
}
