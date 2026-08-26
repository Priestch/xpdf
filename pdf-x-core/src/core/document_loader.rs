//! Shell-driven progressive document open (Phase D step 1).
//!
//! [`DocumentLoader`] unifies the open path behind one state machine a shell can
//! step: locate `startxref` from the file tail, parse the xref (retrying per
//! range as data arrives), run the same recovery ladder as the buffered path
//! when recorded offsets fail, then hand off to the shared open back half
//! (`PDFDocument::from_xref`).
//!
//! Additive by design: [`PDFDocument::open`] and [`PDFDocument::open_file`]
//! keep their current entry points; later steps converge them onto this loader
//! once `XRef` no longer owns a `Box<dyn BaseStream>`.

use super::PDFDocument;
use super::base_stream::BaseStream;
use super::byte_source::{Loadable, Need, StepErr};
use super::error::{PDFError, PDFResult};
use super::xref::XRef;

/// Outcome of advancing a [`DocumentLoader`] with [`DocumentLoader::step`].
pub enum OpenProgress {
    /// A byte range must be made available before the next `step()`. The shell
    /// loads it through its source (the loader retries from the top of its
    /// current phase, which is restartable — the same contract as
    /// [`drive`](super::byte_source::drive)).
    Need(Need),
    /// The document finished opening (boxed: `PDFDocument` dwarfs `Need`, and
    /// the loader hands it over exactly once).
    Ready(Box<PDFDocument>),
}

/// Bytes scanned at the file tail to locate `startxref` — the same window the
/// buffered path searches (`PDFDocument::find_startxref`).
const TAIL_SCAN: usize = 1024;

/// Window captured around `startxref` for the cheap `xref`-keyword recovery
/// (regenerated files whose recorded offset points just past the `xref`
/// keyword). Mirrors `PDFDocument::open`.
const RECOVERY_WINDOW: usize = 1024;

/// Progressive, shell-steppable document open over a `BaseStream` source.
///
/// Each [`step`](Self::step) runs the open as far as locally-available data
/// allows. Synchronous sources (memory-backed streams, blocking file I/O)
/// typically finish in one call. Progressive sources surface each [`Need`]
/// instead; the shell makes the range available and steps again — the
/// restartable-op contract of [`drive`](super::byte_source::drive), split so
/// async shells can interleave their own loading.
///
/// ```ignore
/// let mut loader = DocumentLoader::new(source);
/// let doc = loop {
///     match loader.step()? {
///         OpenProgress::Ready(doc) => break doc,
///         OpenProgress::Need(need) => shell_load(&need)?, // then step again
///     }
/// };
/// ```
pub struct DocumentLoader<S> {
    /// Source until it moves into the `XRef`; `None` afterwards.
    source: Option<S>,
    /// Xref once constructed; owns the source from then on.
    xref: Option<XRef>,
    /// Located `startxref` offset (`None` → straight to full-file recovery).
    startxref: Option<usize>,
    /// Base offset of `recovery_region` in the file.
    recovery_start: usize,
    /// ±1 KiB window around `startxref` for cheap `xref`-keyword recovery,
    /// captured before the source moved into the `XRef`.
    recovery_region: Vec<u8>,
    /// Whether `LocateStartxref` has completed.
    tail_scanned: bool,
    /// Terminal: the loader produced (or failed to produce) a document.
    done: bool,
}

