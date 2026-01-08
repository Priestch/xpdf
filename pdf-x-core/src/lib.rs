//! # PDF-X: A High-Performance PDF Library for Rust
//!
//! PDF-X is a Rust port of Mozilla's PDF.js library that maintains architectural fidelity
//! while leveraging Rust's performance and memory safety. It provides progressive PDF loading
//! and comprehensive text extraction capabilities.
//!
//! ## Features
//!
//! - **Progressive Loading**: Load PDFs incrementally with chunked data access
//! - **Text Extraction**: Extract text with position and font information
//! - **Linearized PDF Support**: Fast first-page display for optimized PDFs
//! - **Memory Safe**: Built with Rust's safety guarantees
//! - **Cross-Platform**: Works on Windows, macOS, Linux, and WebAssembly
//!
//! ## Quick Start
//!
//! ```rust
//! use pdf_x::core::PDFDocument;
//!
//! // Open a PDF file
//! let pdf_data = std::fs::read("document.pdf")?;
//! let mut doc = PDFDocument::open(pdf_data)?;
//!
//! // Get page count
//! let page_count = doc.page_count()?;
//! println!("PDF has {} pages", page_count);
//!
//! // Extract text from first page
//! let page = doc.get_page(0)?;
//! let text_items = page.extract_text(&mut doc.xref_mut())?;
//!
//! for item in text_items {
//!     println!("Text: '{}' at {:?}", item.text, item.position);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Architecture
//!
//! PDF-X follows a four-layer architecture:
//!
//! 1. **Data Source Layer**: Abstract chunked data loading from files, HTTP, or memory
//! 2. **Parser Layer**: Incremental PDF parsing with exception-driven loading
//! 3. **Document Layer**: Page tree navigation and metadata management
//! 4. **Content Layer**: Content stream interpretation and text extraction
//!
//! ## Progressive Loading
//!
//! PDF-X supports progressive loading that mirrors PDF.js's architecture:
//!
//! ```rust
//! use pdf_x::core::{PDFDocument, FileChunkedStream};
//!
//! // Open with chunked loading for large files
//! let mut stream = FileChunkedStream::open("large.pdf")?;
//! let mut doc = PDFDocument::open_stream(Box::new(stream))?;
//!
//! // Pages are loaded on-demand, not all at once
//! let page = doc.get_page(0)?; // This will trigger progressive loading
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Text Extraction
//!
//! Extract text with detailed information:
//!
//! ```rust
//! use pdf_x::core::PDFDocument;
//!
//! let mut doc = PDFDocument::open(pdf_data)?;
//! let page = doc.get_page(0)?;
//! let text_items = page.extract_text(&mut doc.xref_mut())?;
//!
//! for item in text_items {
//!     println!("Text: {}", item.text);
//!     println!("Font: {:?}", item.font_name);
//!     println!("Size: {:?}", item.font_size);
//!     println!("Position: {:?}", item.position);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## CLI Tool
//!
//! PDF-X includes a command-line tool for PDF inspection:
//!
//! ```bash
//! # Analyze PDF structure
//! cargo run --bin pdf-inspect document.pdf
//! ```
//!
//! ## Performance
//!
//! PDF-X is optimized for performance:
//! - Lazy loading of pages and content
//! - Efficient memory usage with caching
//! - Exception-driven progressive loading
//! - Thread-safe operations
//!
//! For more detailed examples and advanced usage, see the examples directory.

pub mod core;
pub mod rendering;

// Re-export main types for convenience
pub use core::{
    Annotation, AnnotationBorder, AnnotationColor, AnnotationData, AnnotationFlags,
    AnnotationRect, AnnotationType, BaseStream, DestinationType, FileAttachmentAnnotation,
    FileChunkedStream, FormFieldType, ImageDecoder, ImageFormat, LinearizedInfo, Lexer,
    LinkAction, LinkAnnotation, OutlineDestination, OutlineItem, Page, PDFDocument, PDFError,
    PDFObject, Parser, PopupAnnotation, Stream, TextAnnotation, TextItem, Token, WidgetAnnotation,
    XRef, XRefEntry,
};

// Re-export rendering types
pub use rendering::{Device, GraphicsState, Paint, Path, PathBuilder, PathDrawMode, RenderingContext};

#[cfg(feature = "rendering")]
pub use rendering::SkiaDevice;

// Re-export decode module
pub use core::decode;

#[cfg(feature = "async")]
pub use core::{AsyncHttpChunkedStream, HttpChunkedStream, ProgressCallback};
