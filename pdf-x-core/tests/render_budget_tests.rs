//! C.3 render-budget guards: hostile inputs must error or stop instead of
//! hanging or overflowing the stack.
//!
//! - Form XObject recursion (self- or mutual) is capped at
//!   `RenderingContext::FORM_XOBJECT_MAX_DEPTH`.
//! - `interpret_with_scale` validates scale (finite, positive) and output
//!   dimensions/area before touching a device.

#![cfg(feature = "rendering")]

use pdf_x_core::core::PDFDocument;
use pdf_x_core::rendering::{TestDevice, interpret_with_scale};

/// Assemble a minimal single-page PDF from object bodies, computing the xref
/// table from the actual byte offsets (hand-computed offsets rot fast).
fn build_pdf(objects: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
    }
    let xref_start = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        out.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_start
        )
        .as_bytes(),
    );
    out
}

/// Page with a Form XObject that invokes itself: /Fm0's content is
/// `q /Fm0 Do Q` and its /Resources point back at itself.
fn self_recursive_form_pdf() -> Vec<u8> {
    // The form invokes itself AND draws a rect, so the recorded fill count on
    // a TestDevice proves how deep the recursion actually went.
    let page_content = "q /Fm0 Do Q";
    let form_content = "q /Fm0 Do Q 0 0 20 20 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R \
         /Resources << /XObject << /Fm0 5 0 R >> >> >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            page_content.len(),
            page_content
        ),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] \
             /Resources << /XObject << /Fm0 5 0 R >> >> /Length {} >>\nstream\n{}\nendstream",
            form_content.len(),
            form_content
        ),
    ];
    build_pdf(&objects)
}

fn resolve_first_page(data: Vec<u8>) -> pdf_x_core::core::resolved_page::ResolvedPage {
    let mut doc = PDFDocument::open(data).expect("open test PDF");
    let page = doc.resolve_page(0).expect("resolve page 0");
    // Interpretation borrows nothing from the document; cloning out of the
    // Arc keeps the types simple for repeat calls below.
    (*page).clone()
}

#[test]
fn self_recursive_form_xobject_terminates() {
    let page = resolve_first_page(self_recursive_form_pdf());
    let mut device = TestDevice::new(100.0, 100.0);
    // Without the depth cap this overflows the stack (each nested Do re-enters
    // paint_form_xobject); with it, rendering completes and the result is Ok.
    interpret_with_scale(&page, &mut device, 1.0).expect("recursion-capped render completes");

    // Each nesting level draws one rect before recursing: one fill proves the
    // form was entered (the resource resolved), several prove it re-entered
    // itself before the cap stopped the descent.
    let fills = device
        .operations()
        .iter()
        .filter(|op| op.starts_with("draw_path(fill"))
        .count();
    assert!(
        (2..=16).contains(&fills),
        "expected the recursion to run several levels then stop at the cap, got {fills} fills"
    );
}

#[test]
fn scale_must_be_finite_and_positive() {
    let page = resolve_first_page(self_recursive_form_pdf());
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -2.0] {
        let mut device = TestDevice::new(100.0, 100.0);
        let err = interpret_with_scale(&page, &mut device, bad)
            .expect_err("non-finite or non-positive scale is rejected");
        assert!(
            format!("{err}").contains("Scale"),
            "unexpected error for scale {bad}: {err}"
        );
    }
}

#[test]
fn oversized_output_dimensions_are_rejected() {
    let page = resolve_first_page(self_recursive_form_pdf());
    let mut device = TestDevice::new(100.0, 100.0);
    // 100pt page at 1e30 scale — dimension/area caps must fire before any
    // pixmap-sized work happens.
    let err = interpret_with_scale(&page, &mut device, 1e30)
        .expect_err("absurd scale is rejected by the dimension budget");
    assert!(
        format!("{err}").to_lowercase().contains("too large"),
        "unexpected error: {err}"
    );
}
