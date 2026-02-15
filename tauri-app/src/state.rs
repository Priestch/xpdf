use std::path::PathBuf;
use std::sync::Mutex;

/// Application state
///
/// Stores the PDF file data to avoid re-reading from disk.
pub struct AppState {
    /// File path of current document
    pub file_path: Mutex<Option<PathBuf>>,

    /// Raw PDF file data (cached in memory for fast access)
    pub pdf_data: Mutex<Option<Vec<u8>>>,
}

impl AppState {
    /// Create a new empty application state
    pub fn new() -> Self {
        Self {
            file_path: Mutex::new(None),
            pdf_data: Mutex::new(None),
        }
    }

    /// Clear all cached data
    pub fn clear(&self) {
        let mut path_guard = self.file_path.lock().unwrap();
        *path_guard = None;

        let mut data_guard = self.pdf_data.lock().unwrap();
        *data_guard = None;
    }
}
