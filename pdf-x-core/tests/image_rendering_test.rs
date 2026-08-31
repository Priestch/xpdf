//! Test image rendering with different color formats.
//!
//! Images follow PDF image-space semantics: the image always occupies the
//! unit square and the CTM positions/scales it onto the canvas. The
//! `transform` argument to `Device::draw_image` is identity — the content
//! stream sets up the CTM, mirroring how `rendering/context/xobject.rs`
//! invokes the device.
//!
//! Image data row 0 maps to the top of the unit square, so with
//! ctm = [2, 0, 0, 2, 10, 10] a 2x2 image covers device pixels
//! (10..12, 10..12) with data row 0 at y = 11 and column 0 at x = 10.
//!
//! This test verifies that:
//! - RGB images render correctly
//! - CMYK images convert to RGB and render correctly
//! - Grayscale images render correctly
//! - Images are positioned correctly with transforms

use pdf_x_core::rendering::{Device, ImageData};
use pdf_x_device_tinyskia::SkiaDevice;
use tiny_skia::Pixmap;

#[test]
fn test_render_rgb_image() {
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut font_cache_device_1 = std::collections::HashMap::new();

    let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache_device_1);

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
        original_bits_per_component: 8,
        is_mask: false,
        mask_paint_one: true,
        mask_color: [0, 0, 0],
        interpolate: false,
    };

    // Position via the CTM: 2x2 image at 1:1 scale over (10, 10)
    device.set_matrix(&[2.0, 0.0, 0.0, 2.0, 10.0, 10.0]);
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        .unwrap();

    // Data row 0 (red, green) lands on device y = 11
    let pixel_index = (11 * 100 + 10) * 4;
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

    // Green pixel (row 0, column 1) at (11, 11)
    let pixel_index2 = (11 * 100 + 11) * 4;
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

    let mut font_cache_device_2 = std::collections::HashMap::new();

    let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache_device_2);

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

    // The core converts CMYK to RGB before the image reaches the device
    // (pdf.js polynomial coefficients; see cmyk_icc.rs and test_cmyk_conversion_formula)
    let rgb_data = pdf_x_core::rendering::cmyk_icc::cmyk_image_to_rgb(&image_data);

    let image = ImageData {
        width: 2,
        height: 2,
        data: rgb_data,
        has_alpha: false,
        bits_per_component: 8,
        original_bits_per_component: 8,
        is_mask: false,
        mask_paint_one: true,
        mask_color: [0, 0, 0],
        interpolate: false,
    };

    // Position via the CTM: 2x2 image at 1:1 scale over (10, 10)
    device.set_matrix(&[2.0, 0.0, 0.0, 2.0, 10.0, 10.0]);
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        .unwrap();

    // Data row 0 starts with CMYK red → device (10, 11)
    let pixel_index = (11 * 100 + 10) * 4;
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

    let mut font_cache_device_3 = std::collections::HashMap::new();

    let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache_device_3);

    // Set a clean transform (no Y-flip) for easier testing
    device.set_matrix(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    // Create a 2x2 grayscale image
    // row 0: black, medium gray — row 1: white, dark gray
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
        original_bits_per_component: 8,
        is_mask: false,
        mask_paint_one: true,
        mask_color: [0, 0, 0],
        interpolate: false,
    };

    // Position via the CTM: 2x2 image at 1:1 scale over (10, 10)
    device.set_matrix(&[2.0, 0.0, 0.0, 2.0, 10.0, 10.0]);
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        .unwrap();

    // Data row 0, column 0 (black) lands on device (10, 11)
    let pixel_index = (11 * 100 + 10) * 4;
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

    // Data row 1, column 0 (white) lands on device (10, 10)
    let pixel_index2 = (10 * 100 + 10) * 4;
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
    // Test that images render correctly with identity draw transform:
    // the CTM alone positions a 10x10 image over (50, 50)
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut font_cache_device_4 = std::collections::HashMap::new();

    let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache_device_4);

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
        original_bits_per_component: 8,
        is_mask: false,
        mask_paint_one: true,
        mask_color: [0, 0, 0],
        interpolate: false,
    };

    // 10x10 image at 1:1 scale over (50, 50): the unit square must span
    // 10x10 device pixels, so the CTM scale is 10, not 1
    device.set_matrix(&[10.0, 0.0, 0.0, 10.0, 50.0, 50.0]);
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        .unwrap();

    // Check center pixel at (55, 55)
    let pixel_index = (55 * 100 + 55) * 4;
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    assert!(pixel_color[0] > 200, "Should be red (R high)");
    assert!(pixel_color[1] < 50, "Should be red, not white (G low)");
}

#[test]
fn test_rgba_image_passthrough() {
    // Test that RGBA images pass through without modification
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);

    let mut font_cache_device_5 = std::collections::HashMap::new();

    let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache_device_5);

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
        original_bits_per_component: 8,
        is_mask: false,
        mask_paint_one: true,
        mask_color: [0, 0, 0],
        interpolate: false,
    };

    // Position via the CTM: 2x2 image at 1:1 scale over (10, 10)
    device.set_matrix(&[2.0, 0.0, 0.0, 2.0, 10.0, 10.0]);
    device
        .draw_image(image, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        .unwrap();

    // Semi-transparent red over white blends to pink; just verify the blend
    // happened (a pure white pixel would mean nothing was drawn)
    let pixel_index = (11 * 100 + 10) * 4;
    let pixel_color = &pixmap.data()[pixel_index..pixel_index + 4];

    assert!(
        pixel_color[0] < 250 || pixel_color[1] < 250 || pixel_color[2] < 250,
        "Should have drawn non-white pixel"
    );
    assert!(
        pixel_color[1] > 50 && pixel_color[1] < 250,
        "Semi-transparent red over white should blend G to mid-range, got {}",
        pixel_color[1]
    );
}
