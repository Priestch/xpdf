//! Test utilities and helpers for PDF-X tests
//!
//! This module provides common functionality for testing PDF parsing,
//! similar to PDF.js's test_utils.js

use pdf_x_core::core::*;
use std::path::PathBuf;
use std::fs;

/// Get the path to the test fixtures directory
pub fn fixtures_dir() -> PathBuf {
    // When running from pdf-x-core package, we need to go up to workspace root
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Check if we're in the pdf-x-core directory
    if manifest_dir.ends_with("pdf-x-core") {
        // Go up to workspace root, then to tests/fixtures
        manifest_dir
            .parent()
            .unwrap()
            .join("tests")
            .join("fixtures")
    } else {
        // Already at workspace root or tests directory
        manifest_dir.join("tests").join("fixtures")
    }
}

/// Get the path to the test PDFs directory
pub fn pdfs_dir() -> PathBuf {
    fixtures_dir().join("pdfs")
}

/// Get the path to a specific test PDF by name
pub fn get_test_pdf_path(name: &str) -> PathBuf {
    pdfs_dir().join(name)
}

/// Check if a test PDF exists
pub fn test_pdf_exists(name: &str) -> bool {
    get_test_pdf_path(name).exists()
}

/// Load a test PDF into memory (for small test files only)
pub fn load_test_pdf_bytes(name: &str) -> Result<Vec<u8>, std::io::Error> {
    let path = get_test_pdf_path(name);
    fs::read(path)
}

/// Create a FileChunkedStream from a test PDF
pub fn create_file_stream(name: &str) -> Result<FileChunkedStream, PDFError> {
    let path = get_test_pdf_path(name);
    FileChunkedStream::open(path, None, None)
}

/// Helper to assert that a PDF loads without errors
pub fn assert_pdf_loads(name: &str) -> Result<PDFDocument, PDFError> {
    let bytes = load_test_pdf_bytes(name)
        .map_err(|e| PDFError::Generic(format!("IO Error: {}", e)))?;
    PDFDocument::open(bytes)
}

/// Test manifest entry structure
#[derive(Debug, Clone)]
pub struct TestManifestEntry {
    pub id: String,
    pub file: String,
    pub test_type: String,
    pub description: String,
    pub pages: Option<usize>,
    pub priority: String,
    pub features: Vec<String>,
}

/// Load and parse the test manifest
pub fn load_test_manifest() -> Result<Vec<TestManifestEntry>, Box<dyn std::error::Error>> {
    let manifest_path = fixtures_dir().join("test_manifest.json");
    let content = fs::read_to_string(manifest_path)?;

    // Simple JSON parsing - in production, use serde_json
    // For now, return empty vec as placeholder
    Ok(Vec::new())
}

/// Helper to create a mock XRef for testing
pub struct XRefMock {
    entries: std::collections::HashMap<u32, XRefEntry>,
}

impl XRefMock {
    pub fn new() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
        }
    }

    pub fn add_entry(&mut self, obj_num: u32, offset: u64, generation: u16) {
        self.entries.insert(
            obj_num,
            XRefEntry::Uncompressed {
                offset,
                generation: generation as u32,
            },
        );
    }

    pub fn get_entry(&self, obj_num: u32) -> Option<&XRefEntry> {
        self.entries.get(&obj_num)
    }
}

/// Helper to create test byte streams
/// TODO: Implement BaseStream trait for this to work with Parser/Lexer
pub struct ByteStream {
    data: Vec<u8>,
    pos: usize,
}

impl ByteStream {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, pos: 0 }
    }

    pub fn from_str(s: &str) -> Self {
        Self::new(s.as_bytes().to_vec())
    }

    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let remaining = self.data.len() - self.pos;
        let to_read = buf.len().min(remaining);
        buf[..to_read].copy_from_slice(&self.data[self.pos..self.pos + to_read]);
        self.pos += to_read;
        to_read
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.data.len());
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixtures_dir_exists() {
        assert!(fixtures_dir().exists());
    }

    #[test]
    fn test_pdfs_dir_exists() {
        assert!(pdfs_dir().exists());
    }

    #[test]
    fn test_basicapi_pdf_exists() {
        assert!(test_pdf_exists("basicapi.pdf"));
    }

    #[test]
    fn test_byte_stream() {
        let mut stream = ByteStream::from_str("Hello, World!");
        assert_eq!(stream.len(), 13);
        assert_eq!(stream.position(), 0);

        let mut buf = [0u8; 5];
        let n = stream.read(&mut buf);
        assert_eq!(n, 5);
        assert_eq!(&buf, b"Hello");
        assert_eq!(stream.position(), 5);
    }

    #[test]
    fn test_xref_mock() {
        let mut xref = XRefMock::new();
        xref.add_entry(1, 100, 0);
        xref.add_entry(2, 200, 0);

        let entry = xref.get_entry(1).unwrap();
        match entry {
            XRefEntry::Uncompressed { offset, generation } => {
                assert_eq!(*offset, 100);
                assert_eq!(*generation, 0);
            }
            _ => panic!("Expected Uncompressed entry"),
        }
    }
}
