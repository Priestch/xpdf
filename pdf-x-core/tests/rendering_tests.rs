//! Rendering regression tests for PDF-X.
//!
//! These tests verify the rendering pipeline produces consistent output
//! for various PDF content stream operations.

use pdf_x_core::rendering::graphics_state::{Color, FillRule, StrokeProps};
use pdf_x_core::rendering::{Device, Paint, PathDrawMode, TestDevice};

#[cfg(feature = "rendering")]
use pdf_x_device_tinyskia::{FontCache, SkiaDevice};

#[cfg(feature = "rendering")]
use tiny_skia::Pixmap;

// ============================================================================
// Basic Drawing Tests
// ============================================================================

#[test]
fn test_draw_rectangle() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);

    let paint = Paint::Solid(Color::rgb(255, 0, 0));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "rect(10,10,80,80)");
    assert!(ops[2].contains("draw_path(fill"));
}

#[test]
fn test_draw_stroked_rectangle() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);

    let paint = Paint::Solid(Color::black());
    device
        .draw_path(PathDrawMode::Stroke, &paint, &StrokeProps::default())
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "rect(10,10,80,80)");
    assert_eq!(ops[2], "draw_path(stroke)");
}

#[test]
fn test_draw_filled_and_stroked_rectangle() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);

    let paint = Paint::Solid(Color::rgb(0, 0, 255));
    device
        .draw_path(
            PathDrawMode::FillStroke(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "rect(10,10,80,80)");
    assert!(ops[2].contains("draw_path(fill_stroke"));
}

// ============================================================================
// Path Tests
// ============================================================================

#[test]
fn test_draw_path_with_lines() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.move_to(10.0, 10.0);
    device.line_to(50.0, 50.0);
    device.line_to(90.0, 10.0);
    device.close_path();

    let paint = Paint::Solid(Color::black());
    device
        .draw_path(PathDrawMode::Stroke, &paint, &StrokeProps::default())
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "move_to(10,10)");
    assert_eq!(ops[2], "line_to(50,50)");
    assert_eq!(ops[3], "line_to(90,10)");
    assert_eq!(ops[4], "close_path");
    assert_eq!(ops[5], "draw_path(stroke)");
}

#[test]
fn test_draw_path_with_curves() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.move_to(10.0, 10.0);
    device.curve_to(20.0, 20.0, 30.0, 20.0, 40.0, 10.0);

    let paint = Paint::Solid(Color::black());
    device
        .draw_path(PathDrawMode::Stroke, &paint, &StrokeProps::default())
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "move_to(10,10)");
    assert_eq!(ops[2], "curve_to(20,20,30,20,40,10)");
    assert_eq!(ops[3], "draw_path(stroke)");
}

// ============================================================================
// Clipping Tests
// ============================================================================

#[test]
fn test_clip_path() {
    let mut device = TestDevice::new(100.0, 100.0);

    // Define clipping region
    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);
    device.clip_path(FillRule::NonZero).unwrap();

    // Draw something (should be clipped)
    device.begin_path();
    device.rect(0.0, 0.0, 100.0, 100.0);

    let paint = Paint::Solid(Color::rgb(255, 0, 0));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "rect(10,10,80,80)");
    assert_eq!(ops[2], "clip_path(NonZero)");
    assert_eq!(ops[3], "begin_path");
    assert_eq!(ops[4], "rect(0,0,100,100)");
}

#[test]
fn test_clip_path_even_odd() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);
    device.clip_path(FillRule::EvenOdd).unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "rect(10,10,80,80)");
    assert_eq!(ops[2], "clip_path(EvenOdd)");
}

// ============================================================================
// Transformation Tests
// ============================================================================

#[test]
fn test_scale_transform() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.save_state();
    device.concat_matrix(&[2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);

    device.begin_path();
    device.rect(10.0, 10.0, 20.0, 20.0);

    let paint = Paint::Solid(Color::black());
    device
        .draw_path(PathDrawMode::Stroke, &paint, &StrokeProps::default())
        .unwrap();

    device.restore_state();

    let ops = device.operations();
    assert_eq!(ops[0], "save_state");
    assert_eq!(ops[1], "concat_matrix([2.0, 0.0, 0.0, 2.0, 0.0, 0.0])");
    assert_eq!(ops[2], "begin_path");
    assert_eq!(ops[3], "rect(10,10,20,20)");
    assert_eq!(ops[4], "draw_path(stroke)");
    assert_eq!(ops[5], "restore_state");
}

