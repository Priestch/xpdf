//! Accuracy comparison: pdf-x extract_text vs pdftotext (Poppler) reference.
//! Computes char-level similarity after normalizing whitespace.
use pdf_x_core::core::PDFDocument;
use std::env;
use std::fs;
use std::process::Command;

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    // Simple LCS-based similarity
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut prev = vec![0usize; n + 1];
    let mut curr = vec![0usize; n + 1];
    for i in 1..=m {
        for j in 1..=n {
            curr[j] = if a[i - 1] == b[j - 1] {
                prev[j - 1] + 1
            } else {
                prev[j].max(curr[j - 1])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.iter_mut().for_each(|x| *x = 0);
    }
    let lcs = prev[n];
    2.0 * lcs as f64 / (m + n) as f64
}

fn extract_pdfx(path: &str) -> String {
    let data = fs::read(path).expect("read pdf");
    let mut doc = PDFDocument::open(data).expect("open");
    let pages = doc.page_count().expect("page count") as usize;
    let mut text = String::new();
    for i in 0..pages {
        if let Ok(s) = doc.extract_text_from_page_as_string(i) {
            text.push_str(&s);
            text.push('\n');
        }
    }
    text
}

fn extract_pdftotext(path: &str, layout: bool) -> String {
    let mut cmd = Command::new("pdftotext");
    if layout {
        cmd.arg("-layout");
    }
    cmd.arg(path).arg("-");
    cmd.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let pdfs: Vec<String> = if args.len() > 1 {
        args[1..].to_vec()
    } else {
        vec![
            "pdf.js/test/pdfs/tracemonkey.pdf".into(),
            "pdf.js/test/pdfs/freeculture.pdf".into(),
        ]
    };

    println!(
        "{:<30} {:>8} {:>8} {:>8} {:>8}",
        "File", "Pages", "Raw%", "Layout%", "NWhchars"
    );
    println!("{}", "-".repeat(70));

    for pdf in &pdfs {
        let basename = std::path::Path::new(pdf)
            .file_name()
            .unwrap()
            .to_string_lossy();
        let ours = extract_pdfx(pdf);
        let ref_raw = extract_pdftotext(pdf, false);
        let ref_layout = extract_pdftotext(pdf, true);

        let ours_norm = normalize(&ours);
        let ref_raw_norm = normalize(&ref_raw);
        let ref_layout_norm = normalize(&ref_layout);

        let sim_raw = similarity(&ours_norm, &ref_raw_norm);
        let sim_layout = similarity(&ours_norm, &ref_layout_norm);

        let pages = PDFDocument::open(fs::read(pdf).unwrap())
            .ok()
            .and_then(|mut d| d.page_count().ok())
            .unwrap_or(0);

        println!(
            "{:<30} {:>8} {:>7.2}% {:>7.2}% {:>8}",
            basename,
            pages,
            sim_raw * 100.0,
            sim_layout * 100.0,
            ours_norm.len()
        );

        // Show first divergence for debugging
        if sim_raw < 0.99 {
            let ours_chars: Vec<char> = ours_norm.chars().collect();
            let ref_chars: Vec<char> = ref_raw_norm.chars().collect();
            let min_len = ours_chars.len().min(ref_chars.len());
            let mut first_diff = min_len;
            for i in 0..min_len {
                if ours_chars[i] != ref_chars[i] {
                    first_diff = i;
                    break;
                }
            }
            if first_diff < min_len {
                let start = first_diff.saturating_sub(30);
                let end = (first_diff + 30).min(min_len);
                eprintln!(
                    "  first diff at char {} (of {}/{}):",
                    first_diff,
                    ours_chars.len(),
                    ref_chars.len()
                );
                eprintln!(
                    "  ours[{}..{}]: ...{}...",
                    start,
                    end,
                    ours_chars[start..end].iter().collect::<String>()
                );
                eprintln!(
                    "  ref  [{}..{}]: ...{}...",
                    start,
                    end,
                    ref_chars[start..end].iter().collect::<String>()
                );
            } else if ours_chars.len() != ref_chars.len() {
                let short = ours_chars.len().min(ref_chars.len());
                let end = (short + 30).min(ours_chars.len().max(ref_chars.len()));
                eprintln!(
                    "  diverge at end: ours[{}..{}]={} ref[{}..{}]={}",
                    short,
                    end.min(ours_chars.len()),
                    ours_chars[short..end.min(ours_chars.len())]
                        .iter()
                        .collect::<String>(),
                    short,
                    end.min(ref_chars.len()),
                    ref_chars[short..end.min(ref_chars.len())]
                        .iter()
                        .collect::<String>()
                );
            }
        }
    }
}
