//! Rendering regression tests for PDF-X.
//!
//! These tests verify the rendering pipeline produces consistent output
//! for various PDF content stream operations.

use pdf_x_core::rendering::graphics_state::{Color, FillRule, StrokeProps};
use pdf_x_core::rendering::{Device, Paint, PathDrawMode, TestDevice};

#[cfg(feature = "rendering")]
use pdf_x_core::rendering::skia_device::SkiaDevice;

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
        )
        .unwrap();
    device
        .draw_text(
            b"Second line",
            "Times-Roman",
            14.0,
            &paint,
            &[1.0, 0.0, 0.0, 1.0, 100.0, 680.0],
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
    let mut device = SkiaDevice::new(pixmap.as_mut());

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
    let mut device = SkiaDevice::new(pixmap.as_mut());

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
    let mut device = SkiaDevice::new(pixmap.as_mut());

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