#[test]
fn test_nested_transforms() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.save_state();
    device.concat_matrix(&[2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);

    device.save_state();
    device.concat_matrix(&[1.0, 0.0, 0.0, 1.0, 10.0, 20.0]);

    device.begin_path();
    device.rect(5.0, 5.0, 10.0, 10.0);

    let paint = Paint::Solid(Color::black());
    device
        .draw_path(PathDrawMode::Stroke, &paint, &StrokeProps::default())
        .unwrap();

    device.restore_state();
    device.restore_state();

    let ops = device.operations();
    assert_eq!(ops[0], "save_state");
    assert_eq!(ops[1], "concat_matrix([2.0, 0.0, 0.0, 2.0, 0.0, 0.0])");
    assert_eq!(ops[2], "save_state");
    assert_eq!(ops[3], "concat_matrix([1.0, 0.0, 0.0, 1.0, 10.0, 20.0])");
    assert_eq!(ops[4], "begin_path");
    assert_eq!(ops[5], "rect(5,5,10,10)");
}

// ============================================================================
// Text Rendering Tests
// ============================================================================

#[test]
fn test_draw_text() {
    let mut device = TestDevice::new(612.0, 792.0);

    let paint = Paint::Solid(Color::black());
    device
        .draw_text(
            b"Hello, World!",
            "Helvetica",
            12.0,
            &paint,
            &[1.0, 0.0, 0.0, 1.0, 100.0, 700.0],
            100.0,
            0.0,
            0.0,
            0.0,
        )
        .unwrap();

    let ops = device.operations();
    assert!(ops[0].contains("draw_text(Helvetica, 12"));
}

#[test]
fn test_draw_multiple_text_objects() {
    let mut device = TestDevice::new(612.0, 792.0);

    let paint = Paint::Solid(Color::black());
    device
        .draw_text(
            b"First line",
            "Helvetica",
            12.0,
            &paint,
            &[1.0, 0.0, 0.0, 1.0, 100.0, 700.0],
            100.0,
            0.0,
            0.0,
            0.0,
        )
        .unwrap();
    device
        .draw_text(
            b"Second line",
            "Times-Roman",
            14.0,
            &paint,
            &[1.0, 0.0, 0.0, 1.0, 100.0, 680.0],
            100.0,
            0.0,
            0.0,
            0.0,
        )
        .unwrap();

    let ops = device.operations();
    assert!(ops[0].contains("draw_text(Helvetica, 12"));
    assert!(ops[1].contains("draw_text(Times-Roman, 14"));
}

// ============================================================================
// Color Tests
// ============================================================================

#[test]
fn test_draw_with_rgb_color() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);

    // Red rectangle
    let paint = Paint::Solid(Color::rgb(255, 0, 0));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "rect(10,10,80,80)");
    assert!(ops[2].contains("draw_path(fill"));
}

#[test]
fn test_draw_with_gray_color() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);

    // 50% gray
    let paint = Paint::Solid(Color::Gray(0.5));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops.len(), 3);
    assert!(ops[2].contains("fill"));
}

// ============================================================================
// Stroke Properties Tests
// ============================================================================

#[test]
fn test_draw_with_line_width() {
    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);

    let paint = Paint::Solid(Color::black());
    let stroke_props = StrokeProps {
        line_width: 2.0,
        ..Default::default()
    };
    device
        .draw_path(PathDrawMode::Stroke, &paint, &stroke_props)
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "rect(10,10,80,80)");
    assert_eq!(ops[2], "draw_path(stroke)");
}

#[test]
fn test_draw_with_line_cap() {
    use pdf_x_core::rendering::graphics_state::LineCap;

    let mut device = TestDevice::new(100.0, 100.0);

    device.begin_path();
    device.line_to(50.0, 50.0);

    let paint = Paint::Solid(Color::black());
    let stroke_props = StrokeProps {
        line_cap: LineCap::Round,
        ..Default::default()
    };
    device
        .draw_path(PathDrawMode::Stroke, &paint, &stroke_props)
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "line_to(50,50)");
}

