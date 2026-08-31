//! Differential: our extract_text_from_page_as_string vs `pdftotext`.
use pdf_x_core::core::PDFDocument;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let custom = base.join("hayro/hayro-tests/pdfs/custom");
    let targets = [
        "font_truetype_1.pdf",
        "font_cid_1.pdf",
        "arabiccidtruetype-pdf.pdf",
        "fireworks.pdf",
    ];
    for t in &targets {
        let p = custom.join(t);
        if !p.exists() {
            eprintln!("{t}: missing");
            continue;
        }
        let ref_text = Command::new("pdftotext")
            .arg("-f")
            .arg("1")
            .arg("-l")
            .arg("1")
            .arg(&p)
            .arg("-")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_else(|| "<pdftotext failed>".into());
        let ours = match PDFDocument::open(std::fs::read(&p).unwrap()) {
            Ok(mut d) => d
                .extract_text_from_page_as_string(0)
                .unwrap_or_else(|e| format!("<err {e}>")),
            Err(e) => format!("<open err {e}>"),
        };
        println!("=== {t} ===");
        println!(
            "ref : {}",
            ref_text
                .chars()
                .take(160)
                .collect::<String>()
                .replace('\n', "\\n")
        );
        println!(
            "ours: {}",
            ours.chars()
                .take(160)
                .collect::<String>()
                .replace('\n', "\\n")
        );
        let r: Vec<char> = ref_text.chars().filter(|c| !c.is_whitespace()).collect();
        let o: Vec<char> = ours.chars().filter(|c| !c.is_whitespace()).collect();
        println!("nwchars: ref={} ours={}", r.len(), o.len());
    }
}
