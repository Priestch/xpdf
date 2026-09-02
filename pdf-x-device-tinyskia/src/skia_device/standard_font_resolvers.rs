//! Pre-built standard-font resolver functions for [`SkiaDevice`].
//!
//! Install via [`SkiaDevice::set_standard_font_resolver`].
//!
//! The default (no resolver, or resolver returns `None`) uses Foxit Base-14 CFF
//! compiled into `pdf-x-core` — the same font data as hayro and pdf.js's
//! `standard_fonts/Foxit*.pfb`.
//!
//! [`SkiaDevice`]: super::SkiaDevice

use std::path::{Path, PathBuf};

/// Map a BaseFont name to a Liberation TTF filename.
///
/// Covers all 12 Liberation variants (Sans/Serif/Mono × Regular/Bold/Italic/BoldItalic).
/// Returns `None` for Symbol/ZapfDingbats (no Liberation equivalent — fall through
/// to core's bundled Foxit PFB).
/// 12 Liberation TTF filenames, indexed by family (0=Sans, 1=Serif, 2=Mono)
/// × style (0=Regular, 1=Bold, 2=Italic, 3=BoldItalic).
const LIBERATION_FILES: [[&str; 4]; 3] = [
    // Sans
    [
        "LiberationSans-Regular.ttf",
        "LiberationSans-Bold.ttf",
        "LiberationSans-Italic.ttf",
        "LiberationSans-BoldItalic.ttf",
    ],
    // Serif
    [
        "LiberationSerif-Regular.ttf",
        "LiberationSerif-Bold.ttf",
        "LiberationSerif-Italic.ttf",
        "LiberationSerif-BoldItalic.ttf",
    ],
    // Mono
    [
        "LiberationMono-Regular.ttf",
        "LiberationMono-Bold.ttf",
        "LiberationMono-Italic.ttf",
        "LiberationMono-BoldItalic.ttf",
    ],
];

fn liberation_filename(base_font: &str) -> Option<&'static str> {
    let lower = base_font.to_ascii_lowercase();

    let symbol = lower.contains("symbol");
    let dingbats = base_font == "ZapfDingbats"
        || lower.contains("dingbat");
    if symbol || dingbats {
        return None;
    }

    // Detect style flags (same signals as `pick_fallback_standard_font_key` in core).
    let bold = lower.contains("bold")
        || lower.contains("black")
        || lower.contains("heavy")
        || lower.contains("medi")
        || lower.contains("demibold")
        || lower.contains("semibold");
    let italic = lower.contains("italic")
        || lower.contains("oblique")
        || lower.contains("slant")
        || lower.contains("kursiv");
    let style = match (bold, italic) {
        (true, true) => 3,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 0,
    };

    let mono = lower.contains("mono")
        || lower.contains("courier")
        || lower.contains("typewriter")
        || lower.contains("console");
    let serif = lower.contains("times")
        || lower.contains("roman")
        || lower.contains("serif")
        || lower.contains("nimbusrom")
        || lower.contains("garamond")
        || lower.contains("book")
        || lower.contains("palatino")
        || lower.contains("schoolbook");

    let family = if mono { 2 } else if serif { 1 } else { 0 };
    Some(LIBERATION_FILES[family][style])
}