// ============================================================================
// Complex Drawing Tests
// ============================================================================

#[test]
fn test_draw_complex_shape() {
    let mut device = TestDevice::new(100.0, 100.0);

    // Draw a star-like shape
    device.begin_path();
    device.move_to(50.0, 10.0);
    device.line_to(61.0, 35.0);
    device.line_to(88.0, 35.0);
    device.line_to(66.0, 50.0);
    device.line_to(75.0, 75.0);
    device.line_to(50.0, 60.0);
    device.line_to(25.0, 75.0);
    device.line_to(34.0, 50.0);
    device.line_to(12.0, 35.0);
    device.line_to(39.0, 35.0);
    device.close_path();

    let paint = Paint::Solid(Color::rgb(255, 215, 0));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    let ops = device.operations();
    assert_eq!(ops[0], "begin_path");
    assert_eq!(ops[1], "move_to(50,10)");
    assert_eq!(ops.len(), 13); // 12 path ops + 1 draw_path
}

#[test]
fn test_draw_clipped_content() {
    let mut device = TestDevice::new(200.0, 200.0);

    // Create a circular clipping region using a path approximation
    device.save_state();
    device.begin_path();
    device.move_to(100.0, 50.0);
    // Approximate circle with lines
    for i in 0..8 {
        let angle = (i as f64 + 1.0) * std::f64::consts::PI / 4.0;
        let x = 100.0 + 50.0 * angle.cos();
        let y = 100.0 + 50.0 * angle.sin();
        device.line_to(x, y);
    }
    device.close_path();
    device.clip_path(FillRule::NonZero).unwrap();

    // Draw content that will be clipped
    device.begin_path();
    device.rect(0.0, 0.0, 200.0, 200.0);

    let paint = Paint::Solid(Color::rgb(255, 0, 0));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    device.restore_state();

    let ops = device.operations();
    assert_eq!(ops[0], "save_state");
    assert_eq!(ops[1], "begin_path");
    assert_eq!(ops[ops.len() - 1], "restore_state");
}

// ============================================================================
// Image Rendering Tests
// ============================================================================

#[test]
fn test_draw_image() {
    let mut device = TestDevice::new(100.0, 100.0);

    // Create simple image data (10x10 red square)
    let pixel_data = vec![255u8; 10 * 10 * 4]; // RGBA
    let image = pdf_x_core::rendering::ImageData {
        width: 10,
        height: 10,
        data: pixel_data,
        has_alpha: true,
        bits_per_component: 8,
        original_bits_per_component: 8,
        is_mask: false,
        mask_paint_one: true,
        mask_color: [0, 0, 0],
        interpolate: false,
    };

    let transform = [10.0, 0.0, 0.0, 10.0, 0.0, 0.0];
    device.draw_image(image, &transform).unwrap();

    let ops = device.operations();
    assert_eq!(
        ops[0],
        "draw_image(10x10, [10.0, 0.0, 0.0, 10.0, 0.0, 0.0])"
    );
}

// ============================================================================
// Skia Device Integration Tests (only when rendering feature is enabled)
// ============================================================================

#[cfg(feature = "rendering")]
#[test]
fn test_skia_draw_rectangle() {
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    let mut font_cache = FontCache::new();
    let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache);

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);

    let paint = Paint::Solid(Color::rgb(255, 0, 0));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    // If we got here without panicking, the test passes
    // In real tests, we would compare the pixmap data to expected values
}

#[cfg(feature = "rendering")]
#[test]
fn test_skia_draw_path() {
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    let mut font_cache = FontCache::new();
    let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache);

    device.begin_path();
    device.move_to(10.0, 10.0);
    device.line_to(90.0, 90.0);
    device.line_to(10.0, 90.0);
    device.close_path();

    let paint = Paint::Solid(Color::rgb(0, 0, 255));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();
}

#[cfg(feature = "rendering")]
#[test]
fn test_skia_transformations() {
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    let mut font_cache = FontCache::new();
    let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache);

    device.save_state();
    device.concat_matrix(&[0.5, 0.0, 0.0, 0.5, 25.0, 25.0]);

    device.begin_path();
    device.rect(0.0, 0.0, 100.0, 100.0);

    let paint = Paint::Solid(Color::rgb(0, 255, 0));
    device
        .draw_path(
            PathDrawMode::Fill(Default::default()),
            &paint,
            &StrokeProps::default(),
        )
        .unwrap();

    device.restore_state();
}

