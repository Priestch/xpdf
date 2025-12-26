/// Simple text extraction example using extract_text_as_string().

use pdf_x::PDFDocument;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Simple Text Extraction");
        eprintln!("Usage: {} <pdf-file> [page-number]", args[0]);
        std::process::exit(1);
    }

    let pdf_path = &args[1];
    let page_num = if args.len() > 2 {
        args[2].parse::<usize>().unwrap_or(0)
    } else {
        0
    };

    println!("Extracting text from {} (page {})...\n", pdf_path, page_num + 1);

    let mut doc = match PDFDocument::open_file(pdf_path, None, None) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("Error: {:?}", e);
            std::process::exit(1);
        }
    };

    let page = match doc.get_page(page_num) {
        Ok(page) => page,
        Err(e) => {
            eprintln!("Error getting page: {:?}", e);
            std::process::exit(1);
        }
    };

    match page.extract_text_as_string(doc.xref_mut()) {
        Ok(text) => {
            if text.is_empty() {
                println!("(No text found on this page)");
            } else {
                println!("{}", text);
            }
        }
        Err(e) => {
            eprintln!("Error extracting text: {:?}", e);
            std::process::exit(1);
        }
    }
}
