//! `DocStore` — the single `delta → base` object-resolution authority.
//!
//! This is the **decision B** type from `CLAUDE.md`. Every object lookup goes through
//! [`DocStore::resolve_ref`] / [`DocStore::resolve_if_ref`]: the delta is checked first
//! (cheap, local — and it may even *avoid* a base fetch), then the base `XRef`.
//!
//! `DocStore` owns the `XRef` and `DeltaLayer` directly (Phase 1 migration: was a borrowed
//! view over `(&mut XRef, &mut DeltaLayer)`). Owned by [`crate::core::PDFDocument`].

use super::delta::DeltaLayer;
use super::error::{PDFError, PDFResult};
use super::parser::{PDFObject, Ref};
use super::resource_budget::ResourceBudget;
use super::xref::XRef;
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::sync::Arc;

/// The `delta → base` object-resolution authority.
///
/// Owns the base `XRef` and the edit `DeltaLayer`. Constructed by
/// [`crate::core::PDFDocument`] and accessed via
/// [`crate::core::PDFDocument::doc_store_mut`].
pub struct DocStore {
    /// The base cross-reference table (owns the stream + cache + all object data).
    xref: XRef,
    /// The edit delta overlaid on the base.
    delta: DeltaLayer,
    /// Limits applied while decoding and resolving untrusted input.
    resource_budget: ResourceBudget,
    /// Root `/Pages` object reference, seeded by `PDFDocument::from_xref` once the
    /// catalog is known. Lets link/GoTo destinations resolve page references to
    /// indices without a `PDFDocument` handle (see [`DocStore::resolve_page_index`]).
    root_pages_ref: Option<Ref>,
    /// `(page ref → 0-based index)` map over the page tree, built lazily by
    /// [`DocStore::resolve_page_index`] and invalidated when the delta revision
    /// changes (commands may move/delete pages).
    page_index_cache: FxHashMap<(u32, u32), usize>,
    /// The `delta.revision()` value [`Self::page_index_cache`] was built at
    /// (`None` = not built yet).
    page_index_cache_revision: Option<u64>,
}

impl DocStore {
    /// Take ownership of the base `XRef` and the edit `DeltaLayer`.
    pub(crate) fn new(xref: XRef, delta: DeltaLayer) -> Self {
        Self {
            xref,
            delta,
            resource_budget: ResourceBudget::default(),
            root_pages_ref: None,
            page_index_cache: FxHashMap::default(),
            page_index_cache_revision: None,
        }
    }

    /// Seed the root `/Pages` reference (called once by the document loader).
    pub(crate) fn set_root_pages_ref(&mut self, root_pages_ref: Ref) {
        self.root_pages_ref = Some(root_pages_ref);
        self.page_index_cache.clear();
        self.page_index_cache_revision = None;
    }

    /// Current limits for decoding and page resolution.
    pub fn resource_budget(&self) -> ResourceBudget {
        self.resource_budget
    }

    /// Replace limits for subsequent decoding and page resolution.
    pub fn set_resource_budget(&mut self, budget: ResourceBudget) {
        self.resource_budget = budget;
    }

    /// Resolve an indirect object reference: `delta → base`.
    ///
    /// Order: deleted (deletion wins — `delete_object` also clears any prior modification)
    /// → modified/new override → base `XRef` fetch. Returns an `Arc<PDFObject>` — cheap
    /// clone (refcount inc) from the XRef cache; delta overrides are Arc-wrapped clones.
    pub fn resolve_ref(&mut self, num: u32, generation: u32) -> PDFResult<Arc<PDFObject>> {
        let key = Ref::new(num, generation);
        if self.delta.is_deleted(&key) {
            return Err(PDFError::ObjectDeleted { num, generation });
        }
        if let Some(delta_obj) = self.delta.get(&key) {
            return Ok(Arc::new(delta_obj.object.clone()));
        }
        // Fall back to the base PDF (progressive loading lives inside XRef today).
        self.xref.fetch(num, generation)
    }

    /// Resolve `obj` if it is an indirect reference; otherwise wrap in an Arc.
    ///
    /// Mirrors `XRef::fetch_if_ref`, but delta-aware.
    pub fn resolve_if_ref(&mut self, obj: &PDFObject) -> PDFResult<Arc<PDFObject>> {
        match obj {
            PDFObject::Ref(r) => self.resolve_ref(r.num, r.generation),
            other => Ok(Arc::new(other.clone())),
        }
    }

