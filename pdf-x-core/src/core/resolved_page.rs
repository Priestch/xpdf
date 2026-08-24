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
use super::page::{Page, PageResolver};
use super::parser::PDFObject;
use std::sync::Arc;

/// A fully-resolved, immutable, self-contained page model.
///
/// Everything needed to interpret a page, with no further object resolution required.
/// `Send + Sync + 'static` so a native shell can render it on another thread while the
/// `DocStore` keeps being edited.
#[derive(Debug, Clone)]
pub struct ResolvedPage {
    /// Intersection of MediaBox/CropBox in PDF user-space coordinates `[x0, y0, x1, y1]`.
    view_box: [f64; 4],
    /// Page rotation in degrees, normalized to `{0, 90, 180, 270}`.
    rotation: i32,
    /// Resolved `/Resources` dictionary (`delta → base`). [`PDFObject::Null`] if absent.
    resources: PDFObject,
    /// Decoded content-stream bytes (all content streams concatenated, filters applied).
    contents: Vec<u8>,
    /// Pre-resolved snapshot of every object reachable from `/Resources` and from
    /// each annotation's `/AP` appearance graph (fonts, xobjects, appearance streams,
    /// ext-gstate, color spaces, and their sub-objects, recursively). Action/dest
    /// refs on annots are **not** followed — that would pull destination page trees.
    /// Captured during `resolve_page` so `interpret` needs no live `DocStore`.
    /// Keyed by `(object_num, generation)`. Uses `Arc<PDFObject>` so the snapshot
    /// shares allocations with the XRef cache (no deep clone on capture).
    objects: std::collections::HashMap<(u32, u32), Arc<PDFObject>>,
    /// Resolved annotation dictionaries for this page (Widget/AP appearances, etc.).
    /// Empty when `/Annots` is absent. Only `/AP` appearance streams (and their
    /// resources) are folded into `objects` — not `/A`/`/Dest`/`/P` graphs.
    /// Source: hayro `interpret_page` annotation pass (PDF 32000-1 §12.5.5).
    annotations: Vec<PDFObject>,
}

impl ResolvedPage {
    /// Intersection of MediaBox/CropBox `[x0, y0, x1, y1]`.
    pub fn view_box(&self) -> [f64; 4] {
        self.view_box
    }
    /// Page rotation in degrees (`{0, 90, 180, 270}`).
    pub fn rotation(&self) -> i32 {
        self.rotation
    }
    /// Resolved `/Resources` dictionary.
    pub fn resources(&self) -> &PDFObject {
        &self.resources
    }
    /// Decoded content-stream bytes.
    pub fn contents(&self) -> &[u8] {
        &self.contents
    }
    /// Pre-resolved object snapshot (for `SnapshotPageResolver`).
    pub fn objects(&self) -> &std::collections::HashMap<(u32, u32), Arc<PDFObject>> {
        &self.objects
    }
    /// Resolved page annotation dictionaries (for appearance-stream painting).
    pub fn annotations(&self) -> &[PDFObject] {
        &self.annotations
    }
}

impl DocStore {
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
        let view_box = page.resolve_view_box_for_rendering(self);
        let rotation = page.resolve_rotate_for_rendering(self);

        // /Resources is inheritable in the page tree. Resolve it through the
        // same page helper used by the legacy render path so parent Pages
        // resource dictionaries are available during pure interpretation.
        let resources = page
            .get_inheritable_resources(self)?
            .unwrap_or_else(|| Arc::new(PDFObject::Null));

        let budget = self.resource_budget();
        let contents = self.resolve_contents(page, budget)?;

        // Capture every object reachable from /Resources into an immutable snapshot, so
        // `interpret` can run with no live DocStore. RenderingContext fetches resources
        // by name from /Resources, so walking its ref-graph captures all fetch targets
        // (fonts/xobjects/ext-gstate/color spaces + their sub-objects, recursively).
        let mut objects = std::collections::HashMap::new();
        capture_reachable_into(self, &resources, &mut objects, budget)?;

