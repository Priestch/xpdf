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

    /// Data missing - indicates which bytes are needed for progressive loading
    DataMissing { position: usize, length: usize },

    /// Invalid stream position
    InvalidPosition { pos: usize, length: usize },

    /// Invalid PDF object encountered
    InvalidObject { expected: String, found: String },

    /// Object marked as deleted in the delta layer (resolve: `delta → base`)
    ObjectDeleted { num: u32, generation: u32 },

    /// An xref entry's recorded offset is unusable — it doesn't land on the expected
    /// `N G obj` header (wrong object number, missing generation, out of bounds, or the
    /// bytes there don't parse as an object header). The signature of a stale xref in a
    /// regenerated/corrupt PDF. Carries the failing object number so lazy fetch-time
    /// recovery can rebuild the xref and retry.
    BadXRefOffset { num: u32, reason: String },

    /// No xref entry for this object number. Can be a legitimate free/absent object, but
    /// also the signature of a *partial* xref parse (e.g. linearized first-page section
    /// with a broken `/Prev` that was skipped, so early objects never got indexed). Lazy
    /// fetch-time recovery rebuilds the table from a full-file scan and retries once.
    ObjectNotInXRef { num: u32 },

    /// Parse error with context
    ParseError {
        message: String,
        context: Option<String>,
        /// File position where error occurred (byte offset)
        position: Option<usize>,
    },

    /// XRef table errors
    XRefError { message: String },

    /// Page tree errors
    PageError { message: String },

    /// Font errors
    FontError { message: String },

    /// Content stream errors
    ContentStreamError { message: String },

    /// I/O or network error
    IOError { message: String },

    /// Invalid or corrupted PDF structure
    CorruptedPDF { message: String },

    /// Feature not yet implemented
    Unsupported { feature: String },

    /// A configured resource limit was exceeded while processing untrusted input.
    LimitExceeded {
        /// Name of the exhausted limit.
        resource: &'static str,
        /// Configured maximum.
        limit: usize,
        /// Observed or requested amount.
        actual: usize,
    },

    /// Validation error
    ValidationError { message: String },

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
            PDFError::DataMissing { position, length } => {
                write!(
                    f,
                    "Data missing at position {} ({} bytes needed)",
                    position, length
                )
            }
            PDFError::InvalidPosition { pos, length } => {
                write!(
                    f,
                    "Invalid position {} for stream of length {}",
                    pos, length
                )
            }
            PDFError::InvalidObject { expected, found } => {
                write!(f, "Invalid object: expected {}, found {}", expected, found)
            }
            PDFError::ObjectDeleted { num, generation } => {
                write!(f, "Object {} {} deleted in delta layer", num, generation)
            }
            PDFError::BadXRefOffset { num, reason } => {
                write!(f, "Bad xref offset for object {num}: {reason}")
            }
            PDFError::ObjectNotInXRef { num } => {
                write!(f, "Object {num} not found in xref")
            }
            PDFError::ParseError {
                message,
                context,
                position,
            } => {
                let pos_str = position
                    .map(|p| format!(" at byte {}", p))
                    .unwrap_or_default();
                match context {
                    Some(ctx) => {
                        write!(f, "Parse error{}: {} (context: {})", pos_str, message, ctx)
                    }
                    None => write!(f, "Parse error{}: {}", pos_str, message),
                }
            }
            PDFError::XRefError { message } => {
                write!(f, "Cross-reference table error: {}", message)
            }
            PDFError::PageError { message } => {
                write!(f, "Page error: {}", message)
            }
            PDFError::FontError { message } => {
                write!(f, "Font error: {}", message)
            }
            PDFError::ContentStreamError { message } => {
                write!(f, "Content stream error: {}", message)
            }
            PDFError::IOError { message } => {
                write!(f, "I/O error: {}", message)
            }
            PDFError::CorruptedPDF { message } => {
                write!(f, "Corrupted PDF: {}", message)
            }
            PDFError::Unsupported { feature } => {
                write!(f, "Unsupported feature: {}", feature)
            }
            PDFError::LimitExceeded {
                resource,
                limit,
                actual,
            } => {
                write!(
                    f,
                    "Resource limit exceeded for {resource}: {actual} > {limit}"
                )
            }
            PDFError::ValidationError { message } => {
                write!(f, "Validation error: {}", message)
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

impl PDFError {
    /// Creates a parse error with context and optional position.
    pub fn parse_error<S: Into<String>>(message: S, context: Option<S>) -> Self {
        PDFError::ParseError {
            message: message.into(),
            context: context.map(|s| s.into()),
            position: None,
        }
    }

    /// Creates a parse error with context and file position.
    pub fn parse_error_at<S: Into<String>>(
        message: S,
        context: Option<S>,
        position: usize,
    ) -> Self {
        PDFError::ParseError {
            message: message.into(),
            context: context.map(|s| s.into()),
            position: Some(position),
        }
    }

    /// Creates an XRef error.
    pub fn xref_error<S: Into<String>>(message: S) -> Self {
        PDFError::XRefError {
            message: message.into(),
        }
    }

    /// Creates a page error.
    pub fn page_error<S: Into<String>>(message: S) -> Self {
        PDFError::PageError {
            message: message.into(),
        }
    }

    /// Creates a font error.
    pub fn font_error<S: Into<String>>(message: S) -> Self {
        PDFError::FontError {
            message: message.into(),
        }
    }

    /// Creates a content stream error.
    pub fn content_stream_error<S: Into<String>>(message: S) -> Self {
        PDFError::ContentStreamError {
            message: message.into(),
        }
    }

    /// Creates an I/O error.
    pub fn io_error<S: Into<String>>(message: S) -> Self {
        PDFError::IOError {
            message: message.into(),
        }
    }

    /// Creates a corrupted PDF error.
    pub fn corrupted_pdf<S: Into<String>>(message: S) -> Self {
        PDFError::CorruptedPDF {
            message: message.into(),
        }
    }

    /// Creates an unsupported feature error.
    pub fn unsupported<S: Into<String>>(feature: S) -> Self {
        PDFError::Unsupported {
            feature: feature.into(),
        }
    }

    /// Creates a validation error.
    pub fn validation_error<S: Into<String>>(message: S) -> Self {
        PDFError::ValidationError {
            message: message.into(),
        }
    }

    /// Creates a data missing error for progressive loading.
    pub fn data_missing(position: usize, length: usize) -> Self {
        PDFError::DataMissing { position, length }
    }

    /// Creates an invalid object error.
    pub fn invalid_object<S: Into<String>>(expected: S, found: S) -> Self {
        PDFError::InvalidObject {
            expected: expected.into(),
            found: found.into(),
        }
    }

    /// Creates a `BadXRefOffset` error (a stale/unusable xref entry offset).
    pub fn bad_xref_offset<S: Into<String>>(num: u32, reason: S) -> Self {
        PDFError::BadXRefOffset {
            num,
            reason: reason.into(),
        }
    }
}

impl std::error::Error for PDFError {}

/// Result type alias for PDF operations
pub type PDFResult<T> = Result<T, PDFError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = PDFError::data_missing(100, 50);
        assert_eq!(
            format!("{}", err),
            "Data missing at position 100 (50 bytes needed)"
        );

        let err = PDFError::xref_error("Invalid entry");
        assert_eq!(
            format!("{}", err),
            "Cross-reference table error: Invalid entry"
        );

        let err = PDFError::page_error("Page not found");
        assert_eq!(format!("{}", err), "Page error: Page not found");

        let err = PDFError::parse_error("Invalid token", Some("while reading object"));
        assert_eq!(
            format!("{}", err),
            "Parse error: Invalid token (context: while reading object)"
        );

        let err = PDFError::unsupported("Linearized PDFs");
        assert_eq!(format!("{}", err), "Unsupported feature: Linearized PDFs");
    }

    #[test]
    fn test_error_creation_methods() {
        let err = PDFError::parse_error("test", Some("context"));
        match err {
            PDFError::ParseError {
                message,
                context,
                position,
            } => {
                assert_eq!(message, "test");
                assert_eq!(context, Some("context".to_string()));
                assert_eq!(position, None);
            }
            _ => panic!("Expected ParseError"),
        }

        let err = PDFError::parse_error_at("test", Some("context"), 42);
        match err {
            PDFError::ParseError {
                message,
                context,
                position,
            } => {
                assert_eq!(message, "test");
                assert_eq!(context, Some("context".to_string()));
                assert_eq!(position, Some(42));
            }
            _ => panic!("Expected ParseError"),
        }

        let err = PDFError::data_missing(42, 10);
        match err {
            PDFError::DataMissing { position, length } => {
                assert_eq!(position, 42);
                assert_eq!(length, 10);
            }
            _ => panic!("Expected DataMissing"),
        }

        let err = PDFError::invalid_object("Number", "String");
        match err {
            PDFError::InvalidObject { expected, found } => {
                assert_eq!(expected, "Number");
                assert_eq!(found, "String");
            }
            _ => panic!("Expected InvalidObject"),
        }
    }

    #[test]
    fn test_bad_xref_offset_error() {
        let err = PDFError::bad_xref_offset(7, "object number mismatch: expected 7, got 5");
        assert_eq!(
            format!("{}", err),
            "Bad xref offset for object 7: object number mismatch: expected 7, got 5"
        );
        match err {
            PDFError::BadXRefOffset { num, reason } => {
                assert_eq!(num, 7);
                assert_eq!(reason, "object number mismatch: expected 7, got 5");
            }
            _ => panic!("Expected BadXRefOffset"),
        }
    }

    #[test]
    fn test_error_chain_compatibility() {
        let err = PDFError::io_error("File not found");

        // Test that it can be used as a standard error
        let _dyn_err: &dyn std::error::Error = &err;

        // Test that it works with Result
        let result: PDFResult<()> = Err(err);
        assert!(result.is_err());
    }
}
