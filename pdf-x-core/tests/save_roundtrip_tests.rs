//! Round-trip acceptance tests for incremental save (phase-0 spec §3.3):
//! apply command → `incremental_update` → (original + update) →
//! `qpdf --check` + reopen in ourselves → assert the change is visible, the
//! file is valid, and the original bytes were never touched.
//!
//! `qpdf --check` is the external validity oracle; tests skip it gracefully
//! when qpdf is not installed (first external-binary test in this crate).

use pdf_x_core::core::outline::parse_document_outline;
use pdf_x_core::core::{
    FindOptions, OutlineDestination, PDFDocument, PDFError, PDFObject, XRefEntry,
};

// ==========================================================================
// PDF assembly helper (fixtures with computed xref offsets)
// ==========================================================================

/// Assemble a complete PDF from object bodies, computing xref offsets.
/// Object numbers must be contiguous: `objects[0]` is object 1, etc.
fn assemble_pdf(objects: &[String], trailer_entries: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
    }

    let xref_pos = out.len();
    let size = objects.len() + 1; // + free-list head object 0
    out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} {trailer_entries} >>\nstartxref\n{xref_pos}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

/// A stream object body with a correct /Length.
fn stream_object(data: &str) -> String {
    format!("<< /Length {} >>\nstream\n{}\nendstream", data.len(), data)
}

/// Three-page PDF (flat page tree). Object map:
/// 1=Catalog, 2=Pages, 3-5=Pages, 6=Font, 7-9=Contents.
/// Same shape as document.rs's in-module `create_3page_flat_pdf`.
fn three_page_pdf() -> Vec<u8> {
    let page = |contents: &str| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 6 0 R >> >> /Contents {contents} 0 R >>"
        )
    };
    assemble_pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_string(),
            page("7"),
            page("8"),
            page("9"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            stream_object("BT /F1 12 Tf 100 700 Td (P1) Tj ET"),
            stream_object("BT /F1 12 Tf 100 700 Td (P2) Tj ET"),
            stream_object("BT /F1 12 Tf 100 700 Td (P3) Tj ET"),
        ],
        "/Root 1 0 R",
    )
}

/// One-page text PDF. Object map: 1=Catalog, 2=Pages, 3=Page, 4=Contents,
/// 5=Font. Same shape as text_edit_tests.rs's `make_minimal_text_pdf`.
fn text_pdf() -> Vec<u8> {
    assemble_pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_string(),
            stream_object("BT /F1 12 Tf 100 700 Td (Hello World) Tj ET"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ],
        "/Root 1 0 R",
    )
}

/// Two-page PDF with one outline item whose /Dest targets page 2 (object 4).
/// Object map: 1=Catalog, 2=Pages, 3=Page1, 4=Page2, 5=Outlines, 6=Item.
fn outlined_two_page_pdf() -> Vec<u8> {
    assemble_pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
            "<< /Type /Outlines /First 6 0 R /Last 6 0 R /Count 1 >>".to_string(),
            "<< /Title (Target) /Dest [4 0 R /Fit] /Parent 5 0 R >>".to_string(),
        ],
        "/Root 1 0 R",
    )
}

/// Four-page PDF with a two-level page tree (one intermediate /Pages node per
/// branch). Object map: 1=Catalog, 2=RootPages, 3=Inter1 (pages 0,1),
/// 4=Inter2 (pages 2,3), 5-8=Page dicts.
fn nested_tree_pdf() -> Vec<u8> {
    let page =
        |parent: u32| format!("<< /Type /Page /Parent {parent} 0 R /MediaBox [0 0 612 792] >>");
    assemble_pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 4 >>".to_string(),
            "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R] /Count 2 >>".to_string(),
            "<< /Type /Pages /Parent 2 0 R /Kids [7 0 R 8 0 R] /Count 2 >>".to_string(),
            page(3),
            page(3),
            page(4),
            page(4),
        ],
        "/Root 1 0 R",
    )
}

