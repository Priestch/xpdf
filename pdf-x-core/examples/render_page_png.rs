//! Render one PDF page to a raw RGBA buffer (u32le w, u32le h, RGBA8).
//!
//! ```text
//! render_page_png <pdf> <page0> <scale> <out.raw> [password]
//! ```
//!
//! Optional 5th arg / `PDFX_PASSWORD` env: user password for encrypted PDFs
//! that don't open with the empty password. Empty-password corpora still work
//! without it (auto-auth during `open`).
use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::RenderToImage;
use std::env;
use std::fs;
use std::io::Write;

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = &args[1];
    let page: usize = args[2].parse().expect("page number");
    let scale: f32 = args[3].parse().unwrap_or(1.0);
    let out = &args[4];
    let password = args
        .get(5)
        .cloned()
        .or_else(|| env::var("PDFX_PASSWORD").ok())
        .unwrap_or_default();

    let data = fs::read(path).expect("read pdf");
    let mut doc = PDFDocument::open(data).expect("open");
    if doc.is_encrypted() {
        // Empty password is auto-tried during open. If that failed, require a
        // non-empty password (arg or PDFX_PASSWORD) before any page fetch —
        // otherwise ciphertext is parsed as content and fails obscurely
        // (e.g. "Command token too long").
        let unlocked = doc.get_encryption_key().is_ok();
        if !unlocked {
            if password.is_empty() {
                eprintln!(
                    "error: encrypted PDF requires a password: {}\n  usage: render_page_png <pdf> <page> <scale> <out> <password>\n  or:    PDFX_PASSWORD=... render_page_png ...",
                    path
                );
                std::process::exit(2);
            }
            let ok = doc
                .authenticate_with_password(password.as_bytes())
                .expect("authenticate");
            if !ok {
                eprintln!("error: password rejected for encrypted PDF {}", path);
                std::process::exit(2);
            }
        } else if !password.is_empty() {
            // Already open with empty password; optional re-auth with provided pw.
            let _ = doc.authenticate_with_password(password.as_bytes());
        }
    }
    let (w, h, pixels) = match doc.render_page_to_image(page, Some(scale)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: render failed: {e}");
            std::process::exit(1);
        }
    };

    let mut buf = Vec::with_capacity(8 + pixels.len());
    buf.extend_from_slice(&w.to_le_bytes());
    buf.extend_from_slice(&h.to_le_bytes());
    buf.extend_from_slice(&pixels);

    if out == "-" {
        std::io::stdout().write_all(&buf).expect("write");
    } else {
        fs::write(out, &buf).expect("write");
    }
}
