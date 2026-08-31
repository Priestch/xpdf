use pdf_x_core::core::PDFDocument;
use std::env;
use std::fs;

fn main() {
    let path = env::args().nth(1).expect("need pdf path");
    let page_num: usize = env::args()
        .nth(2)
        .expect("need page num")
        .parse()
        .expect("number");
    let data = fs::read(&path).expect("read");
    let mut doc = PDFDocument::open(data).expect("open");
    let page = doc.get_page(page_num).expect("get_page");
    let items = page.extract_text(doc.doc_store_mut()).expect("extract");
    for item in &items {
        let pos = item
            .position
            .map(|p| format!("({:.0},{:.0})", p.0, p.1))
            .unwrap_or_default();
        let fs = item
            .font_size
            .map(|f| format!("{:.1}", f))
            .unwrap_or_default();
        println!(
            "{:>16} {:>8} {:>10} {}",
            pos,
            fs,
            item.text.len(),
            item.text
                .replace('\n', "\\n")
                .chars()
                .take(80)
                .collect::<String>()
        );
    }
}
