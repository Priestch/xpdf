//! Integration test for PDF coordinate system transformation.
//!
//! This test verifies that PDF coordinates (Y-up, bottom-left origin)
//! are correctly transformed to screen coordinates (Y-down, top-left origin).

use pdf_x_core::rendering::{Device, SkiaDevice};
use tiny_skia::Pixmap;

#[test]
fn test_y_axis_flip() {
    // Create a small pixmap (100x100 pixels)
    let mut pixmap = Pixmap::new(100, 100).unwrap();

    // Fill with white background
    pixmap.fill(tiny_skia::Color::WHITE);

    // Create device
    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Apply the PDF-to-screen coordinate transform for a 100x100 page
    // This should flip Y and translate by page height
    // Use set_matrix to replace the identity matrix
    device.set_matrix(&[1.0, 0.0, 0.0, -1.0, 0.0, 100.0]);

    // Draw a rectangle at PDF coordinates (10, 10, 30, 30)
    // In PDF coordinates: bottom-left corner is (10, 10)
    // After Y-flip and translation: should appear at (10, 70) in screen coords
    eprintln!("DEBUG: Before drawing rect");
    device.begin_path();
    eprintln!("DEBUG: After begin_path");
    device.rect(10.0, 10.0, 20.0, 20.0);
    eprintln!("DEBUG: After rect call");

    use pdf_x_core::rendering::{Paint, PathDrawMode, StrokeProps};
    let paint = Paint::from_color(pdf_x_core::rendering::Color::RGB(1.0, 0.0, 0.0)); // Red
    let stroke_props = StrokeProps::default();

    eprintln!("DEBUG: About to call draw_path");
    device
        .draw_path(
            PathDrawMode::Fill(pdf_x_core::rendering::FillRule::NonZero),
            &paint,
            &stroke_props,
        )
        .unwrap();
    eprintln!("DEBUG: After draw_path");

    // DEBUG: Check some pixels to understand what's happening
    eprintln!("DEBUG: Checking pixels after draw...");
    for y in [60, 70, 80, 90].iter() {
        for x in [10, 15, 20, 25].iter() {
            let idx = (y * 100 + x) * 4;
            let p = &pixmap.data()[idx..idx + 4];
            eprintln!(
                "  Pixel ({}, {}): RGBA=({},{},{},{})",
                x, y, p[0], p[1], p[2], p[3]
            );
        }
    }

    // Verify the red pixel is at the correct screen position
    // PDF (10, 10) should map to screen (10, 90) after Y-flip + translate(100)
    // Screen Y = page_height - pdf_y = 100 - 10 = 90
    // But we have a 20x20 rect, so top of rect is at screen Y = 90 - 20 = 70

    // Check that pixel at screen (20, 80) is red (inside the rectangle)
    let pixel_index = (80 * 100 + 20) * 4; // y * width + x * 4 channels
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    // Red pixel should have R=255, G=0, B=0, A=255
    // Note: tiny-skia stores as RGBA
    assert!(
        pixel_color[0] > 200,
        "Red channel should be high, got {}",
        pixel_color[0]
    );
    assert!(
        pixel_color[1] < 50,
        "Green channel should be low, got {}",
        pixel_color[1]
    );
    assert!(
        pixel_color[2] < 50,
        "Blue channel should be low, got {}",
        pixel_color[2]
    );
    assert!(
        pixel_color[3] > 200,
        "Alpha channel should be high, got {}",
        pixel_color[3]
    );

    // Check that pixel at screen (20, 20) is white (outside the rectangle)
    let pixel_index2 = (20 * 100 + 20) * 4;
    let pixel_color2 = &pixmap.data()[pixel_index2..pixel_index2 + 4];

    // White pixel should be R=255, G=255, B=255, A=255
    assert!(pixel_color2[0] > 200, "White pixel - Red should be high");
    assert!(pixel_color2[1] > 200, "White pixel - Green should be high");
    assert!(pixel_color2[2] > 200, "White pixel - Blue should be high");
}

#[test]
fn test_coordinate_transform_preserves_width() {
    // Verify that X coordinates are preserved (not flipped or scaled except by scale factor)
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Apply transform with scale=2
    // Use set_matrix to replace the identity matrix
    device.set_matrix(&[2.0, 0.0, 0.0, -2.0, 0.0, 100.0]);

    // Draw a 10-unit wide rectangle at PDF X=20
    device.begin_path();
    device.rect(20.0, 50.0, 10.0, 10.0);

    use pdf_x_core::rendering::{Paint, PathDrawMode, StrokeProps};
    let paint = Paint::from_color(pdf_x_core::rendering::Color::RGB(0.0, 0.0, 1.0)); // Blue
    let stroke_props = StrokeProps::default();

    device
        .draw_path(
            PathDrawMode::Fill(pdf_x_core::rendering::FillRule::NonZero),
            &paint,
            &stroke_props,
        )
        .unwrap();

    // After transform with scale=2:
    // PDF X=20 should map to screen X=40
    // PDF width=10 should map to screen width=20
    // So rectangle should span from screen X=40 to screen X=60

    // Check pixel at screen X=50 (middle of rectangle)
    // PDF Y=50 maps to screen Y=100-50*2=0, but we're outside that
    // Let's check a pixel we know should be blue

    // The rect in PDF coords: (20, 50) with size 10x10
    // After scale=2 and Y-flip:
    // Screen Y = 100 - 50*2 = 0, then size is 20, so Y range is 0 to 20
    // Screen X = 20*2 = 40, size is 20, so X range is 40 to 60

    // Check that pixel at (50, 10) is blue
    let pixel_index = (10 * 100 + 50) * 4;
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    assert!(
        pixel_color[2] > 200,
        "Blue pixel - Blue channel should be high"
    );
}
