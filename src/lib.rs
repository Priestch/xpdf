pub mod core;

// Re-export main types for convenience
pub use core::{
    BaseStream, FileChunkedStream, HttpChunkedStream, Lexer, PDFDocument, PDFError, PDFObject,
    Parser, Stream, Token, XRef, XRefEntry,
};
