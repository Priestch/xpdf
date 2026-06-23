//! `ResolvedPage` — the immutable, I/O-free page model (decision C).
//!
//! [`DocStore::resolve_page`] produces a [`ResolvedPage`] by resolving a page's
//! `/Resources` and `/Contents` through the `DocStore` (`delta → base`). Interpretation
//! is then a pure function over a `ResolvedPage` + `Device` — no `DocStore`, no `Need`,
//! no blocking. That separation is the resolve-then-interpret boundary: it makes
//! interpretation restartable-safe (so it can't hang `drive`), lets a native shell render
//! snapshots on another thread, and is what lets delta edits to `/Resources`/`/Contents`
//! actually affect output.
//!
//! ## Step 4b scope (skeleton)
//!
//! The type and the delta-aware `resolve_page` are real and tested. What is **not** done
//! here: the render pipeline is **not** rewired to consume a `ResolvedPage` —
//! `RenderingContext` still resolves objects lazily mid-interpretation, and
//! `render_page_to_image` still uses the old path. So a `ResolvedPage` does not yet
//! participate in actual rendering. The `interpret` function exists (in the `rendering`
//! module) only as a pure-signature stub; the full engine moves onto `ResolvedPage` in
//! the Step 4a rewire. This step validates the type shape and proves the resolve seam is
//! delta-aware.
//!
//! Known skeleton gap: inherited `MediaBox`/`Rotate` from parent `Pages` nodes still
//! resolve through the base `XRef` (via `Page`'s existing resolvers) and are not yet
//! delta-aware. The page object itself, and `/Resources`/`/Contents`, ARE delta-aware.

use super::decode;
use super::doc_store::DocStore;
use super::error::{PDFError, PDFResult};
use super::page::Page;
use super::parser::PDFObject;

/// A fully-resolved, immutable, self-contained page model.
///
/// Everything needed to interpret a page, with no further object resolution required.
/// `Send + Sync + 'static` so a native shell can render it on another thread while the
/// `DocStore` keeps being edited.
#[derive(Debug, Clone)]
pub struct ResolvedPage {
    /// Intersection of MediaBox/CropBox in PDF user-space coordinates `[x0, y0, x1, y1]`.
    pub view_box: [f64; 4],
    /// Page rotation in degrees, normalized to `{0, 90, 180, 270}`.
    pub rotation: i32,
    /// Resolved `/Resources` dictionary (`delta → base`). [`PDFObject::Null`] if absent.
    pub resources: PDFObject,
    /// Decoded content-stream bytes (all content streams concatenated, filters applied).
    pub contents: Vec<u8>,
}

impl<'a> DocStore<'a> {
    /// Resolve a page to an immutable, I/O-free [`ResolvedPage`] (`delta → base`).
    ///
    /// The page object (resolved by `PDFDocument::get_page`), `/Resources`, and
    /// `/Contents` are all resolved through this `DocStore`, so delta overrides are
    /// honored. The decoded `contents` and resolved `resources` need no further object
    /// fetches during interpretation.
    pub fn resolve_page(&mut self, page: &Page) -> PDFResult<ResolvedPage> {
        // view_box / rotation reuse Page's existing resolvers. These read the page dict
        // (already delta-resolved) for the page's OWN MediaBox/Rotate; only inherited
        // values from parent nodes still go through the base XRef (see module gap note).
        let view_box = page.resolve_view_box_for_rendering(self.xref_mut());
        let rotation = page.resolve_rotate_for_rendering(self.xref_mut());

        // /Resources resolved delta-aware (direct page-dict value; inheritance is a
        // later refinement).
        let resources = match page.resources() {
            Some(r) => self.resolve_if_ref(r)?,
            None => PDFObject::Null,
        };

        let contents = self.resolve_contents(page)?;

        Ok(ResolvedPage {
            view_box,
            rotation,
            resources,
            contents,
        })
    }

    /// Resolve and decode the page's `/Contents` (single stream or array of streams).
    fn resolve_contents(&mut self, page: &Page) -> PDFResult<Vec<u8>> {
        let Some(contents_obj) = page.contents() else {
            return Ok(Vec::new());
        };
        let resolved = self.resolve_if_ref(contents_obj)?;
        let mut out = Vec::new();
        match &resolved {
            PDFObject::Stream { dict, data } => {
                out.extend(decode_stream(dict, data)?);
            }
            PDFObject::Array(arr) => {
                for obj in arr.iter() {
                    let entry = self.resolve_if_ref(obj.as_ref())?;
                    match &entry {
                        PDFObject::Stream { dict, data } => {
                            out.extend(decode_stream(dict, data)?);
                        }
                        _ => {
                            return Err(PDFError::Generic(
                                "Contents array entry is not a stream".to_string(),
                            ));
                        }
                    }
                }
            }
            // Graceful: unexpected /Contents type → no bytes (matches render's handling).
            _ => {}
        }
        Ok(out)
    }
}