impl<S> DocumentLoader<S>
where
    S: BaseStream + 'static,
{
    /// Begin opening over `source`.
    pub fn new(source: S) -> Self {
        Self {
            source: Some(source),
            xref: None,
            startxref: None,
            recovery_start: 0,
            recovery_region: Vec::new(),
            tail_scanned: false,
            done: false,
        }
    }

    /// Advance the open one round.
    ///
    /// Returns [`OpenProgress::Need`] when the source must load a range before
    /// progress is possible (the shell fulfills it, then steps again), or
    /// [`OpenProgress::Ready`] once the document is open. Fatal parse/IO errors
    /// surface as `Err` — including exhausted recovery, so a loader reports the
    /// same terminal outcome as `PDFDocument::open` on corrupt input.
    pub fn step(&mut self) -> PDFResult<OpenProgress> {
        if self.done {
            return Err(PDFError::Generic(
                "loader already finished; construct a new DocumentLoader".into(),
            ));
        }

        if !self.tail_scanned {
            match self.scan_tail() {
                Ok(()) => {}
                Err(StepErr::Need(need)) => return Ok(OpenProgress::Need(need)),
                Err(StepErr::Fatal(err)) => return Err(err),
            }
        }

        if self.xref.is_none() {
            let source = self
                .source
                .take()
                .expect("source is retained until the XRef takes it");
            self.xref = Some(XRef::new(Box::new(source) as Box<dyn BaseStream>));
        }

        let xref = self.xref.as_mut().expect("XRef constructed above");
        let mut parse_ok = false;
        if let Some(sx) = self.startxref {
            parse_ok = match Self::parse_attempt(xref, sx) {
                Ok(()) => true,
                Err(StepErr::Need(need)) => {
                    // Satisfy the range synchronously (blocking sources) and
                    // retry once from `startxref`; a still-missing range belongs
                    // to the shell.
                    if xref.ensure_range(need.pos(), need.len()).is_ok()
                        && Self::parse_attempt(xref, sx).is_ok()
                    {
                        true
                    } else {
                        return Ok(OpenProgress::Need(need));
                    }
                }
                Err(StepErr::Fatal(_)) => false, // fall through to recovery
            };
        }

        if !parse_ok {
            // Cheap `xref`-keyword recovery first (common "startxref slightly
            // off" case), then the full-file scan — the same escalation ladder
            // as `PDFDocument::open`.
            let recovered = self
                .recovery_region
                .windows(b"\nxref".len())
                .rposition(|w| w == b"\nxref")
                .map(|rel| self.recovery_start + rel + 1); // +1: skip '\n'
            let mut ok = false;
            if let Some(offset) = recovered {
                xref.set_stream_pos(offset)?;
                ok = xref.parse().is_ok();
            }
            if !ok {
                // Same terminal error the buffered path reported, so converging
                // `PDFDocument::open` onto this loader (D step 2) changes no
                // observable failure text.
                xref.recover_by_scan().map_err(|_| {
                    PDFError::Generic(format!(
                        "Failed to parse xref{} and full-file recovery scan failed",
                        self.startxref
                            .map(|s| format!(" at startxref offset {s}"))
                            .unwrap_or_default()
                    ))
                })?;
            }
        }

        let xref = self.xref.take().expect("XRef present until consumed");
        self.done = true;
        Ok(OpenProgress::Ready(Box::new(PDFDocument::from_xref(xref)?)))
    }

    /// One restartable xref-parse attempt from `startxref`.
    ///
    /// Mirrors the `drive` op in `PDFDocument::open_file`: the position reset
    /// on every entry is what makes retrying safe (xref entries use
    /// set-if-None guards, so re-parsing is idempotent).
    fn parse_attempt(xref: &mut XRef, startxref: usize) -> Result<(), StepErr> {
        xref.set_stream_pos(startxref)?;
        StepErr::adapt(xref.parse())
    }

    /// Read the file tail through the source's own loading path and locate
    /// `startxref`, capturing the recovery window before the source moves into
    /// the `XRef`. A range the source cannot load synchronously is reported to
    /// the caller as a `Need`; the shell fills it and steps again.
    fn scan_tail(&mut self) -> Result<(), StepErr> {
        let Some(source) = self.source.as_mut() else {
            return Err(StepErr::Fatal(PDFError::Generic(
                "loader state machine advanced past its source".into(),
            )));
        };

        let len = source.length();
        let tail_start = len.saturating_sub(TAIL_SCAN);
        let tail = read_range_or_ensure(source, tail_start, len)?;

        // Missing startxref is not fatal: the buffered path falls straight
        // through to the full-file recovery scan.
        self.startxref = PDFDocument::find_startxref_in_bytes(&tail, tail_start).ok();

        if let Some(sx) = self.startxref {
            let start = sx.saturating_sub(RECOVERY_WINDOW).min(len);
            let end = len.min(sx.saturating_add(RECOVERY_WINDOW));
            if start < end {
                self.recovery_region = read_range_or_ensure(source, start, end)?;
                self.recovery_start = start;
            }
        }

        self.tail_scanned = true;
        Ok(())
    }
}