        // Annotation appearance streams (Widget `/AP /N` Form XObjects) are not under
        // /Resources. Beamer-style animated PDFs put almost all slide content there
        // (`animated-distributions.pdf`). Resolve `/Annots` and fold **appearance**
        // ref-graphs into the same snapshot so pure interpret can paint them.
        //
        // Important: do NOT walk the full annotation dict. Link annotations carry
        // `/A /D` destinations and `/P` parent-page refs that point at other pages;
        // following those pulls the destination page tree (and often most of the
        // document) into the snapshot. A real index page with ~100 links then
        // spends minutes resolving unrelated pages.
        // Source: hayro/hayro-interpret/src/interpret/mod.rs `interpret_page`
        // (only reads `/AP /N`, never action/dest graphs).
        let annotations = self.resolve_annotations(page, &mut objects, budget)?;

        Ok(ResolvedPage {
            view_box,
            rotation,
            resources: (*resources).clone(),
            contents,
            objects,
            annotations,
        })
    }

    /// Resolve `/Annots` and merge each annotation's **appearance** objects into `objects`.
    ///
    /// Only `/AP` (and its transitive refs: `/N` Form XObject + its `/Resources`) is
    /// captured. Action/dest/parent refs on the annot dict are intentionally ignored.
    fn resolve_annotations(
        &mut self,
        page: &Page,
        objects: &mut std::collections::HashMap<(u32, u32), Arc<PDFObject>>,
        budget: crate::core::resource_budget::ResourceBudget,
    ) -> PDFResult<Vec<PDFObject>> {
        let Some(annots_obj) = page.annotations() else {
            return Ok(Vec::new());
        };
        let resolved = self.resolve_if_ref(annots_obj)?;
        let arr = match &*resolved {
            PDFObject::Array(a) => a,
            PDFObject::Dictionary(_) => {
                // Single annotation written without an array wrapper.
                Self::capture_annotation_appearance(self, &resolved, objects, budget)?;
                return Ok(vec![(*resolved).clone()]);
            }
            _ => return Ok(Vec::new()),
        };

        let mut out = Vec::with_capacity(arr.len());
        for entry in arr.iter() {
            let annot = match self.resolve_if_ref(entry.as_ref()) {
                Ok(a) => a,
                Err(_) => continue,
            };
            Self::capture_annotation_appearance(self, &annot, objects, budget)?;
            out.push((*annot).clone());
        }
        Ok(out)
    }

    /// Snapshot only the objects needed to paint an annotation's Normal appearance.
    fn capture_annotation_appearance(
        store: &mut DocStore,
        annot: &PDFObject,
        objects: &mut std::collections::HashMap<(u32, u32), Arc<PDFObject>>,
        budget: crate::core::resource_budget::ResourceBudget,
    ) -> PDFResult<()> {
        let PDFObject::Dictionary(dict) = annot else {
            return Ok(());
        };
        // Appearance streams live under `/AP` (dict of states, or rarely a stream).
        // Nothing else on the annot is needed for paint_annotation_appearances.
        let Some(ap) = dict.get("AP") else {
            return Ok(());
        };
        capture_reachable_into(store, ap, objects, budget)?;
        Ok(())
    }

    /// Resolve and decode the page's `/Contents` (single stream or array of streams).
    fn resolve_contents(
        &mut self,
        page: &Page,
        budget: crate::core::resource_budget::ResourceBudget,
    ) -> PDFResult<Vec<u8>> {
        let Some(contents_obj) = page.contents() else {
            return Ok(Vec::new());
        };
        let resolved = self.resolve_if_ref(contents_obj)?;
        let mut out = Vec::new();
        match &*resolved {
            PDFObject::Stream { dict, data } => {
                append_decoded_stream(&mut out, dict, data, budget)?;
            }
            PDFObject::Array(arr) => {
                for obj in arr.iter() {
                    let entry = self.resolve_if_ref(obj.as_ref())?;
                    match &*entry {
                        PDFObject::Stream { dict, data } => {
                            append_decoded_stream(&mut out, dict, data, budget)?;
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

/// Decode one content stream and enforce the cumulative page-content budget.
fn append_decoded_stream(
    out: &mut Vec<u8>,
    dict: &std::collections::HashMap<String, PDFObject>,
    data: &[u8],
    budget: crate::core::resource_budget::ResourceBudget,
) -> PDFResult<()> {
    let decoded = match dict.get("Filter") {
        Some(PDFObject::Name(name)) if name == "FlateDecode" || name == "Fl" => {
            decode::decode_flate_with_budget(data, budget)?
        }
        Some(f) => decode::apply_filters(data, f)?,
        None => data.to_vec(),
    };
    let actual = out.len().saturating_add(decoded.len());
    if actual > budget.max_decompressed_bytes {
        return Err(PDFError::LimitExceeded {
            resource: "resolved page content bytes",
            limit: budget.max_decompressed_bytes,
            actual,
        });
    }
    out.extend(decoded);
    Ok(())
}

/// Resolve every indirect object reachable from `root` into `snapshot` while
/// enforcing object-count, depth and retained-byte limits.
fn capture_reachable_into(
    store: &mut DocStore,
    root: &PDFObject,
    snapshot: &mut std::collections::HashMap<(u32, u32), Arc<PDFObject>>,
    budget: crate::core::resource_budget::ResourceBudget,
) -> PDFResult<()> {
    let mut retained_bytes = snapshot
        .values()
        .map(|object| estimate_object_bytes(object))
        .sum::<usize>();
    let mut queue = find_refs_bounded(root, budget.max_reference_depth)?
        .into_iter()
        .map(|id| (id, 1usize))
        .collect::<Vec<_>>();

    while let Some((id, depth)) = queue.pop() {
        if snapshot.contains_key(&id) {
            continue;
        }
        if depth > budget.max_reference_depth {
            return Err(PDFError::LimitExceeded {
                resource: "resolved reference depth",
                limit: budget.max_reference_depth,
                actual: depth,
            });
        }
        if snapshot.len() >= budget.max_resolved_objects {
            return Err(PDFError::LimitExceeded {
                resource: "resolved object count",
                limit: budget.max_resolved_objects,
                actual: snapshot.len().saturating_add(1),
            });
        }

        let object = match store.resolve_ref(id.0, id.1) {
            Ok(object) => object,
            Err(PDFError::LimitExceeded {
                resource,
                limit,
                actual,
            }) => {
                return Err(PDFError::LimitExceeded {
                    resource,
                    limit,
                    actual,
                });
            }
            Err(_) => continue,
        };
        let object_bytes = estimate_object_bytes(&object);
        retained_bytes = retained_bytes.saturating_add(object_bytes);
        if retained_bytes > budget.max_resolved_page_bytes {
            return Err(PDFError::LimitExceeded {
                resource: "resolved page snapshot bytes",
                limit: budget.max_resolved_page_bytes,
                actual: retained_bytes,
            });
        }
        queue.extend(
            find_refs_bounded(&object, budget.max_reference_depth)?
                .into_iter()
                .map(|child| (child, depth.saturating_add(1))),
        );
        snapshot.insert(id, object);
    }
    Ok(())
}

/// Collect references without recursive Rust calls so hostile container nesting
/// cannot overflow the process stack.
fn find_refs_bounded(obj: &PDFObject, max_depth: usize) -> PDFResult<Vec<(u32, u32)>> {
    let mut refs = Vec::new();
    let mut stack = vec![(obj, 0usize)];
    while let Some((current, depth)) = stack.pop() {
        if depth > max_depth {
            return Err(PDFError::LimitExceeded {
                resource: "PDF object container depth",
                limit: max_depth,
                actual: depth,
            });
        }
        match current {
            PDFObject::Ref(reference) => refs.push((reference.num, reference.generation)),
            PDFObject::Dictionary(dict) => {
                stack.extend(dict.values().map(|value| (value, depth.saturating_add(1))));
            }
            PDFObject::Array(array) => {
                stack.extend(
                    array
                        .iter()
                        .map(|value| (value.as_ref(), depth.saturating_add(1))),
                );
            }
            PDFObject::Stream { dict, .. } => {
                stack.extend(dict.values().map(|value| (value, depth.saturating_add(1))));
            }
            _ => {}
        }
    }
    Ok(refs)
}

fn estimate_object_bytes(root: &PDFObject) -> usize {
    let mut total = std::mem::size_of::<PDFObject>();
    let mut stack = vec![root];
    while let Some(object) = stack.pop() {
        match object {
            PDFObject::String(bytes)
            | PDFObject::HexString(bytes)
            | PDFObject::InlineImageData(bytes) => total = total.saturating_add(bytes.len()),
            PDFObject::Name(name) | PDFObject::Command(name) => {
                total = total.saturating_add(name.len())
            }
            PDFObject::Dictionary(dict) => {
                total = total.saturating_add(dict.keys().map(String::len).sum::<usize>());
                stack.extend(dict.values());
            }
            PDFObject::Array(array) => stack.extend(array.iter().map(|item| item.as_ref())),
            PDFObject::Stream { dict, data } => {
                total = total.saturating_add(data.len());
                total = total.saturating_add(dict.keys().map(String::len).sum::<usize>());
                stack.extend(dict.values());
            }
            _ => {}
        }
    }
    total
}

/// A [`PageResolver`] backed by a frozen object snapshot (no live `DocStore`).
///
/// Used by `interpret` to run `RenderingContext` purely: `resolve_*` look up the
/// pre-resolved snapshot, so interpretation performs no I/O and cannot block or emit `Need`.
/// `as_xref_mut` is unreachable during interpretation (RenderingContext fetches only via
/// `resolve_*`).
pub struct SnapshotPageResolver<'o> {
    objects: &'o std::collections::HashMap<(u32, u32), Arc<PDFObject>>,
}

impl<'o> SnapshotPageResolver<'o> {
    /// Wrap a frozen object snapshot as a [`PageResolver`] for pure interpretation.
    pub fn new(objects: &'o std::collections::HashMap<(u32, u32), Arc<PDFObject>>) -> Self {
        Self { objects }
    }
}

impl<'o> PageResolver for SnapshotPageResolver<'o> {
    fn resolve_if_ref(&mut self, obj: &PDFObject) -> PDFResult<Arc<PDFObject>> {
        match obj {
            PDFObject::Ref(r) => self.resolve_ref(r.num, r.generation),
            other => Ok(Arc::new(other.clone())),
        }
    }

    fn resolve_ref(&mut self, num: u32, generation: u32) -> PDFResult<Arc<PDFObject>> {
        self.objects
            .get(&(num, generation))
            .map(|obj| Arc::clone(obj))
            .ok_or_else(|| {
                PDFError::Generic(format!(
                    "object {num} {generation} not present in resolved snapshot"
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::PDFDocument;
    use crate::core::parser::Ref;
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

        assert_eq!(
            resolved.view_box(),
            [0.0, 0.0, 200.0, 300.0],
            "MediaBox resolved"
        );
        assert_eq!(resolved.rotation(), 0);
        assert!(
            matches!(resolved.resources(), PDFObject::Null),
            "no /Resources → Null"
        );
        assert_eq!(
            resolved.contents(),
            b"q Q\n",
            "content stream decoded (no filter)"
        );
    }

    /// Two-page PDF: page 0 has Link annots whose `/A /D` points at page 1, plus one
    /// Widget with an `/AP /N` Form XObject. Used to prove resolve_page captures the
    /// appearance graph but never the destination page tree.
    fn pdf_with_link_annots_to_other_page() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n");

        let o1 = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let o2 = buf.len();
        buf.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n",
        );

        // Page 0: empty content, two annots.
        let o3 = buf.len();
        buf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] \
              /Contents 5 0 R /Annots [10 0 R 11 0 R] >>\nendobj\n",
        );

        // Page 1: destination page with a Form XObject resource that must NOT be
        // pulled into page 0's snapshot via link destinations.
        let o4 = buf.len();
        buf.extend_from_slice(
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] \
              /Contents 6 0 R /Resources << /XObject << /DestFm 7 0 R >> >> >>\nendobj\n",
        );

        let o5 = buf.len();
        buf.extend_from_slice(b"5 0 obj\n<< /Length 4 >>\nstream\nq Q\nendstream\nendobj\n");

        let o6 = buf.len();
        buf.extend_from_slice(b"6 0 obj\n<< /Length 4 >>\nstream\nq Q\nendstream\nendobj\n");

        // Destination-only form (unique marker stream body).
        let o7 = buf.len();
        buf.extend_from_slice(
            b"7 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 9 >>\n\
              stream\nDESTONLY\nendstream\nendobj\n",
        );

        // Link with no /AP — only /A /D → page 1. Old capture_reachable(annot) would
        // follow this into page 1's resources.
        let o10 = buf.len();
        buf.extend_from_slice(
            b"10 0 obj\n<< /Type /Annot /Subtype /Link /Rect [10 10 20 20] \
              /Border [0 0 0] /A << /S /GoTo /D [4 0 R /Fit] >> /P 3 0 R >>\nendobj\n",
        );

        // Link with /AP /N form appearance (object 12) AND a dest to page 1.
        let o11 = buf.len();
        buf.extend_from_slice(
            b"11 0 obj\n<< /Type /Annot /Subtype /Link /Rect [30 30 40 40] \
              /Border [0 0 0] /A << /S /GoTo /D [4 0 R /Fit] >> \
              /AP << /N 12 0 R >> /P 3 0 R >>\nendobj\n",
        );

        let o12 = buf.len();
        buf.extend_from_slice(
            b"12 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 8 >>\n\
              stream\nAPFORM\nendstream\nendobj\n",
        );

        let xref_offset = buf.len();
        // Objects: 1..7, 10..12 — size must cover highest number + 1.
        buf.extend_from_slice(b"xref\n0 13\n");
        buf.extend_from_slice(format!("{:010} 65535 f \r\n", 0).as_bytes());
        for off in [o1, o2, o3, o4, o5, o6, o7] {
            buf.extend_from_slice(format!("{:010} 00000 n \r\n", off).as_bytes());
        }
        // free 8, 9
        buf.extend_from_slice(format!("{:010} 00000 f \r\n", 0).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 f \r\n", 0).as_bytes());
        for off in [o10, o11, o12] {
            buf.extend_from_slice(format!("{:010} 00000 n \r\n", off).as_bytes());
        }
        buf.extend_from_slice(b"trailer\n<< /Size 13 /Root 1 0 R >>\nstartxref\n");
        buf.extend_from_slice(format!("{}\n", xref_offset).as_bytes());
        buf.extend_from_slice(b"%%EOF");
        buf
    }

    #[test]
    fn resolve_page_captures_annot_ap_not_dest_page_tree() {
        let mut doc = PDFDocument::open(pdf_with_link_annots_to_other_page())
            .expect("pdf with link annots should open");
        let page = doc.get_page(0).expect("page 0");
        let resolved = doc
            .doc_store_mut()
            .resolve_page(&page)
            .expect("resolve_page");

        assert_eq!(
            resolved.annotations().len(),
            2,
            "both Link annots must be listed"
        );

        // Appearance Form XObject (12 0) is required for paint_annotation_appearances.
        assert!(
            resolved.objects().contains_key(&(12, 0)),
            "must snapshot /AP /N Form XObject"
        );

        // Destination page (4), its contents (6), and its exclusive resource (7) must
        // NOT be pulled in via /A /D or /P. That was the multi-minute regression on
        // index pages with hundreds of naked Link annots.
        for id in [4u32, 6, 7] {
            assert!(
                !resolved.objects().contains_key(&(id, 0)),
                "must not snapshot dest-page object {} via link actions",
                id
            );
        }
    }

    #[test]
    fn resolve_page_honors_delta_override_of_contents() {
        // The seam proof: overriding the /Contents object (4 0) in the delta must change
        // the resolved content bytes — even though the render pipeline isn't rewired yet,
        // resolve_page itself is delta-aware.
        let mut doc = PDFDocument::open(pdf_with_contents()).expect("pdf should parse and open");
        let page = doc.get_page(0).expect("page 0 should resolve");

        // Baseline.
        let before = doc
            .doc_store_mut()
            .resolve_page(&page)
            .expect("baseline resolve");
        assert_eq!(before.contents(), b"q Q\n");

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
            after.contents(),
            b"Q q",
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
        use crate::rendering::{TestDevice, interpret};

        let page = ResolvedPage {
            view_box: [0.0, 0.0, 10.0, 10.0],
            rotation: 0,
            resources: PDFObject::Null,
            contents: Vec::new(),
            objects: std::collections::HashMap::new(),
            annotations: Vec::new(),
        };
        let mut device = TestDevice::new(10.0, 10.0);
        interpret(&page, &mut device).expect("interpret runs with no DocStore (pure)");
    }

    #[test]
    fn interpret_matches_live_render_output() {
        // Parity: the pure interpret path and the live Page::render path must produce the
        // same drawing for the same page (this is what makes rewiring render_page_to_image
        // to resolve_page → interpret safe). Compare meaningful drawing (fills), ignoring
        // the per-stream save/restore wrapper that only Page::render emits.
        use crate::rendering::{TestDevice, interpret};

        let data = form_xobject_pdf(b"10 10 50 50 re f");

        // Live path: Page::render through a DocStore.
        let mut doc_a = PDFDocument::open(data.clone()).expect("pdf should parse and open");
        let page_a = doc_a.get_page(0).expect("page 0 should resolve");
        let mut dev_a = TestDevice::new(200.0, 300.0);
        {
            let mut store = doc_a.doc_store_mut();
            page_a.render(store, &mut dev_a).expect("live render");
        }

        // Pure path: resolve_page → interpret.
        let mut doc_b = PDFDocument::open(data).expect("pdf should parse and open");
        let page_b = doc_b.get_page(0).expect("page 0 should resolve");
        let resolved = doc_b
            .doc_store_mut()
            .resolve_page(&page_b)
            .expect("resolve_page");
        let mut dev_b = TestDevice::new(200.0, 300.0);
        interpret(&resolved, &mut dev_b).expect("pure interpret");

        let count_fills = |d: &TestDevice| {
            d.operations()
                .iter()
                .filter(|o| o.starts_with("draw_path(fill"))
                .count()
        };
        assert_eq!(
            count_fills(&dev_a),
            count_fills(&dev_b),
            "live render and pure interpret must produce the same fills"
        );
    }

    #[test]
    fn interpret_renders_form_xobject_from_snapshot() {
        // Full-purity proof: resolve_page captures a snapshot, then interpret renders
        // purely from it (no live DocStore). The Form XObject override is in the base
        // here (no delta); resolve_page walks /Resources and snapshots the form object,
        // and interpret's snapshot resolver fetches it.
        use crate::rendering::{TestDevice, interpret};

        let mut doc = PDFDocument::open(form_xobject_pdf(b"10 10 50 50 re f"))
            .expect("pdf should parse and open");
        let page = doc.get_page(0).expect("page 0 should resolve");
        let resolved = doc
            .doc_store_mut()
            .resolve_page(&page)
            .expect("resolve_page captures snapshot");

        let mut device = TestDevice::new(200.0, 300.0);
        interpret(&resolved, &mut device).expect("interpret renders from snapshot");

        let fills = device
            .operations()
            .iter()
            .filter(|op| op.starts_with("draw_path(fill"))
            .count();
        assert!(
            fills > 0,
            "interpret must render the Form XObject's rectangle from the snapshot"
        );
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
            page.render(store, &mut baseline).expect("baseline render");
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
            page.render(store, &mut after)
                .expect("post-override render");
        }

        // The delta override changed rendered output: the q (save_state) + Q (restore_state)
        // from the original content are no longer emitted — exactly two fewer device ops.
        assert_eq!(
            baseline_len,
            after.operations().len() + 2,
            "delta override of /Contents must remove the q/Q save-restore pair from render"
        );
    }

    /// A one-page PDF whose content draws a Form XObject via `Do`. The form is initially
    /// EMPTY; a delta override can replace it with one that draws, proving nested-resource
    /// (XObject) fetches through the resolver are delta-aware. Offsets computed at runtime.
    /// Build the form-XObject PDF with a given Form XObject content body (obj 4).
    fn form_xobject_pdf(form_body: &[u8]) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n");

        let o1 = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let o2 = buf.len();
        buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

        let o3 = buf.len();
        buf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] \
/Resources << /XObject << /Fm1 4 0 R >> >> /Contents 5 0 R >>\nendobj\n",
        );

        // obj 4: the Form XObject with the given content body.
        let o4 = buf.len();
        buf.extend_from_slice(format!(
            "4 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >>\nstream\n",
            form_body.len()
        ).as_bytes());
        buf.extend_from_slice(form_body);
        buf.extend_from_slice(b"\nendstream\nendobj\n");

        // obj 5: page content that draws the form ("/Fm1 Do" = 7 bytes).
        let o5 = buf.len();
        buf.extend_from_slice(b"5 0 obj\n<< /Length 7 >>\nstream\n/Fm1 Do\nendstream\nendobj\n");

        let xref_offset = buf.len();
        buf.extend_from_slice(b"xref\n0 6\n");
        buf.extend_from_slice(format!("{:010} 65535 f \r\n", 0).as_bytes());
        for o in [o1, o2, o3, o4, o5] {
            buf.extend_from_slice(format!("{:010} 00000 n \r\n", o).as_bytes());
        }
        buf.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
        buf.extend_from_slice(format!("{}\n", xref_offset).as_bytes());
        buf.extend_from_slice(b"%%EOF");
        buf
    }

    fn pdf_with_form_xobject() -> Vec<u8> {
        form_xobject_pdf(b"")
    }

    #[test]
    fn render_honors_delta_override_of_form_xobject() {
        // Nested-resource delta-awareness proof: a delta override of the Form XObject
        // (a nested resource fetched by RenderingContext via the resolver) changes the
        // rendered output. The baseline form is empty; the override draws a rectangle,
        // so the override must record more device operations.
        use crate::rendering::TestDevice;

        let mut doc =
            PDFDocument::open(pdf_with_form_xobject()).expect("pdf should parse and open");
        let page = doc.get_page(0).expect("page 0 should resolve");

        let mut baseline = TestDevice::new(200.0, 300.0);
        {
            let mut store = doc.doc_store_mut();
            page.render(store, &mut baseline).expect("baseline render");
        }
        let baseline_fills = baseline
            .operations()
            .iter()
            .filter(|op| op.starts_with("draw_path(fill"))
            .count();

        // Override obj 4 (the Form XObject) with one that draws a filled rectangle.
        let form_body = b"10 10 50 50 re f";
        let mut dict: HashMap<String, PDFObject> = HashMap::new();
        dict.insert("Type".to_string(), PDFObject::Name("XObject".into()));
        dict.insert("Subtype".to_string(), PDFObject::Name("Form".into()));
        dict.insert(
            "BBox".to_string(),
            PDFObject::Array(
                [10.0, 10.0, 50.0, 50.0]
                    .iter()
                    .map(|n| Box::new(PDFObject::Number(*n)))
                    .collect(),
            ),
        );
        dict.insert(
            "Length".to_string(),
            PDFObject::Number(form_body.len() as f64),
        );
        let form_override = PDFObject::Stream {
            dict,
            data: form_body.to_vec(),
        };
        doc.doc_store_mut()
            .delta_mut()
            .modify_object(Ref::new(4, 0), form_override);

        let mut after = TestDevice::new(200.0, 300.0);
        {
            let mut store = doc.doc_store_mut();
            page.render(store, &mut after)
                .expect("post-override render");
        }
        let after_fills = after
            .operations()
            .iter()
            .filter(|op| op.starts_with("draw_path(fill"))
            .count();

        assert_eq!(
            baseline_fills, 0,
            "baseline empty form should draw no filled paths"
        );
        assert!(
            after_fills > baseline_fills,
            "delta override of the Form XObject must add a filled path to the render"
        );
    }

    #[test]
    fn extract_text_honors_delta_override_of_contents() {
        // extract_text routes through a DocStore (delta-aware), mirroring Page::render's
        // PageResolver path. Overriding /Contents with a text-showing stream must change
        // the extracted text. (Font metrics still use the base XRef — same scope limit as
        // render; the string operand itself extracts without a font resource.)
        let mut doc = PDFDocument::open(pdf_with_contents()).expect("pdf should parse and open");
        let page = doc.get_page(0).expect("page 0 should resolve");

        // Baseline: "q Q\n" has no text → empty extraction.
        let before = {
            let mut store = doc.doc_store_mut();
            page.extract_text(store).expect("baseline extract")
        };
        assert!(before.is_empty(), "baseline 'q Q' content has no text");

        // Override /Contents (object 4) with a text-showing stream.
        let txt = b"BT (Hello) Tj ET";
        let mut dict: HashMap<String, PDFObject> = HashMap::new();
        dict.insert("Length".to_string(), PDFObject::Number(txt.len() as f64));
        doc.doc_store_mut().delta_mut().modify_object(
            Ref::new(4, 0),
            PDFObject::Stream {
                dict,
                data: txt.to_vec(),
            },
        );

        let after = {
            let mut store = doc.doc_store_mut();
            page.extract_text(store).expect("post-override extract")
        };
        assert!(
            !after.is_empty(),
            "delta override with a Tj operator must yield text items"
        );
        assert_eq!(
            after[0].text, "Hello",
            "extracted text reflects the delta override"
        );
    }
}
