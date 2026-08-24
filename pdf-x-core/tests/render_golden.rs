//! Golden render-output regression harness.
//!
//! Purpose: pin the *exact* pixel output of the render pipeline for a fixed set
//! of fixtures, so that a deep representation refactor (e.g. an arena-indexed
//! object model) can be proven behavior-preserving — a change that alters even
//! one rendered pixel fails this test. This is the safety net that makes any
//! object-model rewrite reviewable.
//!
//! Each case renders `page` of a fixture at a fixed `scale` and compares the
//! decoded RGBA buffer + dimensions against a committed PNG under
//! `tests/fixtures/golden/`. On mismatch, the actual output is dumped to
//! `<name>.actual.png` next to the golden for visual diffing.
//!
//! Regenerate goldens after an *intentional* rendering change:
//!
//! ```sh
//! UPDATE_GOLDEN=1 cargo test --test render_golden --features rendering
//! ```
//!
//! Determinism note: tiny-skia plus embedded fonts and bundled standard-font
//! fallback (rustybuzz/ttf-parser, no system font scanning) are deterministic
//! run-to-run on a given host, so byte/pixel identity holds locally. Goldens
//! are host-generated; cross-platform pixel drift (if it ever appears on CI)
//! is a known limitation, not a regression.

#![cfg(feature = "rendering")]

use std::env;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::RenderToImage;

/// Fixed render scale for every case. A single value keeps goldens comparable
/// across cases and small.
const SCALE: f32 = 1.0;

/// Expected outcome of a case.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Renders successfully and must match its golden.
    Ok,
    /// Known-unrenderable (e.g. encrypted, or a fixture that intentionally
    /// exercises parser robustness without a renderable page). The case is
    /// recorded so it cannot silently flip to "renders" — if it starts
    /// rendering, the test flags it for a golden to be added.
    Skip,
}

struct Case {
    fixture: &'static str,
    page: usize,
    expect: Expect,
}