/// Read `[begin, end)` through the source. On `DataMissing`, first try to load
/// the range synchronously (`ensure_range` — blocking for file-backed sources)
/// and retry once; if the source still cannot provide it, surface the [`Need`]
/// for the shell. All other errors are fatal.
///
/// This is the `drive` loop's ensure-and-retry half, inlined for the
/// pre-`XRef` phase where the loader still owns the source directly.
fn read_range_or_ensure<S: BaseStream>(
    source: &mut S,
    begin: usize,
    end: usize,
) -> Result<Vec<u8>, StepErr> {
    match source.get_byte_range(begin, end) {
        Ok(bytes) => Ok(bytes),
        Err(PDFError::DataMissing { position, length }) => {
            let need = Need::range(position as u64, length);
            if source.ensure_range(position, length).is_ok() {
                source.get_byte_range(begin, end).map_err(|e| match e {
                    PDFError::DataMissing { .. } => StepErr::Need(need),
                    other => StepErr::Fatal(other),
                })
            } else {
                Err(StepErr::Need(need))
            }
        }
        Err(e) => Err(StepErr::Fatal(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::Stream;

    /// A tiny but structurally complete PDF with a real xref table.
    fn sample_pdf() -> Vec<u8> {
        let mut v = b"%PDF-1.7\n".to_vec();
        let offsets: Vec<usize> = {
            let mut offs = Vec::new();
            for obj in [b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".as_slice()] {
                offs.push(v.len());
                v.extend_from_slice(obj);
            }
            offs.push(v.len());
            v.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
            offs
        };
        let xref_pos = v.len();
        v.extend_from_slice(b"xref\n0 3\n");
        v.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            v.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        v.extend_from_slice(
            format!(
                "trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
                xref_pos
            )
            .as_bytes(),
        );
        v
    }

    #[test]
    fn memory_source_opens_in_one_step() {
        let loader = DocumentLoader::new(Stream::from_bytes(sample_pdf()));
        let mut loader = loader;
        match loader
            .step()
            .expect("memory source should open in one step")
        {
            OpenProgress::Ready(mut doc) => {
                assert_eq!(doc.page_count().expect("pages resolve"), 0);
            }
            OpenProgress::Need(_) => panic!("memory source must not need ranges"),
        }
    }

    #[test]
    fn step_after_ready_is_an_error() {
        let mut loader = DocumentLoader::new(Stream::from_bytes(sample_pdf()));
        match loader.step().expect("open should succeed") {
            OpenProgress::Ready(_) => {}
            OpenProgress::Need(_) => panic!("memory source must not need ranges"),
        }
        assert!(
            loader.step().is_err(),
            "a finished loader must reject steps"
        );
    }

    #[test]
    fn truncated_pdf_recovers_like_buffered_open() {
        // Drop the trailer (startxref + %%EOF): the loader must fall back to the
        // full-file scan and still produce a document, like `PDFDocument::open`.
        let full = sample_pdf();
        let cut = full
            .windows(b"startxref".len())
            .position(|w| w == b"startxref")
            .expect("sample has startxref");
        let truncated = full[..cut].to_vec();

        let mut loader = DocumentLoader::new(Stream::from_bytes(truncated));
        match loader
            .step()
            .expect("recovery scan should open the truncated PDF")
        {
            OpenProgress::Ready(doc) => assert!(doc.catalog().is_some()),
            OpenProgress::Need(_) => panic!("memory source must not need ranges"),
        }
    }

    #[test]
    fn garbage_is_a_fatal_error_not_a_hang() {
        let mut loader = DocumentLoader::new(Stream::from_bytes(b"not a pdf".to_vec()));
        assert!(
            loader.step().is_err(),
            "undecodable input must terminate with an error"
        );
    }
}
