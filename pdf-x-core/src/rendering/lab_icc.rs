use moxcms::{ColorProfile, Layout, TransformOptions, Xyzd};

static LAB_PROFILE_BYTES: &[u8] = include_bytes!("../../assets/LAB.icc");

fn create_lab_transform(xw: f64, yw: f64, zw: f64) -> Option<Box<moxcms::TransformF32BitExecutor>> {
    let mut src = ColorProfile::new_from_slice(LAB_PROFILE_BYTES).ok()?;
    src.white_point = Xyzd::new(xw, yw, zw);
    let dst = ColorProfile::new_srgb();
    src.create_transform_f32(Layout::Rgb, &dst, Layout::Rgb, TransformOptions::default())
        .ok()
}

/// Convert a PDF Lab color to sRGB through the same bundled LAB.icc strategy
/// hayro uses. The analytical pdf.js conversion remains the caller fallback.
pub fn lab_pixel_to_rgb(l: f64, a: f64, b: f64, xw: f64, yw: f64, zw: f64) -> Option<(u8, u8, u8)> {
    let transform = create_lab_transform(xw, yw, zw)?;

    let input = [
        (l / 100.0).clamp(0.0, 1.0) as f32,
        ((a + 128.0) / 255.0).clamp(0.0, 1.0) as f32,
        ((b + 128.0) / 255.0).clamp(0.0, 1.0) as f32,
    ];
    let mut output = [0.0f32; 3];
    transform.transform(&input, &mut output).ok()?;
    Some((
        (output[0] * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
        (output[1] * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
        (output[2] * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
    ))
}

pub fn lab_pixels_to_rgb(
    data: &[u8],
    xw: f64,
    yw: f64,
    zw: f64,
    amin: f64,
    amax: f64,
    bmin: f64,
    bmax: f64,
) -> Option<Vec<u8>> {
    let transform = create_lab_transform(xw, yw, zw)?;
    let pixel_count = data.len() / 3;
    let mut input = Vec::with_capacity(pixel_count * 3);
    for chunk in data.chunks_exact(3) {
        let l = chunk[0] as f64 * 100.0 / 255.0;
        let a = amin + chunk[1] as f64 * (amax - amin) / 255.0;
        let b = bmin + chunk[2] as f64 * (bmax - bmin) / 255.0;
        input.push((l / 100.0).clamp(0.0, 1.0) as f32);
        input.push(((a + 128.0) / 255.0).clamp(0.0, 1.0) as f32);
        input.push(((b + 128.0) / 255.0).clamp(0.0, 1.0) as f32);
    }

    let mut output = vec![0.0f32; pixel_count * 3];
    transform.transform(&input, &mut output).ok()?;
    Some(
        output
            .into_iter()
            .map(|component| (component * 255.0 + 0.5).clamp(0.0, 255.0) as u8)
            .collect(),
    )
}