/// One-page PDF with a pre-existing (base-object) highlight annotation and its
/// indirect `/AP /N` stream. Object map: 1=Catalog, 2=Pages, 3=Page,
/// 4=Contents, 5=Font, 6=Annot, 7=AP stream. The annotation carries the
/// normalized QuadPoints order (TL, TR, BL, BR).
fn annotated_pdf() -> Vec<u8> {
    assemble_pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Annots [6 0 R] /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_string(),
            stream_object("BT /F1 12 Tf 100 700 Td (Annotated) Tj ET"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            "<< /Type /Annot /Subtype /Highlight /Rect [90 690 310 730] \
             /QuadPoints [100 720 300 720 100 700 300 700] /C [1 1 0] /F 4 \
             /AP << /N 7 0 R >> >>"
                .to_string(),
            stream_object("q 1 1 0 rg 100 700 m 300 700 l 300 720 l 100 720 l f Q"),
        ],
        "/Root 1 0 R",
    )
}

// ==========================================================================
// Save / validate helpers
// ==========================================================================

/// Append the delta's incremental update to `original`.
fn save(doc: &PDFDocument, original: &[u8]) -> Vec<u8> {
    let update = doc
        .incremental_update(original)
        .expect("incremental update should serialize");
    let mut saved = original.to_vec();
    saved.extend_from_slice(&update);
    saved
}

/// The original file must survive byte-identical as the saved prefix.
fn assert_prefix_unchanged(original: &[u8], saved: &[u8]) {
    assert!(
        saved.len() > original.len(),
        "saved file must be a strict extension of the original"
    );
    assert_eq!(
        &saved[..original.len()],
        original,
        "incremental update must not modify original bytes"
    );
}

fn qpdf_available() -> bool {
    std::process::Command::new("qpdf")
        .arg("--version")
        .output()
        .is_ok()
}

