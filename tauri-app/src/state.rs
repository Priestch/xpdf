use pdf_x_core::PDFDocument;
use std::path::PathBuf;
use std::sync::Mutex;

/// Application state.
///
/// Caches the opened `PDFDocument` so page renders reuse the object graph /
/// resolved-page LRU instead of re-parsing the file on every navigation.
pub struct AppState {
    /// File path of the current document (for UI display / reopen).
    pub file_path: Mutex<Option<PathBuf>>,
    /// Opened document. `PDFDocument` is `!Sync` (`RefCell` caches), so access
    /// is serialized through this mutex and never held across `.await`.
    pub document: Mutex<Option<PDFDocument>>,
}

impl AppState {
    /// Create a new empty application state.
    pub fn new() -> Self {
        Self {
            file_path: Mutex::new(None),
            document: Mutex::new(None),
        }
    }

    /// Clear all cached data.
    pub fn clear(&self) {
        *self.file_path.lock().unwrap() = None;
        *self.document.lock().unwrap() = None;
    }
}
