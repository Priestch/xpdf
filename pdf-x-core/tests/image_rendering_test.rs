//! Test image rendering with different color formats.
//!
//! This test verifies that:
//! - RGB images render correctly
//! - CMYK images convert to RGB and render correctly
//! - Grayscale images render correctly
//! - Images are positioned correctly with transforms

use pdf_x_core::rendering::{Device, ImageData, SkiaDevice};
use tiny_skia::Pixmap;

#[test]
fn test_render_rgb_image() {
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Set a clean transform (no Y-flip) for easier testing
    device.set_matrix(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    // Create a simple 2x2 RGB image (red, green, blue, white pixels)
    let image_data = vec![
        255, 0, 0, // Red pixel
        0, 255, 0, // Green pixel
        0, 0, 255, // Blue pixel
        255, 255, 255, // White pixel
    ];

    let image = ImageData {
        width: 2,
        height: 2,
        data: image_data,
        has_alpha: false,
        bits_per_component: 8,
    };

    // Draw at (10, 10) with 1x scale (no scaling, just translation)
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 10.0, 10.0])
        .unwrap();

    // Check that red pixel exists at (10, 10) - top-left of image
    let pixel_index = (10 * 100 + 10) * 4;
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    // Should be red
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

    // Check that green pixel exists at (11, 10)
    let pixel_index2 = (10 * 100 + 11) * 4;
    let pixel_color2 = &pixmap.data()[pixel_index2..pixel_index2 + 4];

    // Should be green
    assert!(pixel_color2[0] < 50, "Red channel should be low for green");
    assert!(
        pixel_color2[1] > 200,
        "Green channel should be high for green, got {}",
        pixel_color2[1]
    );
    assert!(pixel_color2[2] < 50, "Blue channel should be low for green");
}

#[test]
fn test_render_cmyk_image() {
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Set a clean transform (no Y-flip) for easier testing
    device.set_matrix(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    // Create a 2x2 CMYK image
    // CMYK (0, 255, 255, 0) should convert to bright red
    // CMYK (255, 0, 255, 0) should convert to bright green
    // CMYK (255, 255, 0, 0) should convert to bright blue
    // CMYK (0, 0, 0, 0) should convert to white
    let image_data = vec![
        0, 255, 255, 0, // Red
        255, 0, 255, 0, // Green
        255, 255, 0, 0, // Blue
        0, 0, 0, 0, // White
    ];

    let image = ImageData {
        width: 2,
        height: 2,
        data: image_data,
        has_alpha: false,
        bits_per_component: 8,
    };

    // Draw at (10, 10) with 1x scale (no scaling, just translation)
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 10.0, 10.0])
        .unwrap();

    // Check that red pixel exists at (10, 10) - top-left of image
    let pixel_index = (10 * 100 + 10) * 4;
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    // Should be red (or close to it)
    assert!(
        pixel_color[0] > 200,
        "Red channel should be high for CMYK red, got {}",
        pixel_color[0]
    );
    assert!(
        pixel_color[1] < 100,
        "Green channel should be low for CMYK red, got {}",
        pixel_color[1]
    );
    assert!(
        pixel_color[2] < 100,
        "Blue channel should be low for CMYK red, got {}",
        pixel_color[2]
    );
}

#[test]
fn test_render_grayscale_image() {
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Set a clean transform (no Y-flip) for easier testing
    device.set_matrix(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    // Create a 2x2 grayscale image
    let image_data = vec![
        0,   // Black
        128, // Medium gray
        255, // White
        64,  // Dark gray
    ];

    let image = ImageData {
        width: 2,
        height: 2,
        data: image_data,
        has_alpha: false,
        bits_per_component: 8,
    };

    // Draw at (10, 10) with 1x scale (no scaling, just translation)
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 10.0, 10.0])
        .unwrap();

    // Check that black pixel exists at (10, 10) - top-left of image
    let pixel_index = (10 * 100 + 10) * 4;
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    // Should be black (or close to it)
    assert!(
        pixel_color[0] < 50,
        "Red channel should be low for black, got {}",
        pixel_color[0]
    );
    assert!(
        pixel_color[1] < 50,
        "Green channel should be low for black, got {}",
        pixel_color[1]
    );
    assert!(
        pixel_color[2] < 50,
        "Blue channel should be low for black, got {}",
        pixel_color[2]
    );

    // Check that white pixel exists at (10, 11) in image coordinates
    let pixel_index2 = (11 * 100 + 10) * 4;
    let pixel_color2 = &pixmap.data()[pixel_index2..pixel_index2 + 4];

    // Should be white
    assert!(
        pixel_color2[0] > 200,
        "Red channel should be high for white"
    );
    assert!(
        pixel_color2[1] > 200,
        "Green channel should be high for white"
    );
    assert!(
        pixel_color2[2] > 200,
        "Blue channel should be high for white"
    );
}