    /// Resolve a page object reference to its 0-based page index.
    ///
    /// Walks the page tree from the seeded root `/Pages` (delta-aware — commands
    /// that move/delete pages bump the revision and invalidate the cache),
    /// assigning indices in document order. Reference: pdf.js `Catalog#getAllPageDicts`
    /// (`catalog.js:1461`) — the same explicit-stack DFS with a cycle guard — with
    /// lookups served from `pageIndexCache` like `Catalog#getPageIndex`
    /// (`catalog.js:1590`).
    ///
    /// Returns `None` when no root has been seeded (resolvers without a page-tree
    /// view) or the reference is not a page in the tree.
    pub fn resolve_page_index(&mut self, num: u32, generation: u32) -> Option<usize> {
        let root_pages_ref = self.root_pages_ref?;
        let revision = self.delta.revision();
        if self.page_index_cache_revision != Some(revision) {
            self.page_index_cache.clear();
            self.fill_page_index_cache(root_pages_ref);
            self.page_index_cache_revision = Some(revision);
        }
        self.page_index_cache.get(&(num, generation)).copied()
    }

    /// Build the `(page ref → index)` map over the whole page tree.
    ///
    /// Mirrors `build_page_ref_cache` (document.rs) but lives here so the
    /// [`super::page::PageResolver`] method works over `&mut DocStore` alone.
    /// Errors are swallowed: a corrupt tree degrades to "page not found"
    /// (`None` from the caller), never a hard failure.
    fn fill_page_index_cache(&mut self, root_pages_ref: Ref) {
        let mut nodes_to_visit: Vec<PDFObject> = vec![PDFObject::Ref(root_pages_ref)];
        let mut visited_refs: HashSet<(u32, u32)> = HashSet::new();
        let mut current_page_index = 0usize;

        while let Some(current_node) = nodes_to_visit.pop() {
            let (node_obj, obj_ref) = match &current_node {
                PDFObject::Ref(ref_obj) => {
                    let ref_key = (ref_obj.num, ref_obj.generation);
                    // Prevent circular references.
                    if !visited_refs.insert(ref_key) {
                        continue;
                    }
                    match self.resolve_ref(ref_obj.num, ref_obj.generation) {
                        Ok(obj) => (obj, Some(ref_key)),
                        Err(_) => continue,
                    }
                }
                _ => (Arc::new(current_node), None),
            };

            let dict = match &*node_obj {
                PDFObject::Dictionary(d) => d,
                _ => continue,
            };

            // Discriminate leaves the same way `build_page_ref_cache` does.
            let is_page = match dict.get("Type") {
                Some(PDFObject::Name(name)) => name == "Page",
                _ => !dict.contains_key("Kids"),
            };
            if is_page {
                if let Some(ref_key) = obj_ref {
                    self.page_index_cache.insert(ref_key, current_page_index);
                }
                current_page_index += 1;
                continue;
            }

            // Intermediate Pages node — collect its Kids (direct or indirect).
            let kids_array = match dict.get("Kids") {
                Some(PDFObject::Array(arr)) => Some(arr.clone()),
                Some(PDFObject::Ref(ref_obj)) => {
                    let fetched = self.resolve_ref(ref_obj.num, ref_obj.generation);
                    match fetched {
                        Ok(obj) => match &*obj {
                            PDFObject::Array(arr) => Some(arr.clone()),
                            _ => None,
                        },
                        Err(_) => None,
                    }
                }
                _ => None,
            };
            let Some(kids_array) = kids_array else {
                continue;
            };

            // Push in reverse so the DFS pops kids in document order.
            for kid in kids_array.iter().rev() {
                nodes_to_visit.push((**kid).clone());
            }
        }
    }

    /// Escape hatch to the base `XRef` for code paths not yet migrated to `resolve_*`.
    ///
    /// The delta is empty in v1, so consulting the base directly is behavior-preserving.
    /// New code should prefer `resolve_ref` / `resolve_if_ref`.
    pub fn xref(&self) -> &XRef {
        &self.xref
    }

    /// Mutable escape hatch to the base `XRef`.
    pub fn xref_mut(&mut self) -> &mut XRef {
        &mut self.xref
    }

    /// The edit surface (commands write here). Local-only; never serialized/merged.
    pub fn delta(&self) -> &DeltaLayer {
        &self.delta
    }

    /// Mutable edit surface.
    pub fn delta_mut(&mut self) -> &mut DeltaLayer {
        &mut self.delta
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
            *resolved, base_obj,
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
            *resolved,
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
        assert_eq!(*passthrough, PDFObject::Number(42.0));
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
}
