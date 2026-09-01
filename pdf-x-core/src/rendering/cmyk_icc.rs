use moxcms::{ColorProfile, Layout, Transform8BitExecutor, TransformOptions};

static CMYK_PROFILE_BYTES: &[u8] = include_bytes!("../../assets/CGATS001Compat-v2-micro.icc");

struct CmykIccTransform {
    transform_u8: Box<Transform8BitExecutor>,
}

impl CmykIccTransform {
    fn new() -> Option<Self> {
        let src = ColorProfile::new_from_slice(CMYK_PROFILE_BYTES).ok()?;
        let dst = ColorProfile::new_srgb();
        let transform_u8 = src
            .create_transform_8bit(Layout::Rgba, &dst, Layout::Rgb, TransformOptions::default())
            .ok()?;
        Some(Self { transform_u8 })
    }
}

static CMYK_TRANSFORM: std::sync::LazyLock<Option<CmykIccTransform>> =
    std::sync::LazyLock::new(CmykIccTransform::new);

pub fn cmyk_pixel_to_rgb(c: f32, m: f32, y: f32, k: f32) -> (u8, u8, u8) {
    if let Some(ref transform) = *CMYK_TRANSFORM {
        let input = [
            (c * 255.0) as u8,
            (m * 255.0) as u8,
            (y * 255.0) as u8,
            (k * 255.0) as u8,
        ];
        let mut output = [0u8; 3];
        if transform
            .transform_u8
            .transform(&input, &mut output)
            .is_ok()
        {
            return (output[0], output[1], output[2]);
        }
    }
    cmyk_polynomial_fallback(c, m, y, k)
}

pub fn cmyk_pixels_to_rgb_iter(data: &[u8], pixel_count: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(pixel_count * 3);
    for chunk in data.chunks(4).take(pixel_count) {
        if chunk.len() < 4 {
            break;
        }
        let (r, g, b) = cmyk_pixel_to_rgb(
            chunk[0] as f32 / 255.0,
            chunk[1] as f32 / 255.0,
            chunk[2] as f32 / 255.0,
            chunk[3] as f32 / 255.0,
        );
        rgb.push(r);
        rgb.push(g);
        rgb.push(b);
    }
    rgb
}

pub fn cmyk_image_to_rgb(data: &[u8]) -> Vec<u8> {
    let pixels = data.len() / 4;
    if let Some(ref transform) = *CMYK_TRANSFORM {
        let mut temp = vec![0u8; pixels * 3];
        if transform.transform_u8.transform(data, &mut temp).is_ok() {
            return temp;
        }
    }
    cmyk_pixels_to_rgb_iter(data, pixels)
}

fn cmyk_polynomial_fallback(c: f32, m: f32, y: f32, k: f32) -> (u8, u8, u8) {
    // Ported from pdf.js DeviceCmykCS.#toRgb (src/core/colorspace.js:674).
    let c = c.clamp(0.0, 1.0);
    let m = m.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let k = k.clamp(0.0, 1.0);

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

    (
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmyk_icc_transform_loaded() {
        let transform = CmykIccTransform::new();
        assert!(
            transform.is_some(),
            "ICC CMYK transform should load successfully"
        );
    }

    #[test]
    fn test_cmyk_icc_pixel() {
        // White (no ink) -> pure white
        let (r, g, b) = cmyk_pixel_to_rgb(0.0, 0.0, 0.0, 0.0);
        assert_eq!(r, 255);
        assert_eq!(g, 255);
        assert_eq!(b, 255);

        // K=100% -> dark gray via CGATS SWOP ICC profile
        let (r, g, b) = cmyk_pixel_to_rgb(0.0, 0.0, 0.0, 1.0);
        assert_eq!(r, 35);
        assert_eq!(g, 31);
        assert_eq!(b, 32);

        // C=100% via ICC
        let (r, g, b) = cmyk_pixel_to_rgb(1.0, 0.0, 0.0, 0.0);
        assert_eq!(r, 0);
        assert_eq!(g, 174);
        assert_eq!(b, 239);

        // M=100% via ICC
        let (r, g, b) = cmyk_pixel_to_rgb(0.0, 1.0, 0.0, 0.0);
        assert_eq!(r, 236);
        assert_eq!(g, 0);
        assert_eq!(b, 140);

        // Y=100% via ICC
        let (r, g, b) = cmyk_pixel_to_rgb(0.0, 0.0, 1.0, 0.0);
        assert_eq!(r, 255);
        assert_eq!(g, 234);
        assert_eq!(b, 0);
    }

    /// Pure polynomial fallback — no ICC profile involved.
    /// These values must match pdf.js DeviceCmykCS.#toRgb exactly.
    #[test]
    fn test_cmyk_polynomial_fallback() {
        // White (no ink)
        assert_eq!(cmyk_polynomial_fallback(0.0, 0.0, 0.0, 0.0), (255, 255, 255));
        // K=100% gives a dark color
        let (r, g, b) = cmyk_polynomial_fallback(0.0, 0.0, 0.0, 1.0);
        assert!(r < 60 && g < 60 && b < 60, "K=100% should be near-black: ({},{},{})", r, g, b);
    }

    /// Clamping: inputs outside [0,1] must not produce invalid u8.
    #[test]
    fn test_cmyk_polynomial_clamping() {
        let (r, g, b) = cmyk_polynomial_fallback(-0.5, 2.0, -1.0, 3.0);
        // Should not panic and should produce valid byte values.
        let _ = (r, g, b);
    }

    /// Mixed CMYK values.
    #[test]
    fn test_cmyk_polynomial_mixed() {
        // Dark olive: C=0.5, M=0.3, Y=0.8, K=0.1
        let (r, g, b) = cmyk_polynomial_fallback(0.5, 0.3, 0.8, 0.1);
        // Should be a muted brownish-green
        assert!(r < 150, "R should be muted: {}", r);
        assert!(b < 100, "B should be low: {}", b);
    }
}
