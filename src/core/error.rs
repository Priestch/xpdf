use std::fmt;

/// Universal error type for PDF operations.
///
/// This error type covers all possible errors that can occur during
/// PDF parsing, loading, and rendering operations.
#[derive(Debug, Clone)]
pub enum PDFError {
    /// End of stream reached unexpectedly
    UnexpectedEndOfStream,

    /// Invalid byte range requested
    InvalidByteRange { begin: usize, end: usize },

    /// Data not yet loaded (for progressive loading scenarios)
    DataNotLoaded { chunk: usize },

    /// Invalid stream position
    InvalidPosition { pos: usize, length: usize },

    /// Stream operation failed
    StreamError(String),

    /// Generic error with message
    Generic(String),
}

impl fmt::Display for PDFError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PDFError::UnexpectedEndOfStream => {
                write!(f, "Unexpected end of stream")
            }
            PDFError::InvalidByteRange { begin, end } => {
                write!(f, "Invalid byte range: {}..{}", begin, end)
            }
            PDFError::DataNotLoaded { chunk } => {
                write!(f, "Data not loaded for chunk {}", chunk)
            }
            PDFError::InvalidPosition { pos, length } => {
                write!(f, "Invalid position {} for stream of length {}", pos, length)
            }
            PDFError::StreamError(msg) => {
                write!(f, "Stream error: {}", msg)
            }
            PDFError::Generic(msg) => {
                write!(f, "{}", msg)
            }
        }
    }
}

impl std::error::Error for PDFError {}

/// Result type alias for PDF operations
pub type PDFResult<T> = Result<T, PDFError>;
