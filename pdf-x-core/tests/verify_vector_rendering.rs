//! Test to verify vector graphics rendering works correctly
//!
//! This test directly constructs vector graphics using the Device trait
//! to verify that the rendering pipeline works independently of PDF parsing.

use pdf_x_core::rendering::{
    Color, Device, FillRule, Paint, PathDrawMode, SkiaDevice, StrokeProps,
};
use tiny_skia::Pixmap;

#[test]
#[cfg(feature = "rendering")]
fn test_colored_shapes_rendering() {
    println!("\nDEBUG: Testing colored shapes rendering...");

    let width = 400u32;
    let height = 400u32;

    let mut pixmap = Pixmap::new(width, height).expect("Failed to create pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Test 1: Red rectangle at (50, 50) size 100x100
    println!("DEBUG: Drawing red rectangle");
    device.begin_path();
    device.rect(50.0, 50.0, 100.0, 100.0);
    device
        .draw_path(
            PathDrawMode::Fill(FillRule::NonZero),
            &Paint::Solid(Color::rgb(255, 0, 0)),
            &StrokeProps::default(),
        )
        .expect("Failed to draw path");

    // Test 2: Green circle at (200, 100) radius 50
    println!("DEBUG: Drawing green circle");
    device.begin_path();
    device.move_to(250.0, 100.0);
    device.rect(200.0, 50.0, 100.0, 100.0);
    device
        .draw_path(
            PathDrawMode::Fill(FillRule::NonZero),
            &Paint::Solid(Color::rgb(0, 255, 0)),
            &StrokeProps::default(),
        )
        .expect("Failed to draw path");

    // Test 3: Blue rectangle at (50, 200) size 100x100
    println!("DEBUG: Drawing blue rectangle");
    device.begin_path();
    device.rect(50.0, 200.0, 100.0, 100.0);
    device
        .draw_path(
            PathDrawMode::Fill(FillRule::NonZero),
            &Paint::Solid(Color::rgb(0, 0, 255)),
            &StrokeProps::default(),
        )
        .expect("Failed to draw path");

    // Test 4: Yellow rectangle at (200, 200) size 100x100
    println!("DEBUG: Drawing yellow rectangle");
    device.begin_path();
    device.rect(200.0, 200.0, 100.0, 100.0);
    device
        .draw_path(
            PathDrawMode::Fill(FillRule::NonZero),
            &Paint::Solid(Color::rgb(255, 255, 0)),
            &StrokeProps::default(),
        )
        .expect("Failed to draw path");

    // Check results
    let pixels = pixmap.data();
    let red_count = pixels
        .chunks(4)
        .filter(|p| p[0] > 200 && p[1] < 50 && p[2] < 50)
        .count();
    let green_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 50 && p[1] > 200 && p[2] < 50)
        .count();
    let blue_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 50 && p[1] < 50 && p[2] > 200)
        .count();
    let yellow_count = pixels
        .chunks(4)
        .filter(|p| p[0] > 200 && p[1] > 200 && p[2] < 50)
        .count();

    println!("DEBUG: Red pixels: {}", red_count);
    println!("DEBUG: Green pixels: {}", green_count);
    println!("DEBUG: Blue pixels: {}", blue_count);
    println!("DEBUG: Yellow pixels: {}", yellow_count);

    let total_colored = red_count + green_count + blue_count + yellow_count;
    println!("DEBUG: Total colored pixels: {}", total_colored);

    let output_path = "/tmp/debug_colored_shapes.png";
    pixmap.save_png(output_path).expect("Failed to save PNG");
    println!("DEBUG: Saved to {}", output_path);

    assert!(red_count > 100, "Expected red rectangle to be visible");
    assert!(green_count > 100, "Expected green rectangle to be visible");
    assert!(blue_count > 100, "Expected blue rectangle to be visible");
    assert!(
        yellow_count > 100,
        "Expected yellow rectangle to be visible"
    );

    println!("SUCCESS: All colored shapes rendered correctly!");
}

#[test]
#[cfg(feature = "rendering")]
fn test_transformed_shapes() {
    println!("\nDEBUG: Testing transformed shapes...");

    let width = 400u32;
    let height = 400u32;

    let mut pixmap = Pixmap::new(width, height).expect("Failed to create pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Apply a transform (translate + scale)
    device.concat_matrix(&[2.0, 0.0, 0.0, 2.0, 50.0, 50.0]);

    // Draw a rectangle at (10, 10) size 50x50
    // After transform: should appear at (2*10+50, 2*10+50) = (70, 70)
    device.begin_path();
    device.rect(10.0, 10.0, 50.0, 50.0);
    device
        .draw_path(
            PathDrawMode::Fill(FillRule::NonZero),
            &Paint::Solid(Color::rgb(255, 0, 0)),
            &StrokeProps::default(),
        )
        .expect("Failed to draw path");

    let pixels = pixmap.data();
    let non_white_count = pixels
        .chunks(4)
        .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
        .count();

    println!("DEBUG: Non-white pixels: {}", non_white_count);

    // Find bounds
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let r = pixels[idx];
            if r > 200 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    println!(
        "DEBUG: Red content bounds: x=[{}, {}], y=[{}, {}]",
        min_x, max_x, min_y, max_y
    );
    println!("DEBUG: Expected bounds: x=[70, 170], y=[70, 170]");

    // After transform (scale by 2, translate by 50):
    // Rectangle at (10, 10) with size (50, 50) maps to:
    // x: 2*10 + 50 = 70 to 2*60 + 50 = 170
    // y: 2*10 + 50 = 70 to 2*60 + 50 = 170

    assert!(
        non_white_count > 0,
        "Expected colored content after transform"
    );
    assert!(
        min_x >= 60 && min_x <= 80,
        "Expected min_x around 70, got {}",
        min_x
    );
    assert!(
        min_y >= 60 && min_y <= 80,
        "Expected min_y around 70, got {}",
        min_y
    );

    println!("SUCCESS: Transformed shapes rendered correctly!");
}
