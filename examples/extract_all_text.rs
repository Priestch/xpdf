use pdf_x::PDFDocument;
use std::env;
use std::fs;

fn main() {
    let path = env::args().nth(1).expect("need pdf path");
    let out = env::args().nth(2).expect("need output path");
    let mut doc = PDFDocument::open_file(&path, None, None).expect("open");
    let pages = doc.page_count();
    let mut text = String::new();
    for i in 0..pages {
        let s = doc.extract_text_from_page_as_string(i).expect("extract");
        text.push_str(&s);
        text.push('\n');
    }
    fs::write(&out, &text).expect("write");
    eprintln!("Wrote {} chars from {} pages", text.len(), pages);
}