/// The manifest. Every fixture in `tests/fixtures/pdfs/` is listed here with
/// its currently-observed outcome, so coverage is explicit and stable. A `Skip`
/// records a fixture the pipeline currently cannot open/render (with the
/// reason); if it ever starts rendering, the test flags the flip so a golden
/// can be added — it never silently passes.
static CASES: &[Case] = &[
    Case {
        fixture: "annotation-border-styles.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    // Renders after lazy fetch-time recovery: this fixture's xref has stale object offsets
    // (parses fine, but offsets point at wrong bytes); the first failing fetch triggers a
    // full-file xref rebuild, then the page renders.
    Case {
        fixture: "annotation-text.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    Case {
        fixture: "asciihexdecode.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    // Open fail: malformed xref (offset 999999 beyond 427-byte stream) — this
    // fixture is a parser-robustness probe, not a renderable page.
    Case {
        fixture: "bad-xref.pdf",
        page: 0,
        expect: Expect::Skip,
    },
    Case {
        fixture: "basicapi.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    // V=5/R=6 AES-256, empty user password, catalog in an ObjStm — proves end-to-end
    // decryption (open → auto-auth empty password → decrypt streams/strings on fetch).
    Case {
        fixture: "catalog-in-objstm-aes.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    // Opens via full-file recovery + ObjStm indexing (page_count resolves to 1), but render
    // fails further on the separate content-stream bug ("Expected object number, got
    // Command('obj')") — the same bug blocking annotation-text/flatedecode/issue3115.
    Case {
        fixture: "compressed-object-stream.pdf",
        page: 0,
        expect: Expect::Skip,
    },
    Case {
        fixture: "empty.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    // Renders after lazy fetch-time recovery (stale xref offsets, like annotation-text).
    Case {
        fixture: "flatedecode.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    // Renders after lazy fetch-time recovery (stale xref offsets, like annotation-text).
    Case {
        fixture: "issue3115.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    // Opens + renders: linearization detection was made tolerant — /O may be an indirect
    // ref, and missing linearization fields now degrade gracefully instead of aborting open.
    Case {
        fixture: "linearized.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    Case {
        fixture: "rotation.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    Case {
        fixture: "simpletype3font.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    Case {
        fixture: "tracemonkey.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    Case {
        fixture: "TrueType_without_cmap.pdf",
        page: 0,
        expect: Expect::Ok,
    },
    // Opens via full-file recovery scan (was Skip: malformed `/Type /XRef` stream that
    // normal parse rejects; recovery rebuilds entries from real object offsets).
    Case {
        fixture: "xref-stream.pdf",
        page: 0,
        expect: Expect::Ok,
    },
];

/// Workspace `tests/fixtures` directory (one level above `pdf-x-core`).
fn fixtures_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("pdf-x-core has a parent workspace dir")
        .join("tests")
        .join("fixtures")
}

fn pdfs_dir() -> PathBuf {
    fixtures_dir().join("pdfs")
}

fn golden_dir() -> PathBuf {
    fixtures_dir().join("golden")
}

fn golden_path(fixture: &str, page: usize) -> PathBuf {
    let stem = fixture.strip_suffix(".pdf").unwrap_or(fixture);
    golden_dir().join(format!("{}_p{}.png", stem, page))
}

/// Encode RGBA pixels to a PNG on disk.
fn write_png(path: &PathBuf, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| format!("create {}: {}", path.display(), e))?;
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc
        .write_header()
        .map_err(|e| format!("png header {}: {}", path.display(), e))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("png write {}: {}", path.display(), e))?;
    Ok(())
}

/// Decode a PNG to (width, height, RGBA).
fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let dec = png::Decoder::new(Cursor::new(bytes));
    let mut reader = dec.read_info().map_err(|e| format!("png decode: {}", e))?;
    let (w, h) = (reader.info().width, reader.info().height);
    let mut buf = vec![0u8; reader.output_buffer_size()];
    reader
        .next_frame(&mut buf)
        .map_err(|e| format!("png read frame: {}", e))?;
    Ok((w, h, buf))
}

enum Rendered {
    Pixels((u32, u32, Vec<u8>)),
    /// Open or render failed; carries the error string.
    Failed(String),
}

fn render_case(case: &Case) -> Rendered {
    let path = pdfs_dir().join(case.fixture);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => return Rendered::Failed(format!("read {}: {}", case.fixture, e)),
    };
    let mut doc = match PDFDocument::open(bytes) {
        Ok(d) => d,
        Err(e) => return Rendered::Failed(format!("open {}: {}", case.fixture, e)),
    };
    match doc.render_page_to_image(case.page, Some(SCALE)) {
        Ok(px) => Rendered::Pixels(px),
        Err(e) => Rendered::Failed(format!("render {} p{}: {}", case.fixture, case.page, e)),
    }
}

/// Outcome of a single case under the current binary, classified against the
/// manifest's `expect`.
enum Verdict {
    /// Matches expectation and (for Ok) the golden.
    Pass,
    /// Ok case whose pixels differ from the golden.
    Differs,
    /// The observed renderability flipped vs. the manifest's `expect`.
    OutcomeFlipped(String),
}

fn check_case(case: &Case, update: bool) -> Result<Verdict, String> {
    let rendered = render_case(case);
    let golden = golden_path(case.fixture, case.page);

    match (&case.expect, &rendered) {
        (Expect::Skip, Rendered::Failed(_)) => {
            // Consistent with the recorded skip.
            Ok(Verdict::Pass)
        }
        (Expect::Skip, Rendered::Pixels(_)) => {
            // Was skipped, now renders — surface it so a golden can be added.
            Ok(Verdict::OutcomeFlipped(format!(
                "{}: now renders but manifest says Skip — add a golden and flip to Ok",
                case.fixture
            )))
        }
        (Expect::Ok, Rendered::Failed(msg)) => {
            // Was expected to render; now it doesn't.
            Ok(Verdict::OutcomeFlipped(format!(
                "{}: now fails ({}) but manifest says Ok",
                case.fixture, msg
            )))
        }
        (Expect::Ok, Rendered::Pixels((w, h, rgba))) => {
            if update {
                write_png(&golden, *w, *h, rgba)?;
                return Ok(Verdict::Pass);
            }
            let golden_bytes = fs::read(&golden).map_err(|e| {
                format!(
                    "missing golden {} (run UPDATE_GOLDEN=1 to generate): {}",
                    golden.display(),
                    e
                )
            })?;
            let (gw, gh, grgba) = decode_png(&golden_bytes)?;
            if (*w, *h) == (gw, gh) && rgba.as_slice() == grgba.as_slice() {
                Ok(Verdict::Pass)
            } else {
                // Dump the actual for a visual diff before failing.
                let _ = write_png(&actual_path(&golden), *w, *h, rgba);
                Ok(Verdict::Differs)
            }
        }
    }
}

fn actual_path(golden: &PathBuf) -> PathBuf {
    let mut p = golden.clone();
    let stem = p.file_stem().unwrap_or_default().to_owned();
    let mut name = stem.to_os_string();
    name.push(".actual.png"); // full extension — `with_extension` would drop ".actual"
    p.set_file_name(name);
    p
}

#[test]
fn golden_render() {
    let update = env::var_os("UPDATE_GOLDEN").is_some();
    if update {
        fs::create_dir_all(golden_dir()).expect("golden dir is creatable");
    }

    let mut errors: Vec<String> = Vec::new();
    let mut diffs = 0usize;
    let mut flips: Vec<String> = Vec::new();
    let mut ok_count = 0usize;

    for case in CASES {
        match check_case(case, update) {
            Ok(Verdict::Pass) => ok_count += 1,
            Ok(Verdict::Differs) => {
                diffs += 1;
                errors.push(format!(
                    "{} p{}: rendered pixels differ from golden (actual dumped to {})",
                    case.fixture,
                    case.page,
                    actual_path(&golden_path(case.fixture, case.page)).display()
                ));
            }
            Ok(Verdict::OutcomeFlipped(msg)) => flips.push(msg),
            Err(e) => errors.push(e),
        }
    }

    if update {
        eprintln!(
            "UPDATE_GOLDEN: wrote {} goldens to {} (errors: {})",
            ok_count,
            golden_dir().display(),
            errors.len()
        );
        // During update, IO errors are hard failures. Outcome flips are surfaced
        // too: regenerating goldens must never silently drop or gain coverage.
        if !errors.is_empty() {
            panic!("UPDATE_GOLDEN had errors:\n{}", errors.join("\n"));
        }
        if !flips.is_empty() {
            panic!(
                "UPDATE_GOLDEN: renderability flipped vs. manifest — update the manifest first:\n{}",
                flips.join("\n")
            );
        }
        return;
    }

    let mut report = String::new();
    if diffs > 0 {
        report.push_str(&format!(
            "rendering regressed against goldens ({} case(s) differ):\n",
            diffs
        ));
        report.push_str(&errors.join("\n"));
        report.push_str("\n\nIf this change is intentional, regenerate with: UPDATE_GOLDEN=1 cargo test --test render_golden --features rendering\n");
    }
    if !flips.is_empty() {
        if !report.is_empty() {
            report.push_str("\n");
        }
        report.push_str("renderability flipped vs. manifest (update the manifest):\n");
        report.push_str(&flips.join("\n"));
    }
    if !report.is_empty() {
        panic!("{report}");
    }
}