#[test]
fn test_cmyk_conversion_formula() {
    // Verify CMYK to RGB conversion formula using PDF.js polynomial coefficients
    // Reference: pdf.js/src/core/colorspace.js - DeviceCmykCS.#toRgb
    // CMYK values are normalized to 0-1 range for the formula

    // CMYK (0, 0, 0, 0) should produce white (no ink)
    let c = 0.0_f32;
    let m = 0.0_f32;
    let y = 0.0_f32;
    let k = 0.0_f32;

    let r = 255.0
        + c * (-4.387332384609988 * c
            + 54.48615194189176 * m
            + 18.82290502165302 * y
            + 212.25662451639585 * k
            - 285.2331026137004)
        + m * (1.7149763477362134 * m
            - 5.6096736904047315 * y
            - 17.873870861415444 * k
            - 5.497006427196366)
        + y * (-2.5217340131683033 * y - 21.248923337353073 * k + 17.5119270841813)
        + k * (-21.86122147463605 * k - 189.48180835922747);

    let g = 255.0
        + c * (8.841041422036149 * c
            + 60.118027045597366 * m
            + 6.871425592049007 * y
            + 31.159100130055922 * k
            - 79.2970844816548)
        + m * (-15.310361306967817 * m + 17.575251261109482 * y + 131.35250912493976 * k
            - 190.9453302588951)
        + y * (4.444339102852739 * y + 9.8632861493405 * k - 24.86741582555878)
        + k * (-20.737325471181034 * k - 187.80453709719578);

    let b = 255.0
        + c * (0.8842522430003296 * c + 8.078677503112928 * m + 30.89978309703729 * y
            - 0.23883238689178934 * k
            - 14.183576799673286)
        + m * (10.49593273432072 * m + 63.02378494754052 * y + 50.606957656360734 * k
            - 112.23884253719248)
        + y * (0.03296041114873217 * y + 115.60384449646641 * k - 193.58209356861505)
        + k * (-22.33816807309886 * k - 180.12613974708367);

    assert_eq!(r, 255.0, "White should have R=255");
    assert_eq!(g, 255.0, "White should have G=255");
    assert_eq!(b, 255.0, "White should have B=255");

    // CMYK (1.0, 0, 0, 0) - full cyan (normalized from 255/255 = 1.0)
    let c = 1.0_f32;
    let m = 0.0_f32;
    let y = 0.0_f32;
    let k = 0.0_f32;

    let r = 255.0
        + c * (-4.387332384609988 * c
            + 54.48615194189176 * m
            + 18.82290502165302 * y
            + 212.25662451639585 * k
            - 285.2331026137004)
        + m * (1.7149763477362134 * m
            - 5.6096736904047315 * y
            - 17.873870861415444 * k
            - 5.497006427196366)
        + y * (-2.5217340131683033 * y - 21.248923337353073 * k + 17.5119270841813)
        + k * (-21.86122147463605 * k - 189.48180835922747);

    let g = 255.0
        + c * (8.841041422036149 * c
            + 60.118027045597366 * m
            + 6.871425592049007 * y
            + 31.159100130055922 * k
            - 79.2970844816548)
        + m * (-15.310361306967817 * m + 17.575251261109482 * y + 131.35250912493976 * k
            - 190.9453302588951)
        + y * (4.444339102852739 * y + 9.8632861493405 * k - 24.86741582555878)
        + k * (-20.737325471181034 * k - 187.80453709719578);

    let b = 255.0
        + c * (0.8842522430003296 * c + 8.078677503112928 * m + 30.89978309703729 * y
            - 0.23883238689178934 * k
            - 14.183576799673286)
        + m * (10.49593273432072 * m + 63.02378494754052 * y + 50.606957656360734 * k
            - 112.23884253719248)
        + y * (0.03296041114873217 * y + 115.60384449646641 * k - 193.58209356861505)
        + k * (-22.33816807309886 * k - 180.12613974708367);

    // Cyan should have low red, high green and blue
    // Note: PDF.js uses polynomial approximation based on US Web Coated (SWOP) colorspace
    // so colors are approximate, not perfect
    assert!(r < 150.0, "Cyan should have low R, got {}", r);
    assert!(g > 150.0, "Cyan should have high G, got {}", g);
    assert!(b > 150.0, "Cyan should have high B, got {}", b);
}

#[test]
fn test_image_with_identity_transform() {
    // Test that images render correctly with identity transform (no scaling)
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Set a clean transform (no Y-flip) for easier testing
    device.set_matrix(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    // Create a 10x10 red square
    let pixel_count = 10 * 10;
    let mut image_data = Vec::with_capacity(pixel_count * 3);
    for _ in 0..pixel_count {
        image_data.extend_from_slice(&[255, 0, 0]); // Red
    }

    let image = ImageData {
        width: 10,
        height: 10,
        data: image_data,
        has_alpha: false,
        bits_per_component: 8,
    };

    // Draw at (50, 50) with identity transform
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 50.0, 50.0])
        .unwrap();

    // Check center pixel at (55, 55)
    let pixel_index = (55 * 100 + 55) * 4;
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    assert!(pixel_color[0] > 200, "Should be red");
}

#[test]
fn test_rgba_image_passthrough() {
    // Test that RGBA images pass through without modification
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut device = SkiaDevice::new(pixmap.as_mut());

    // Set a clean transform (no Y-flip) for easier testing
    device.set_matrix(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    // Create a 2x2 RGBA image with semi-transparent red
    let image_data = vec![
        255, 0, 0, 128, // Semi-transparent red
        0, 255, 0, 128, // Semi-transparent green
        0, 0, 255, 128, // Semi-transparent blue
        255, 255, 255, 64, // Semi-transparent white
    ];

    let image = ImageData {
        width: 2,
        height: 2,
        data: image_data,
        has_alpha: true,
        bits_per_component: 8,
    };

    // Draw at (10, 10) with 1x scale (no scaling, just translation)
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 10.0, 10.0])
        .unwrap();

    // Just verify it doesn't crash and draws something
    let pixel_index = (10 * 100 + 10) * 4;
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    // Should have some color (not white)
    assert!(
        pixel_color[0] < 250 || pixel_color[1] < 250 || pixel_color[2] < 250,
        "Should have drawn non-white pixel"
    );
}
