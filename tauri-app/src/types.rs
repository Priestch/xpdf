use serde::{Deserialize, Serialize};

/// Document metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub creation_date: Option<String>,
    pub modification_date: Option<String>,
    pub page_count: u32,
    pub file_size: u64,
    pub pdf_version: String,
    pub is_linearized: bool,
}

/// Outline item (bookmark)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<u32>,
    pub children: Vec<OutlineItem>,
}

/// Page information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageInfo {
    pub index: usize,
    pub width: f64,
    pub height: f64,
    pub rotation: u32,
}

/// Text extraction result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextExtractionResult {
    pub page: usize,
    pub text_items: Vec<TextItem>,
}

/// Individual text item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextItem {
    pub text: String,
    pub font_name: Option<String>,
    pub font_size: Option<f64>,
    pub x: f64,
    pub y: f64,
}

/// Progress event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    pub stage: String,
    pub progress: u32,
    pub message: String,
}

/// Error types for the Tauri app
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("PDF error: {0}")]
    PdfError(#[from] pdf_x_core::PDFError),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Invalid file path")]
    InvalidPath,
}

// Convert AppError to a String for Tauri
impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Result type for Tauri commands
pub type AppResult<T> = Result<T, AppError>;
