//! Rendering regression tests for PDF-X.
//!
//! These tests verify the rendering pipeline produces consistent output
//! for various PDF content stream operations.

use pdf_x_core::rendering::graphics_state::{Color, FillRule, StrokeProps};
use pdf_x_core::rendering::{Device, Paint, PathDrawMode, TestDevice};

#[cfg(feature = "rendering")]
mod test_utils;

#[cfg(feature = "rendering")]
use pdf_x_core::rendering::skia_device::SkiaDevice;

#[cfg(feature = "rendering")]
use tiny_skia::Pixmap;

#[cfg(feature = "rendering")]
use test_utils::get_test_pdf_path;

#[cfg(feature = "rendering")]
use pdf_x_core::PDFDocument;

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
            0.0,
            0.0,
            &paint,
            &[1.0, 0.0, 0.0, 1.0, 100.0, 700.0],
            100.0,
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
            0.0,
            0.0,
            &paint,
            &[1.0, 0.0, 0.0, 1.0, 100.0, 700.0],
            100.0,
            0.0,
        )
        .unwrap();
    device
        .draw_text(
            b"Second line",
            "Times-Roman",
            14.0,
            0.0,
            0.0,
            &paint,
            &[1.0, 0.0, 0.0, 1.0, 100.0, 680.0],
            100.0,
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

#[cfg(feature = "rendering")]
#[derive(Debug)]
struct RenderMetrics {
    width: u32,
    height: u32,
    non_white: usize,
    dark_pixels: usize,
    rows_with_ink: usize,
    cols_with_ink: usize,
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
    signature: Vec<u8>,
}

#[cfg(feature = "rendering")]
fn analyze_rendered_pixels(
    width: u32,
    height: u32,
    pixels: &[u8],
) -> (usize, usize, usize, usize, usize, usize, usize, usize) {
    let width = width as usize;
    let height = height as usize;
    let mut non_white = 0usize;
    let mut dark_pixels = 0usize;
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut rows_with_ink = 0usize;

    for y in 0..height {
        let mut row_has_ink = false;
        for x in 0..width {
            let idx = (y * width + x) * 4;
            let r = pixels[idx];
            let g = pixels[idx + 1];
            let b = pixels[idx + 2];
            if r != 255 || g != 255 || b != 255 {
                non_white += 1;
                row_has_ink = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                if r < 80 && g < 80 && b < 80 {
                    dark_pixels += 1;
                }
            }
        }
        if row_has_ink {
            rows_with_ink += 1;
        }
    }

    let cols_with_ink = (0..width)
        .filter(|&x| {
            (0..height).any(|y| {
                let idx = (y * width + x) * 4;
                pixels[idx] != 255 || pixels[idx + 1] != 255 || pixels[idx + 2] != 255
            })
        })
        .count();

    (
        non_white,
        dark_pixels,
        rows_with_ink,
        cols_with_ink,
        min_x,
        min_y,
        max_x,
        max_y,
    )
}

#[cfg(feature = "rendering")]
fn coarse_density_signature(
    width: u32,
    height: u32,
    pixels: &[u8],
    cols: usize,
    rows: usize,
) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let mut signature = Vec::with_capacity(cols * rows);

    for row in 0..rows {
        let y0 = row * height / rows;
        let y1 = ((row + 1) * height / rows).min(height);
        for col in 0..cols {
            let x0 = col * width / cols;
            let x1 = ((col + 1) * width / cols).min(width);
            let mut non_white = 0usize;
            let mut total = 0usize;

            for y in y0..y1 {
                for x in x0..x1 {
                    let idx = (y * width + x) * 4;
                    total += 1;
                    if pixels[idx] != 255 || pixels[idx + 1] != 255 || pixels[idx + 2] != 255 {
                        non_white += 1;
                    }
                }
            }

            let bucket = ((non_white * 10) / total).min(9) as u8;
            signature.push(bucket);
        }
    }

    signature
}

#[cfg(feature = "rendering")]
fn trim_trailing_zeros(signature: &[u8]) -> &[u8] {
    let mut end = signature.len();
    while end > 0 && signature[end - 1] == 0 {
        end -= 1;
    }
    &signature[..end]
}

