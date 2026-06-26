//! `DocStore` — the single `delta → base` object-resolution authority.
//!
//! This is the **Phase 0 decision B** type from `CLAUDE.md`. Every object lookup goes
//! through [`DocStore::resolve_ref`] / [`DocStore::resolve_if_ref`]: the delta is checked
//! first (cheap, local — and it may even *avoid* a base fetch), then the base `XRef`.
//!
//! ## Step 2 scope
//!
//! `DocStore` is introduced as a **borrowed view** over `(&mut XRef, &mut DeltaLayer)`,
//! owned by [`crate::core::PDFDocument`] and obtained via `PDFDocument::doc_store_mut()`.
//! It is *not* the owner of the `XRef` yet — that ownership move is a later step, to keep
//! this step behavior-preserving and low-risk (the 29 existing `self.xref` sites are left
//! untouched). The resolution seam is nonetheless **live**: page-tree object resolution in
//! `PDFDocument::get_page_dict` routes through `resolve_ref`, so the delta is consulted
//! for real from day one (delta is empty in v1, so behavior is identical to the base).
//!
//! Deeply-nested helpers in `page.rs` still take `&mut XRef` directly for now; migrating
//! every fetch site is a later, incremental step. Because the delta is empty, this is
//! behavior-preserving.

use super::delta::DeltaLayer;
use super::error::{PDFError, PDFResult};
use super::parser::{PDFObject, Ref};
use super::xref::XRef;

/// The `delta → base` object-resolution authority, borrowed over a document's
/// `XRef` (base) and `DeltaLayer` (edits).
///
/// Constructed via [`PDFDocument::doc_store_mut`](crate::core::PDFDocument::doc_store_mut).
/// All object resolution should go through this type rather than through `XRef` directly,
/// so that edits are honored.
pub struct DocStore<'a> {
    xref: &'a mut XRef,
    delta: &'a mut DeltaLayer,
}

impl<'a> DocStore<'a> {
    /// Borrow the base `XRef` and the `DeltaLayer` as a resolution authority.
    pub fn new(xref: &'a mut XRef, delta: &'a mut DeltaLayer) -> Self {
        Self { xref, delta }
    }

    /// Resolve an indirect object reference: `delta → base`.
    ///
    /// Order: deleted (deletion wins — `delete_object` also clears any prior modification)
    /// → modified/new override → base `XRef` fetch. Returns a **cloned** owned object.
    pub fn resolve_ref(&mut self, num: u32, generation: u32) -> PDFResult<PDFObject> {
        let key = Ref::new(num, generation);
        if self.delta.is_deleted(&key) {
            return Err(PDFError::ObjectDeleted { num, generation });
        }
        if let Some(delta_obj) = self.delta.get(&key) {
            return Ok(delta_obj.object.clone());
        }
        // Fall back to the base PDF (progressive loading lives inside XRef today).
        let fetched = self.xref.fetch(num, generation)?;
        Ok((*fetched).clone())
    }

    /// Resolve `obj` if it is an indirect reference; otherwise return a clone as-is.
    ///
    /// Mirrors `XRef::fetch_if_ref`, but delta-aware.
    pub fn resolve_if_ref(&mut self, obj: &PDFObject) -> PDFResult<PDFObject> {
        match obj {
            PDFObject::Ref(r) => self.resolve_ref(r.num, r.generation),
            other => Ok(other.clone()),
        }
    }

    /// Escape hatch to the base `XRef` for code paths not yet migrated to `resolve_*`.
    ///
    /// The delta is empty in v1, so consulting the base directly is behavior-preserving.
    /// New code should prefer `resolve_ref` / `resolve_if_ref`.
    pub fn xref(&self) -> &XRef {
        self.xref
    }

    /// Mutable escape hatch to the base `XRef`.
    pub fn xref_mut(&mut self) -> &mut XRef {
        self.xref
    }

    /// The edit surface (commands write here). Local-only; never serialized/merged.
    pub fn delta(&self) -> &DeltaLayer {
        self.delta
    }

    /// Mutable edit surface.
    pub fn delta_mut(&mut self) -> &mut DeltaLayer {
        self.delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::PDFDocument;

    /// Build a minimal but valid single-page PDF with a **classic xref table**, with
    /// offsets computed at runtime so it is guaranteed well-formed. The repo's
    /// `sample.pdf` uses an xref stream the parser doesn't yet handle; this avoids
    /// depending on any external fixture.
    fn minimal_pdf() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n");

        let o1 = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let o2 = buf.len();
        buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

        let o3 = buf.len();
        buf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
        );