static QPDF_TEMP_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Run `qpdf --check` over `bytes`, panicking on structural errors.
/// qpdf warnings (exit 3) are logged but not fatal; a missing qpdf skips the
/// check entirely (the in-process reopen assertions still run).
fn assert_qpdf_clean(bytes: &[u8], context: &str) {
    if !qpdf_available() {
        eprintln!("skipping qpdf --check ({context}): qpdf not installed");
        return;
    }
    let mut path = std::env::temp_dir();
    let serial = QPDF_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.push(format!(
        "pdfx_roundtrip_{}_{}.pdf",
        std::process::id(),
        serial
    ));
    std::fs::write(&path, bytes).expect("temp file write for qpdf check");

    let output = std::process::Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .output();
    let _ = std::fs::remove_file(&path);

    let output = match output {
        Ok(out) => out,
        Err(_) => {
            eprintln!("skipping qpdf --check ({context}): qpdf vanished mid-run");
            return;
        }
    };
    match output.status.code() {
        Some(0) => {}
        Some(3) => eprintln!(
            "qpdf --check warnings for {context}: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
        code => panic!(
            "qpdf --check failed for {context} (exit {code:?}): {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

fn reopen(bytes: Vec<u8>) -> PDFDocument {
    PDFDocument::open(bytes).expect("saved document should reopen")
}

// ==========================================================================
// Command round-trips
// ==========================================================================

#[test]
fn rotate_page_save_reopens_and_qpdf_clean() {
    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    doc.rotate_page(0, 90).expect("rotate should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "rotate_page");

    let mut reopened = reopen(saved);
    let page = reopened.get_page(0).expect("page 0 after reopen");
    assert_eq!(
        page.get("Rotate"),
        Some(&PDFObject::Number(90.0)),
        "rotation must be visible after save + reopen"
    );
}

#[test]
fn set_info_save_reopens_and_qpdf_clean() {
    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    doc.set_info(vec![(
        "Title".to_string(),
        Some(PDFObject::String(b"Round Trip".to_vec())),
    )])
    .expect("set_info should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "set_info");

    let mut reopened = reopen(saved);
    let info = reopened
        .document_info()
        .expect("document_info should resolve")
        .expect("info dict must exist after reopen");
    match &*info {
        PDFObject::Dictionary(dict) => assert_eq!(
            dict.get("Title"),
            Some(&PDFObject::String(b"Round Trip".to_vec())),
            "title must round-trip through save + reopen"
        ),
        other => panic!("expected info dictionary, got {other:?}"),
    }
}

#[test]
fn add_highlight_save_reopens_and_qpdf_clean() {
    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    doc.add_highlight(0, [10.0, 20.0, 110.0, 40.0], [1.0, 0.9, 0.1])
        .expect("highlight should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    // qpdf validates the synthesized AP form XObject and its /Length.
    assert_qpdf_clean(&saved, "add_highlight");

    let mut reopened = reopen(saved);
    let page = reopened.get_page(0).expect("page 0 after reopen");
    let annots = page
        .get("Annots")
        .and_then(|a| a.as_array())
        .expect("annotation must persist after save + reopen");
    assert_eq!(annots.len(), 1, "exactly one highlight expected");
}

#[test]
fn replace_text_save_reopens_and_qpdf_clean() {
    let original = text_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    doc.replace_text(0, "World", "PDF", &FindOptions::default())
        .expect("replace_text should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "replace_text");

    let mut reopened = reopen(saved);
    let text = reopened
        .extract_text_from_page_as_string(0)
        .expect("text extraction after reopen");
    assert!(
        text.contains("Hello PDF"),
        "replaced text must persist: {text}"
    );
    assert!(!text.contains("World"), "old text must be gone: {text}");
}

// ==========================================================================
// Deletion persistence
// ==========================================================================

#[test]
fn delete_page_save_persists_as_free_entry() {
    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    // Page index 1 is object 4 in the fixture.
    doc.delete_page(1).expect("delete_page should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "delete_page");

    let mut reopened = reopen(saved);
    assert_eq!(
        reopened.page_count().expect("page count"),
        2,
        "deleted page must not come back"
    );

    // The freed page dict persists as a free xref entry — direct
    // no-resurrection assertion against the reopened file.
    let entry = reopened
        .doc_store_mut()
        .xref()
        .get_entry(4)
        .expect("object 4 must still have an xref entry");
    assert!(
        matches!(entry, XRefEntry::Free { .. }),
        "object 4 must be persisted as free, got {entry:?}"
    );

    // Fetching it fails with the deletion error — even though the object's
    // original bytes are still in the file prefix.
    let err = reopened
        .doc_store_mut()
        .xref_mut()
        .fetch(4, 0)
        .expect_err("freed object must not resolve");
    assert!(
        matches!(err, PDFError::ObjectDeleted { num: 4, .. }),
        "unexpected error: {err}"
    );

    // The failed fetch must not have triggered recovery-by-scan (which would
    // find the stale bytes and resurrect the object): entry stays free, and
    // a live object still fetches.
    assert!(
        reopened
            .doc_store_mut()
            .xref()
            .get_entry(4)
            .unwrap()
            .is_free(),
        "free entry was rewritten — recovery resurrected a deleted object"
    );
    let live = reopened
        .doc_store_mut()
        .xref_mut()
        .fetch(3, 0)
        .expect("surviving page object must still fetch");
    assert!(matches!(&*live, PDFObject::Dictionary(_)));
}

#[test]
fn delete_page_multi_generation_save() {
    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    // Generation 1: delete page 2.
    doc.delete_page(1).expect("delete should apply");
    let gen1 = save(&doc, &original);

    // Generation 2: rotate a page of the already-appended file.
    let mut doc2 = reopen(gen1.clone());
    doc2.rotate_page(0, 90).expect("rotate should apply");
    let gen2 = save(&doc2, &gen1);
    assert_prefix_unchanged(&gen1, &gen2);
    assert_qpdf_clean(&gen2, "multi-generation");

    // Both effects survive: the deletion (newest-wins free entry through the
    // /Prev chain) and the rotation.
    let mut reopened = reopen(gen2);
    assert_eq!(reopened.page_count().expect("page count"), 2);
    let page = reopened.get_page(0).expect("page 0");
    assert_eq!(page.get("Rotate"), Some(&PDFObject::Number(90.0)));
    assert!(matches!(
        reopened.doc_store_mut().xref().get_entry(4),
        Some(XRefEntry::Free { .. })
    ));
}

#[test]
fn outline_survives_dangling_dest_after_delete() {
    let original = outlined_two_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    // Sanity before the delete: the item resolves to page index 1.
    let items = parse_document_outline(&mut doc)
        .expect("outline should parse")
        .expect("fixture has an outline");
    assert_eq!(items.len(), 1);
    assert!(
        matches!(
            &items[0].dest,
            Some(OutlineDestination::Explicit { page_index: 1, .. })
        ),
        "dest must resolve before deletion, got {:?}",
        items[0].dest
    );

    // Delete the dest's target page: the item must degrade, not vanish.
    doc.delete_page(1).expect("delete_page should apply");
    let items = parse_document_outline(&mut doc)
        .expect("outline must still parse after deleting its dest target")
        .expect("outline must not be lost");
    assert_eq!(items.len(), 1, "the item itself must survive");
    assert_eq!(items[0].title, "Target");
    assert!(
        items[0].dest.is_none(),
        "dangling dest must degrade to None, got {:?}",
        items[0].dest
    );

    // Same after persisting + reopening the saved file.
    let saved = save(&doc, &original);
    assert_qpdf_clean(&saved, "outline dangling dest");
    let mut reopened = reopen(saved);
    let items = parse_document_outline(&mut reopened)
        .expect("outline must parse in the reopened file")
        .expect("outline must persist");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Target");
    assert!(items[0].dest.is_none());
}

// ==========================================================================
// Multi-edit sessions
// ==========================================================================

#[test]
fn save_twice_same_session() {
    let original = three_page_pdf();

    // First session: one highlight, saved.
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");
    doc.add_highlight(0, [10.0, 20.0, 110.0, 40.0], [1.0, 0.9, 0.1])
        .expect("first highlight should apply");
    let gen1 = save(&doc, &original);

    // Second session on the appended file: another highlight, saved again.
    let mut doc2 = reopen(gen1.clone());
    doc2.add_highlight(0, [50.0, 60.0, 150.0, 80.0], [0.0, 1.0, 0.0])
        .expect("second highlight should apply");
    let gen2 = save(&doc2, &gen1);
    assert_prefix_unchanged(&gen1, &gen2);
    assert_qpdf_clean(&gen2, "save twice");

    let mut reopened = reopen(gen2);
    let annots = reopened
        .get_page(0)
        .expect("page 0 after reopen")
        .get("Annots")
        .and_then(|a| a.as_array())
        .expect("annots must persist")
        .len();
    assert_eq!(annots, 2, "both highlights must survive two saves");
}

// ==========================================================================
// Page reorder
// ==========================================================================

/// Current page order as object refs — the order oracle.
fn page_refs(doc: &mut PDFDocument) -> Vec<(u32, u32)> {
    let n = doc.page_count().expect("page count") as usize;
    (0..n)
        .map(|i| {
            doc.get_page(i)
                .unwrap_or_else(|e| panic!("page {i} should resolve: {e}"))
                .reference()
                .unwrap_or_else(|| panic!("page {i} has no object reference"))
        })
        .collect()
}

#[test]
fn move_page_same_parent_roundtrip() {
    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    // Move last page to the front: [P1 P2 P3] -> [P3 P1 P2].
    doc.move_page(2, 0).expect("move should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "move_page same parent");

    let mut reopened = reopen(saved);
    assert_eq!(
        reopened.page_count().expect("page count"),
        3,
        "page count must survive a reorder"
    );
    assert_eq!(
        page_refs(&mut reopened),
        vec![(5, 0), (3, 0), (4, 0)],
        "moved order must persist after save + reopen"
    );
}

#[test]
fn move_page_cross_parent_roundtrip() {
    let original = nested_tree_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    // Move page 0 (obj 5, branch 3) to the end (after obj 8 in branch 4):
    // [5 6 | 7 8] -> [6 | 7 8 5]. The shared root /Count must stay 4.
    doc.move_page(0, 3).expect("cross-parent move should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "move_page cross parent");

    let mut reopened = reopen(saved);
    assert_eq!(reopened.page_count().expect("page count"), 4);
    assert_eq!(
        page_refs(&mut reopened),
        vec![(6, 0), (7, 0), (8, 0), (5, 0)],
        "cross-parent move must persist with consistent /Count"
    );
}

#[test]
fn move_page_to_end_roundtrip() {
    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    // Adjacent move is a no-op that must not dirty the delta…
    doc.move_page(0, 1).expect("no-op move");
    assert_eq!(doc.editor_state(), (false, false, false));

    // …then move the first page to the end (destination beyond the last page
    // clamps): [P1 P2 P3] -> [P2 P3 P1].
    doc.move_page(0, 99)
        .expect("clamped move to end should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "move_page to end");

    let mut reopened = reopen(saved);
    assert_eq!(
        page_refs(&mut reopened),
        vec![(4, 0), (5, 0), (3, 0)],
        "only the real move must persist"
    );
}

#[test]
fn move_page_undo_restores_order() {
    let original = nested_tree_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    // Move page 3 (obj 8, branch 4) to the front: [5 6 | 7 8] -> [8 | 5 6 7].
    doc.move_page(3, 0).expect("move should apply");
    doc.undo().expect("undo should apply");
    // Undo restores the tree content exactly. `dirty` stays true because the
    // delta keeps the restored (base-identical) entries — pre-existing
    // behavior shared by every structural command's undo, not move-specific.
    assert_eq!(doc.editor_state(), (true, false, true));
    assert_eq!(page_refs(&mut doc), vec![(5, 0), (6, 0), (7, 0), (8, 0)]);

    // Redo, then persist: the saved file shows the moved order.
    doc.redo().expect("redo should apply");
    let saved = save(&doc, &original);
    assert_qpdf_clean(&saved, "move_page undo/redo");

    let mut reopened = reopen(saved);
    assert_eq!(
        page_refs(&mut reopened),
        vec![(8, 0), (5, 0), (6, 0), (7, 0)]
    );
}

// ==========================================================================
// Annotation CRUD persistence
// ==========================================================================

/// One horizontal band under a text line, in the normalized quad order.
fn line_quad(y_top: f64) -> Vec<pdf_x_core::core::delta::QuadPoints> {
    let (top, bottom) = (y_top, y_top - 20.0);
    vec![[100.0, top, 300.0, top, 100.0, bottom, 300.0, bottom]]
}

#[test]
fn add_underline_save_reopens_and_qpdf_clean() {
    use pdf_x_core::core::annotation::AnnotationType;
    use pdf_x_core::core::delta::MarkupKind;

    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    doc.add_markup(0, MarkupKind::Underline, line_quad(720.0), [0.0, 0.0, 0.0])
        .expect("add_markup should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    // qpdf validates the synthesized annotation + AP form XObject.
    assert_qpdf_clean(&saved, "add_underline");

    let mut reopened = reopen(saved);
    let annots = reopened
        .annotations_with_refs(0)
        .expect("annotations after reopen");
    assert_eq!(annots.len(), 1, "exactly one underline expected");
    let (annot_ref, annot) = &annots[0];
    assert!(
        annot_ref.is_some(),
        "saved annotation must be an indirect object"
    );
    assert_eq!(annot.annotation_type, AnnotationType::Underline);
    assert_eq!(annot.color, Some(vec![0.0, 0.0, 0.0]), "/C must round-trip");
}

#[test]
fn add_squiggly_and_strikeout_save_reopens() {
    use pdf_x_core::core::annotation::AnnotationType;
    use pdf_x_core::core::delta::MarkupKind;

    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    doc.add_markup(0, MarkupKind::Squiggly, line_quad(720.0), [0.0, 0.5, 0.0])
        .expect("squiggly should apply");
    doc.add_markup(0, MarkupKind::StrikeOut, line_quad(600.0), [1.0, 0.0, 0.0])
        .expect("strikeout should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "add_squiggly_strikeout");

    let mut reopened = reopen(saved);
    let annots = reopened
        .annotations_with_refs(0)
        .expect("annotations after reopen");
    assert_eq!(annots.len(), 2, "both markups must persist");
    assert!(
        annots
            .iter()
            .any(|(_, a)| a.annotation_type == AnnotationType::Squiggly),
        "squiggly must persist"
    );
    assert!(
        annots
            .iter()
            .any(|(_, a)| a.annotation_type == AnnotationType::StrikeOut),
        "strikeout must persist"
    );
}

#[test]
fn delete_annotation_save_removes_and_undo_restores() {
    use pdf_x_core::core::annotation::AnnotationType;

    // Base-object path: delete the fixture's highlight (object 6), save, and
    // the annotation is gone on reopen.
    let original = annotated_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");
    doc.delete_annotation(0, 6).expect("delete should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "delete_annotation");

    let mut reopened = reopen(saved);
    assert!(
        reopened
            .annotations_with_refs(0)
            .expect("annotations after delete")
            .is_empty(),
        "deleted annotation must not come back"
    );

    // Undo path: delete then undo, save, and the highlight (+ its AP) is back.
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");
    doc.delete_annotation(0, 6).expect("delete should apply");
    doc.undo().expect("undo should apply");

    let saved = save(&doc, &original);
    assert_qpdf_clean(&saved, "delete_annotation undo");

    let mut reopened = reopen(saved);
    let annots = reopened
        .annotations_with_refs(0)
        .expect("annotations after undo");
    assert_eq!(annots.len(), 1, "undo must restore the annotation");
    assert_eq!(annots[0].1.annotation_type, AnnotationType::Highlight);
}

#[test]
fn add_then_delete_annotation_save_leaves_no_annotation() {
    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    use pdf_x_core::core::delta::MarkupKind;
    doc.add_markup(0, MarkupKind::Underline, line_quad(720.0), [0.0, 0.0, 0.0])
        .expect("add_markup should apply");
    let object_num = doc
        .annotations_with_refs(0)
        .expect("annotations")
        .iter()
        .find_map(|(r, _)| r.map(|r| r.num))
        .expect("added annotation must be an indirect object");

    doc.delete_annotation(0, object_num)
        .expect("delete should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "add_then_delete_annotation");

    let mut reopened = reopen(saved);
    assert!(
        reopened
            .annotations_with_refs(0)
            .expect("annotations after reopen")
            .is_empty(),
        "add-then-delete must leave no annotation"
    );
}

#[test]
fn edit_annotation_color_save_reopens() {
    use pdf_x_core::core::delta::MarkupKind;

    let original = three_page_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");

    doc.add_markup(0, MarkupKind::Highlight, line_quad(720.0), [1.0, 1.0, 0.0])
        .expect("add_markup should apply");
    let object_num = doc
        .annotations_with_refs(0)
        .expect("annotations")
        .iter()
        .find_map(|(r, _)| r.map(|r| r.num))
        .expect("added annotation must be an indirect object");

    doc.edit_annotation(0, object_num, Some([1.0, 0.0, 0.0]), None)
        .expect("edit should apply");

    let saved = save(&doc, &original);
    assert_prefix_unchanged(&original, &saved);
    assert_qpdf_clean(&saved, "edit_annotation_color");

    let mut reopened = reopen(saved);
    let annots = reopened
        .annotations_with_refs(0)
        .expect("annotations after reopen");
    assert_eq!(annots.len(), 1, "exactly one highlight expected");
    assert_eq!(
        annots[0].1.color,
        Some(vec![1.0, 0.0, 0.0]),
        "recolored /C must round-trip"
    );
}