/// Create a resolver that reads Liberation TTF files from `font_dir`.
///
/// Maps BaseFont names to the 12 Liberation TTF files:
/// `LiberationSans/Serif/Mono-{Regular,Bold,Italic,BoldItalic}.ttf`.
///
/// pdf.js uses Liberation TTFs for its Helvetica substitution path
/// (`font_substitutions.js`: Helvetica → `LiberationSans-*.ttf` with
/// `helvetica_factors.js` metric rescaling). This resolver covers all three
/// families (Sans/Serif/Mono) for completeness.
///
/// For Symbol and ZapfDingbats (which have no Liberation equivalent),
/// returns `None` so the core's bundled Foxit PFB is used instead.
///
/// # Example
/// ```no_run
/// use pdf_x_device_tinyskia::standard_font_resolvers::liberation_ttf_resolver;
/// use pdf_x_device_tinyskia::SkiaDevice;
///
/// let mut device: SkiaDevice = /* ... */;
/// device.set_standard_font_resolver(liberation_ttf_resolver("/path/to/fonts"));
/// ```
pub fn liberation_ttf_resolver(font_dir: impl AsRef<Path>) -> impl Fn(&str) -> Option<Vec<u8>> {
    let dir = font_dir.as_ref().to_path_buf();
    move |base_font: &str| {
        let filename = liberation_filename(base_font)?;
        std::fs::read(dir.join(filename)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liberation_maps_known_names() {
        assert_eq!(
            liberation_filename("Helvetica"),
            Some("LiberationSans-Regular.ttf")
        );
        assert_eq!(
            liberation_filename("Helvetica-Bold"),
            Some("LiberationSans-Bold.ttf")
        );
        assert_eq!(
            liberation_filename("Helvetica-Oblique"),
            Some("LiberationSans-Italic.ttf")
        );
        assert_eq!(
            liberation_filename("Helvetica-BoldOblique"),
            Some("LiberationSans-BoldItalic.ttf")
        );
        assert_eq!(
            liberation_filename("Times-Roman"),
            Some("LiberationSerif-Regular.ttf")
        );
        assert_eq!(
            liberation_filename("Times-Bold"),
            Some("LiberationSerif-Bold.ttf")
        );
        assert_eq!(
            liberation_filename("Times-Italic"),
            Some("LiberationSerif-Italic.ttf")
        );
        assert_eq!(
            liberation_filename("Times-BoldItalic"),
            Some("LiberationSerif-BoldItalic.ttf")
        );
        assert_eq!(
            liberation_filename("Courier"),
            Some("LiberationMono-Regular.ttf")
        );
        assert_eq!(
            liberation_filename("Courier-Bold"),
            Some("LiberationMono-Bold.ttf")
        );
        assert_eq!(
            liberation_filename("Courier-Oblique"),
            Some("LiberationMono-Italic.ttf")
        );
        assert_eq!(
            liberation_filename("Courier-BoldOblique"),
            Some("LiberationMono-BoldItalic.ttf")
        );
    }

    #[test]
    fn liberation_maps_common_aliases() {
        // These should all map to LiberationSans variants.
        assert_eq!(
            liberation_filename("Arial"),
            Some("LiberationSans-Regular.ttf")
        );
        assert_eq!(
            liberation_filename("Arial-BoldMT"),
            Some("LiberationSans-Bold.ttf")
        );
        // TimesNewRoman → serif family.
        assert_eq!(
            liberation_filename("TimesNewRomanPSMT"),
            Some("LiberationSerif-Regular.ttf")
        );
        // CourierNew → mono family.
        assert_eq!(
            liberation_filename("CourierNewPSMT"),
            Some("LiberationMono-Regular.ttf")
        );
    }

    #[test]
    fn liberation_symbol_dingbats_fall_through() {
        assert_eq!(liberation_filename("Symbol"), None);
        assert_eq!(liberation_filename("SymbolMT"), None);
        assert_eq!(liberation_filename("ZapfDingbats"), None);
    }

    #[test]
    fn liberation_unknown_name_fallback() {
        // Garamond → serif family (contains "garamond").
        assert_eq!(
            liberation_filename("Garamond"),
            Some("LiberationSerif-Regular.ttf")
        );
        assert_eq!(
            liberation_filename("Garamond-Bold"),
            Some("LiberationSerif-Bold.ttf")
        );
        assert_eq!(
            liberation_filename("Garamond-BoldOblique"),
            Some("LiberationSerif-BoldItalic.ttf")
        );
        // Verdana → sans family (not mono, not serif → sans default).
        assert_eq!(
            liberation_filename("Verdana"),
            Some("LiberationSans-Regular.ttf")
        );
    }

    #[test]
    fn liberation_resolver_reads_actual_files() {
        let font_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../pdf-x-core/assets/fonts");
        let resolver = liberation_ttf_resolver(&font_dir);

        // Should read actual TTF data.
        let data = resolver("Helvetica").expect("LiberationSans-Regular.ttf should exist");
        // TrueType files start with 0x00010000 or 'OTTO'. Liberation is TrueType.
        assert!(data.len() > 1000, "TTF file should be substantial");
        // Check TTF header: first 4 bytes are sfVersion (0x00010000 for TrueType).
        assert_eq!(&data[0..4], &[0x00, 0x01, 0x00, 0x00]);

        // Symbol should fall through (no Liberation file).
        assert!(resolver("Symbol").is_none());

        // ZapfDingbats should fall through.
        assert!(resolver("ZapfDingbats").is_none());

        // All 12 Liberation files should be readable.
        let names = [
            "Helvetica",
            "Helvetica-Bold",
            "Helvetica-Oblique",
            "Helvetica-BoldOblique",
            "Times-Roman",
            "Times-Bold",
            "Times-Italic",
            "Times-BoldItalic",
            "Courier",
            "Courier-Bold",
            "Courier-Oblique",
            "Courier-BoldOblique",
        ];
        for name in &names {
            let d = resolver(name).unwrap_or_else(|| {
                panic!("Liberation TTF for {name} should exist in {font_dir:?}")
            });
            assert!(d.len() > 1000, "{name}: TTF too small");
        }
    }
}