// ============================================================================
// Page Bounds Tests
// ============================================================================

#[test]
fn test_page_bounds() {
    let device = TestDevice::new(612.0, 792.0);
    let (width, height) = device.page_bounds();
    assert_eq!(width, 612.0);
    assert_eq!(height, 792.0);
}

#[test]
fn test_a4_page_bounds() {
    let device = TestDevice::new(595.0, 842.0);
    let (width, height) = device.page_bounds();
    assert_eq!(width, 595.0);
    assert_eq!(height, 842.0);
}

// ============================================================================
// Parallel Rendering Tests
// ============================================================================

#[cfg(feature = "rendering")]
mod parallel {
    use pdf_x_core::core::PDFDocument;
    use pdf_x_device_tinyskia::RenderToImage;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest_dir
            .parent()
            .expect("pdf-x-core has a parent workspace dir")
            .join("tests")
            .join("fixtures")
    }

    /// Parallel render of the same page twice must produce byte-identical output
    /// to the sequential render — verifies `Arc<ResolvedPage>` + `interpret` are
    /// safe to run on multiple threads with no shared mutable state.
    #[test]
    fn parallel_matches_sequential() {
        let path = fixtures_dir().join("pdfs").join("tracemonkey.pdf");
        let bytes = std::fs::read(&path).expect("fixture should exist");
        let mut doc = PDFDocument::open(bytes).expect("pdf should open");

        // Sequential render of page 0 (twice, to warm the LRU cache so both paths
        // hit the same Arc<ResolvedPage> snapshot).
        let seq0 = doc
            .render_page_to_image(0, Some(1.0))
            .expect("seq render p0");
        let seq1 = doc
            .render_page_to_image(0, Some(1.0))
            .expect("seq render p0 again");

        // Parallel render of page 0 twice.
        let par = doc.render_pages_to_images(&[0, 0], Some(1.0));
        assert_eq!(par.len(), 2, "should return one result per input index");
        let par0 = par[0].as_ref().expect("parallel render p0 (first)");
        let par1 = par[1].as_ref().expect("parallel render p0 (second)");

        // Dimensions match.
        assert_eq!(par0.0, seq0.0, "width: parallel vs sequential");
        assert_eq!(par0.1, seq0.1, "height: parallel vs sequential");

        // Byte-identical pixels (the LRU cache returns the same Arc<ResolvedPage> to
        // both threads, so interpret runs over the exact same snapshot).
        assert_eq!(
            par0.2, seq0.2,
            "pixels: parallel render must match sequential"
        );
        assert_eq!(
            par1.2, seq1.2,
            "pixels: second parallel slot must match too"
        );
    }
}

// ============================================================================
// Annotation appearance rendering
// ============================================================================

