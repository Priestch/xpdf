use pdf_x_core::core::PDFDocument;
use std::env;
use std::fs;
use std::io::Write;

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = &args[1];
    let data = fs::read(path).expect("read pdf");
    let mut doc = PDFDocument::open(data).expect("open");
    let pages = doc.page_count().expect("page count") as usize;

    // Mode: "all" or "page N"
    if args.len() > 2 && args[2].starts_with("page=") {
        let page: usize = args[2][5..].parse().expect("page number");
        let s = doc.extract_text_from_page_as_string(page).expect("extract");
        std::io::stdout().write_all(s.as_bytes()).expect("write");
    } else {
        let mut text = String::new();
        for i in 0..pages {
            if let Ok(s) = doc.extract_text_from_page_as_string(i) {
                text.push_str(&s);
                text.push('\n');
            }
        }
        std::io::stdout().write_all(text.as_bytes()).expect("write");
    }
}
