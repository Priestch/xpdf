//! Native filesystem byte source for PDF-X (Phase D step 4).
//!
//! The core (`pdf-x-core`) is platform-agnostic and performs no I/O; the
//! blocking file access that [`FileChunkedStream`] needs lives here, in a
//! shell-side adapter crate, implementing core's [`BaseStream`] over a
//! chunk-cached file handle. Native shells (CLI, Tauri) depend on this crate;
//! web/WASM shells provide their own source instead.
//!
//! [`open_file`] is the drop-in replacement for the `PDFDocument::open_file`
//! that used to live in core: it drives the same progressive open
//! ([`DocumentLoader`] over a [`FileChunkedStream`]), so behavior is
//! unchanged — only the ownership of the file handle moved out of core.
//!
//! ```no_run
//! use pdf_x_source_file::open_file;
//!
//! let mut doc = open_file("document.pdf", None, None).unwrap();
//! let pages = doc.page_count().unwrap();
//! ```

mod chunk_manager;
mod file_chunked_stream;

pub use chunk_manager::{ChunkLoader, ChunkManager};
pub use file_chunked_stream::FileChunkedStream;

use pdf_x_core::PDFDocument;
use pdf_x_core::core::base_stream::BaseStream;
use pdf_x_core::core::byte_source::Need;
use pdf_x_core::core::document_loader::{DocumentLoader, OpenProgress};
use pdf_x_core::core::error::{PDFError, PDFResult};
use std::path::Path;

/// Open a PDF from the filesystem with progressive/chunked loading.
///
/// Loads the file in chunks (default 64KB) rather than reading it all into
/// memory at once — the recommended native open path (PDF.js's progressive
/// loading architecture), and the replacement for the core-side
/// `PDFDocument::open_file` removed in Phase D step 4.
///
/// The loader surfaces each byte range the open needs; a file-backed source
/// satisfies it synchronously (`ensure_range` reads the missing chunks), so
/// this drives to completion in a bounded number of steps. The same
/// no-progress guard `drive` applies protects against a source that cannot
/// fulfill a range.
pub fn open_file<P: AsRef<Path>>(
    path: P,
    chunk_size: Option<usize>,
    max_cached_chunks: Option<usize>,
) -> PDFResult<PDFDocument> {
    let stream = FileChunkedStream::open(path, chunk_size, max_cached_chunks)?;
    load_to_end(DocumentLoader::new(stream))
}

/// Step a loader until it yields a document, satisfying each [`Need`] through
/// the source's own blocking `ensure_range` (the sync-shell counterpart of
/// `drive`, expressed over the loader's step state machine).
pub fn load_to_end<S: BaseStream + 'static>(
    mut loader: DocumentLoader<S>,
) -> PDFResult<PDFDocument> {
    let mut last: Option<Need> = None;
    loop {
        match loader.step()? {
            OpenProgress::Ready(doc) => return Ok(*doc),
            OpenProgress::Need(need) => {
                if last.as_ref() == Some(&need) {
                    return Err(PDFError::Generic(format!(
                        "no progress loading {need:?}: ensure_range did not satisfy the range"
                    )));
                }
                last = Some(need);
            }
        }
    }
}