        let xref_offset = buf.len();
        buf.extend_from_slice(b"xref\n0 4\n");
        buf.extend_from_slice(format!("{:010} 65535 f \r\n", 0).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o1).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o2).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \r\n", o3).as_bytes());
        buf.extend_from_slice(b"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n");
        buf.extend_from_slice(format!("{}\n", xref_offset).as_bytes());
        buf.extend_from_slice(b"%%EOF");
        buf
    }

    /// Open the minimal doc and find an object number that the base `XRef` can fetch.
    fn first_fetchable_object(doc: &mut PDFDocument) -> (u32, PDFObject) {
        let count = doc.xref().len();
        for num in 1..=(count as u32) {
            if let Ok(obj) = doc.xref_mut().fetch(num, 0) {
                return (num, (*obj).clone());
            }
        }
        panic!("minimal doc has no fetchable objects");
    }

    #[test]
    fn resolve_ref_matches_base_when_delta_empty() {
        let mut doc = PDFDocument::open(minimal_pdf()).expect("minimal pdf should parse and open");
        let (num, base_obj) = first_fetchable_object(&mut doc);

        let mut store = doc.doc_store_mut();
        let resolved = store
            .resolve_ref(num, 0)
            .expect("resolve_ref should return the base object (delta empty)");
        assert_eq!(
            resolved, base_obj,
            "delta empty → resolve must equal base fetch"
        );
    }

    #[test]
    fn resolve_ref_returns_delta_override() {
        let mut doc = PDFDocument::open(minimal_pdf()).expect("minimal pdf should parse and open");
        let (num, base_obj) = first_fetchable_object(&mut doc);
        assert_ne!(
            base_obj,
            PDFObject::Boolean(true),
            "sanity: base differs from override"
        );

        let mut store = doc.doc_store_mut();
        store
            .delta_mut()
            .modify_object(Ref::new(num, 0), PDFObject::Boolean(true));

        let resolved = store
            .resolve_ref(num, 0)
            .expect("resolve_ref should return the delta override");
        assert_eq!(
            resolved,
            PDFObject::Boolean(true),
            "delta override must win over base"
        );
    }

    #[test]
    fn resolve_ref_errors_when_deleted() {
        let mut doc = PDFDocument::open(minimal_pdf()).expect("minimal pdf should parse and open");
        let (num, _) = first_fetchable_object(&mut doc);

        let mut store = doc.doc_store_mut();
        store.delta_mut().delete_object(Ref::new(num, 0));

        let err = store
            .resolve_ref(num, 0)
            .expect_err("deleted object must not resolve");
        assert!(matches!(err, PDFError::ObjectDeleted { num: n, .. } if n == num));
    }

    #[test]
    fn resolve_if_ref_passes_through_non_refs() {
        let mut doc = PDFDocument::open(minimal_pdf()).expect("minimal pdf should parse and open");
        let mut store = doc.doc_store_mut();
        // A non-Ref object is returned as-is.
        let passthrough = store.resolve_if_ref(&PDFObject::Number(42.0)).unwrap();
        assert_eq!(passthrough, PDFObject::Number(42.0));
    }

    #[test]
    fn get_page_resolves_through_doc_store() {
        // The page-tree traversal in PDFDocument::get_page_dict routes through DocStore
        // (delta → base). With an empty delta, page 0 resolves identically to the base.
        let mut doc = PDFDocument::open(minimal_pdf()).expect("minimal pdf should parse and open");
        let page = doc.get_page(0).expect("page 0 should resolve via DocStore");
        assert_eq!(page.index(), 0);
        assert!(
            page.media_box().is_some(),
            "page inherited/fetched a MediaBox"
        );
    }

    #[test]
    fn open_file_drives_progressive_load() {
        // Step 3b acceptance: the chunked open_file path drives xref.parse() through
        // `drive` (was retry_on_data_missing!). Uses the default 64 KB chunk size, which
        // is how open_file is used in practice: the file fits in one chunk, open_file
        // preloads the xref region, and parse completes in a single driven step.
        //
        // NOTE: a tiny chunk size (e.g. 64) forces parse into the missing-data path and
        // exposes a SEPARATE pre-existing bug — parse/lexer loops instead of returning
        // DataMissing on a partially-loaded stream. That bug is out of scope for the
        // drive migration (it affects the legacy macro equally) and doesn't manifest in
        // normal use because open_file preloads the xref region before parsing.
        use std::io::Write;
        use tempfile::NamedTempFile;

        let data = minimal_pdf();
        let tmp = NamedTempFile::new().expect("temp file should be created");
        tmp.as_file()
            .write_all(&data)
            .expect("temp file should be writable");
        tmp.as_file().sync_all().ok();

        let mut doc = PDFDocument::open_file(tmp.path(), None, None)
            .expect("open_file should drive the progressive load to completion");
        let page = doc
            .get_page(0)
            .expect("page 0 should resolve after the driven load");
        assert_eq!(page.index(), 0);
    }
}