/// Apply the stream's `/Filter` (if any) to its bytes.
fn decode_stream(dict: &std::collections::HashMap<String, PDFObject>, data: &[u8]) -> PDFResult<Vec<u8>> {
    match dict.get("Filter") {
        Some(f) => decode::apply_filters(data, f),
        None => Ok(data.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::parser::Ref;
    use crate::core::PDFDocument;
    use std::collections::HashMap;

    /// A one-page PDF whose page has a `/Contents` stream, so `resolve_page` exercises
    /// delta-aware resource + content resolution. Offsets computed at runtime.
    fn pdf_with_contents() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n");

        let o1 = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let o2 = buf.len();
        buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

        let o3 = buf.len();
        buf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>\nendobj\n",
        );

        let o4 = buf.len();
        // Content stream body: "q Q\n" (4 bytes), no filter.
        buf.extend_from_slice(b"4 0 obj\n<< /Length 4 >>\nstream\nq Q\nendstream\nendobj\n");

        let xref_offset = buf.len();
        buf.extend_from_slice(b"xref\n0 5\n");
        buf.extend_from_slice(format!("{:010} 65535 f \r\n", 0).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o1).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o2).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o3).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o4).as_bytes());
        buf.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
        buf.extend_from_slice(format!("{}\n", xref_offset).as_bytes());
        buf.extend_from_slice(b"%%EOF");
        buf
    }

    #[test]
    fn resolve_page_decodes_contents_and_boxes() {
        let mut doc = PDFDocument::open(pdf_with_contents()).expect("pdf should parse and open");
        let page = doc.get_page(0).expect("page 0 should resolve");
        let resolved = doc
            .doc_store_mut()
            .resolve_page(&page)
            .expect("resolve_page should produce a ResolvedPage");

        assert_eq!(resolved.view_box, [0.0, 0.0, 200.0, 300.0], "MediaBox resolved");
        assert_eq!(resolved.rotation, 0);
        assert!(matches!(resolved.resources, PDFObject::Null), "no /Resources → Null");
        assert_eq!(resolved.contents, b"q Q\n", "content stream decoded (no filter)");
    }

    #[test]
    fn resolve_page_honors_delta_override_of_contents() {
        // The seam proof: overriding the /Contents object (4 0) in the delta must change
        // the resolved content bytes — even though the render pipeline isn't rewired yet,
        // resolve_page itself is delta-aware.
        let mut doc = PDFDocument::open(pdf_with_contents()).expect("pdf should parse and open");
        let page = doc.get_page(0).expect("page 0 should resolve");

        // Baseline.
        let before = doc.doc_store_mut().resolve_page(&page).expect("baseline resolve");
        assert_eq!(before.contents, b"q Q\n");

        // Override object 4 with a different content stream.
        let mut dict: HashMap<String, PDFObject> = HashMap::new();
        dict.insert("Length".to_string(), PDFObject::Number(3.0));
        let override_stream = PDFObject::Stream {
            dict,
            data: b"Q q".to_vec(),
        };
        doc.doc_store_mut()
            .delta_mut()
            .modify_object(Ref::new(4, 0), override_stream);

        let after = doc
            .doc_store_mut()
            .resolve_page(&page)
            .expect("post-override resolve");
        assert_eq!(
            after.contents, b"Q q",
            "delta override of /Contents must be reflected in the resolved page"
        );
    }

    #[test]
    fn resolved_page_is_send_sync_static() {
        // Decision C contract: a resolved page is self-contained and can cross threads so
        // a native shell can render it off the live DocStore.
        fn assert_bounds<T: Send + Sync + 'static>() {}
        assert_bounds::<ResolvedPage>();
    }

    #[cfg(feature = "rendering")]
    #[test]
    fn interpret_needs_no_doc_store() {
        // Decision C purity contract: interpretation runs over a ResolvedPage + Device
        // with NO DocStore/Need in scope. This test fails to compile if `interpret` ever
        // gains a DocStore parameter or fetches objects.
        use crate::rendering::{interpret, TestDevice};

        let page = ResolvedPage {
            view_box: [0.0, 0.0, 10.0, 10.0],
            rotation: 0,
            resources: PDFObject::Null,
            contents: Vec::new(),
        };
        let mut device = TestDevice::new(10.0, 10.0);
        interpret(&page, &mut device);
    }

    #[test]
    fn render_honors_delta_override_of_contents() {
        // Step 4a proof: rendering through a DocStore honors a delta override of /Contents.
        // The page's /Contents is "q Q\n" (save/restore). Overriding it with empty content
        // must remove the save_state/restore_state operations from the rendered output.
        use crate::rendering::TestDevice;

        let mut doc = PDFDocument::open(pdf_with_contents()).expect("pdf should parse and open");
        let page = doc.get_page(0).expect("page 0 should resolve");

        // Baseline render: "q Q\n" → the q/Q pair becomes save_state + restore_state on
        // top of render's own per-stream save/restore and the view-box clip.
        let mut baseline = TestDevice::new(200.0, 300.0);
        {
            let mut store = doc.doc_store_mut();
            page.render(&mut store, &mut baseline).expect("baseline render");
        }
        let baseline_len = baseline.operations().len();

        // Override /Contents (object 4) with an EMPTY stream.
        let mut dict: HashMap<String, PDFObject> = HashMap::new();
        dict.insert("Length".to_string(), PDFObject::Number(0.0));
        let empty_stream = PDFObject::Stream {
            dict,
            data: Vec::new(),
        };
        doc.doc_store_mut()
            .delta_mut()
            .modify_object(Ref::new(4, 0), empty_stream);

        // Render again: empty content → the q/Q pair is gone.
        let mut after = TestDevice::new(200.0, 300.0);
        {
            let mut store = doc.doc_store_mut();
            page.render(&mut store, &mut after).expect("post-override render");
        }

        // The delta override changed rendered output: the q (save_state) + Q (restore_state)
        // from the original content are no longer emitted — exactly two fewer device ops.
        assert_eq!(
            baseline_len,
            after.operations().len() + 2,
            "delta override of /Contents must remove the q/Q save-restore pair from render"
        );
    }
}