#[cfg(feature = "rendering")]
fn render_fixture_metrics(name: &str) -> RenderMetrics {
    let pdf_path = get_test_pdf_path(name);
    assert!(pdf_path.exists(), "missing test fixture: {}", pdf_path.display());

    let pdf_bytes = std::fs::read(&pdf_path).expect("should read regression fixture");
    let mut doc = PDFDocument::open(pdf_bytes).expect("should open regression fixture");

    let (width, height, pixels) = doc
        .render_page_to_image(0, Some(2.0))
        .expect("should render regression fixture");

    let (non_white, dark_pixels, rows_with_ink, cols_with_ink, min_x, min_y, max_x, max_y) =
        analyze_rendered_pixels(width, height, &pixels);
    let signature = coarse_density_signature(width, height, &pixels, 12, 16);

    RenderMetrics {
        width,
        height,
        non_white,
        dark_pixels,
        rows_with_ink,
        cols_with_ink,
        min_x,
        min_y,
        max_x,
        max_y,
        signature,
    }
}

#[cfg(feature = "rendering")]
#[test]
fn test_absw_page2_text_render_regression() {
    let metrics = render_fixture_metrics("absw-page2-text-regression.pdf");
    let expected_signature = vec![
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
        6, 6, 7, 7, 7, 7, 7, 0, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0, 0, 9, 9, 9, 9, 9, 9,
        9, 9, 0, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0, 2, 9, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0,
        4, 8, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 0, 0, 0, 5, 6, 7, 5, 7,
        6, 5, 5, 0, 0, 0, 0, 8, 9, 9, 9, 9, 9, 9, 9, 7, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 9, 0,
        0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 8, 0, 0, 0, 7, 5, 4, 5, 5, 5, 5, 4, 3, 0, 0, 0, 1, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    assert_eq!((metrics.width, metrics.height), (1008, 1332));
    assert!(
        (580_000..=600_000).contains(&metrics.non_white),
        "unexpected non-white coverage: {}",
        metrics.non_white
    );
    assert!(
        (575_000..=590_000).contains(&metrics.dark_pixels),
        "unexpected dark-pixel coverage: {}",
        metrics.dark_pixels
    );
    assert!(
        (930..=960).contains(&metrics.rows_with_ink),
        "text or vector content shifted vertically: {} rows with ink",
        metrics.rows_with_ink
    );
    assert!(
        (760..=790).contains(&metrics.cols_with_ink),
        "text or vector content shifted horizontally: {} columns with ink",
        metrics.cols_with_ink
    );
    assert!(
        (225..=245).contains(&metrics.min_x),
        "unexpected left ink bound: {}",
        metrics.min_x
    );
    assert!(
        (175..=190).contains(&metrics.min_y),
        "unexpected top ink bound: {}",
        metrics.min_y
    );
    assert!(metrics.max_x >= 1000, "unexpected right ink bound: {}", metrics.max_x);
    assert!(
        (1180..=1205).contains(&metrics.max_y),
        "unexpected bottom ink bound: {}",
        metrics.max_y
    );
    assert_eq!(
        metrics.signature, expected_signature,
        "coarse render signature drifted for absw page 2"
    );
}

#[cfg(feature = "rendering")]
#[test]
fn test_absw_page3_text_render_regression() {
    let metrics = render_fixture_metrics("absw-page3-text-regression.pdf");
    let expected_signature = vec![
        0, 0, 0, 8, 9, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0, 5, 9, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0, 9, 9,
        9, 9, 9, 9, 9, 9, 9, 0, 0, 2, 4, 8, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0, 0, 1, 7, 9, 9, 9, 8,
        6, 2, 0, 0, 0, 7, 7, 8, 6, 1, 1, 0, 0, 0, 0, 0, 0, 9, 9, 9, 7, 1, 0, 0, 0, 0, 0, 0, 0,
        9, 9, 9, 9, 4, 0, 0, 0, 0, 0, 0, 0, 9, 9, 9, 8, 3, 3, 3, 3, 2, 0, 0, 0, 9, 9, 9, 9, 9,
        9, 9, 9, 7, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 6, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 7, 0,
        0, 0, 8, 9, 9, 8, 9, 8, 8, 8, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    assert_eq!((metrics.width, metrics.height), (1008, 1332));
    assert!(
        (630_000..=645_000).contains(&metrics.non_white),
        "unexpected non-white coverage: {}",
        metrics.non_white
    );
    assert!(
        (622_000..=635_000).contains(&metrics.dark_pixels),
        "unexpected dark-pixel coverage: {}",
        metrics.dark_pixels
    );
    assert!(
        (1090..=1120).contains(&metrics.rows_with_ink),
        "text or vector content shifted vertically: {} rows with ink",
        metrics.rows_with_ink
    );
    assert!(
        (790..=810).contains(&metrics.cols_with_ink),
        "text or vector content shifted horizontally: {} columns with ink",
        metrics.cols_with_ink
    );
    assert!(
        (200..=220).contains(&metrics.min_x),
        "unexpected left ink bound: {}",
        metrics.min_x
    );
    assert!(metrics.min_y <= 5, "unexpected top ink bound: {}", metrics.min_y);
    assert!(metrics.max_x >= 1000, "unexpected right ink bound: {}", metrics.max_x);
    assert!(
        (1095..=1115).contains(&metrics.max_y),
        "unexpected bottom ink bound: {}",
        metrics.max_y
    );
    assert_eq!(
        trim_trailing_zeros(&metrics.signature),
        trim_trailing_zeros(&expected_signature),
        "coarse render signature drifted for absw page 3"
    );
}

#[cfg(feature = "rendering")]
#[test]
fn test_absw_page4_text_render_regression() {
    let metrics = render_fixture_metrics("absw-page4-text-regression.pdf");
    let expected_signature = vec![
        0, 0, 0, 0, 0, 0, 8, 9, 9, 9, 9, 3, 0, 0, 0, 0, 0, 0, 1, 4, 4, 4, 4, 0, 0, 0, 0, 4, 0,
        1, 1, 0, 1, 0, 1, 2, 0, 0, 0, 8, 7, 8, 7, 8, 9, 8, 9, 7, 0, 0, 0, 9, 7, 8, 9, 9, 9, 9,
        9, 8, 0, 0, 0, 9, 6, 7, 9, 9, 9, 9, 9, 9, 0, 0, 0, 9, 6, 9, 9, 9, 9, 9, 8, 9, 0, 0, 0,
        9, 7, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0, 9, 8, 7, 9, 9, 9, 9, 9, 9, 0, 0, 0, 9, 7, 8, 9, 9,
        9, 9, 9, 9, 0, 0, 0, 9, 7, 7, 9, 9, 9, 9, 9, 9, 0, 0, 0, 9, 7, 8, 9, 9, 9, 9, 9, 9, 0,
        0, 0, 8, 8, 7, 9, 8, 9, 9, 9, 9, 0, 0, 0, 8, 8, 7, 8, 9, 8, 9, 9, 8, 0, 0, 0, 4, 6, 7,
        6, 6, 6, 4, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    assert_eq!((metrics.width, metrics.height), (1008, 1332));
    assert!(
        (720_000..=740_000).contains(&metrics.non_white),
        "unexpected non-white coverage: {}",
        metrics.non_white
    );
    assert!(
        (700_000..=715_000).contains(&metrics.dark_pixels),
        "unexpected dark-pixel coverage: {}",
        metrics.dark_pixels
    );
    assert!(
        (1_180..=1_240).contains(&metrics.rows_with_ink),
        "text or vector content shifted vertically: {} rows with ink",
        metrics.rows_with_ink
    );
    assert!(
        (750..=780).contains(&metrics.cols_with_ink),
        "text or vector content shifted horizontally: {} columns with ink",
        metrics.cols_with_ink
    );
    assert!(
        (235..=255).contains(&metrics.min_x),
        "unexpected left ink bound: {}",
        metrics.min_x
    );
    assert!(metrics.min_y <= 5, "unexpected top ink bound: {}", metrics.min_y);
    assert!(metrics.max_x >= 1000, "unexpected right ink bound: {}", metrics.max_x);
    assert!(
        (1260..=1285).contains(&metrics.max_y),
        "unexpected bottom ink bound: {}",
        metrics.max_y
    );
    assert_eq!(
        metrics.signature, expected_signature,
        "coarse render signature drifted for absw page 4"
    );
}