/// A generated markup `/AP /N` (AddMarkupAnnotationCommand) must paint through
/// the standard pipeline (`paint_annotation_appearances` inside `interpret`) —
/// and still paint after an incremental save + reopen, proving the synthesized
/// appearance stream survives persistence.
#[cfg(feature = "rendering")]
#[test]
fn generated_underline_appearance_paints_after_save_and_reopen() {
    use pdf_x_core::core::PDFDocument;
    use pdf_x_core::core::delta::{MarkupKind, QuadPoints};
    use pdf_x_device_tinyskia::RenderToImage;

    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("pdf-x-core has a parent workspace dir")
        .join("tests")
        .join("fixtures")
        .join("pdfs")
        .join("simpletype3font.pdf");
    let original = std::fs::read(&path).expect("fixture should exist");

    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");
    let baseline = doc
        .render_page_to_image(0, Some(1.0))
        .expect("baseline render");

    // One underline quad across a text line, in user-space coordinates. The
    // pdf.js Underline algorithm strokes y = bottom + 1.3 (here 701.3) with
    // width 0.571.
    let quad: QuadPoints = [100.0, 720.0, 300.0, 720.0, 100.0, 700.0, 300.0, 700.0];
    doc.add_markup(0, MarkupKind::Underline, vec![quad], [0.0, 0.0, 0.0])
        .expect("add_markup should apply");

    // Save + reopen: the appearance must survive persistence.
    let mut saved = original.clone();
    saved.extend_from_slice(&doc.incremental_update(&original).expect("update"));
    let mut reopened = PDFDocument::open(saved).expect("saved file should reopen");
    let (width, height, pixels) = reopened
        .render_page_to_image(0, Some(1.0))
        .expect("render after reopen");

    assert_eq!(
        (width, height),
        (baseline.0, baseline.1),
        "page size must not change"
    );

    // The only visible change must be the underline stroke itself. Device y =
    // 792 - y_pdf puts the stroke at rows ~90-91 at scale 1; allow an
    // antialias margin. x spans the quad's [100, 300].
    let mut changed = Vec::new();
    for i in 0..(width as usize * height as usize) {
        let a = &pixels[i * 4..i * 4 + 4];
        let b = &baseline.2[i * 4..i * 4 + 4];
        if a != b {
            changed.push((i as u32 % width, i as u32 / width));
        }
    }
    assert!(!changed.is_empty(), "the underline must be visible");
    for (x, y) in changed {
        assert!(
            (97..=303).contains(&x) && (88..=94).contains(&y),
            "changed pixel ({x}, {y}) outside the underline band"
        );
    }
}

// ============================================================================
// Form widget appearance synthesis
// ============================================================================

/// Assembles a PDF from object bodies (object N = objects[N-1]) with a
/// programmatic xref. Same shape as save_roundtrip_tests.rs's helper.
#[cfg(feature = "rendering")]
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

/// One-page PDF with a single Text widget at /Rect [100 600 300 640] that
/// has NO /AP and NO /V — the render must show nothing until a value is
/// set, then the synthesized appearance paints the text.
/// Object map: 1=Catalog, 2=Pages, 3=Page, 4=Text widget, 5=AcroForm.
#[cfg(feature = "rendering")]
fn text_widget_pdf() -> Vec<u8> {
    assemble_pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R] >>".to_string(),
            "<< /Type /Annot /Subtype /Widget /T (name) /FT /Tx /Rect [100 600 300 640] \
             /DA (/Helv 0 Tf 0 g) /Q 1 >>"
                .to_string(),
            "<< /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) >>".to_string(),
        ],
        "/Root 1 0 R",
    )
}

/// Setting a Text field's value must make the value visible in the viewer
/// even though no appearance stream exists: the renderer synthesizes one
/// (rendering/context/widget.rs). Device y = 792 - y_pdf puts the widget's
/// [600, 640] band at rows 152..192 at scale 1.
#[cfg(feature = "rendering")]
#[test]
fn text_widget_value_paints_without_appearance() {
    use pdf_x_core::core::PDFDocument;
    use pdf_x_device_tinyskia::RenderToImage;

    let original = text_widget_pdf();
    let mut doc = PDFDocument::open(original.clone()).expect("fixture should open");
    let baseline = doc
        .render_page_to_image(0, Some(1.0))
        .expect("baseline render");

    doc.set_field_value("name", "HELLO FORM")
        .expect("set value");

    // Save + reopen: the synthesized appearance must come from the persisted
    // /V, not from session state.
    let mut saved = original.clone();
    saved.extend_from_slice(&doc.incremental_update(&original).expect("update"));
    let mut reopened = PDFDocument::open(saved).expect("saved file should reopen");
    let (width, height, pixels) = reopened
        .render_page_to_image(0, Some(1.0))
        .expect("render after reopen");

    assert_eq!(
        (width, height),
        (baseline.0, baseline.1),
        "page size must not change"
    );

    let mut changed = Vec::new();
    for i in 0..(width as usize * height as usize) {
        let a = &pixels[i * 4..i * 4 + 4];
        let b = &baseline.2[i * 4..i * 4 + 4];
        if a != b {
            changed.push((i as u32 % width, i as u32 / width));
        }
    }
    assert!(!changed.is_empty(), "the field value must be visible");
    for (x, y) in changed {
        assert!(
            (95..=305).contains(&x) && (148..=196).contains(&y),
            "changed pixel ({x}, {y}) outside the text widget band"
        );
    }
}
