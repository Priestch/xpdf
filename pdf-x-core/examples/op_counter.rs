use pdf_x_core::core::PDFDocument;
use pdf_x_core::core::base_stream::BaseStream;
use pdf_x_core::core::content_stream::ContentStreamEvaluator;
use pdf_x_core::core::lexer::Lexer;
use pdf_x_core::core::parser::Parser;
use pdf_x_core::core::stream::Stream;
use std::env;

fn main() {
    let path = env::args().nth(1).expect("need pdf path");
    let page: usize = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let data = std::fs::read(&path).expect("read");
    let mut doc = PDFDocument::open(data).expect("open");
    let resolved = doc.resolve_page(page).expect("resolve");

    let stream = Box::new(Stream::from_bytes(resolved.contents().to_vec())) as Box<dyn BaseStream>;
    let lexer = Lexer::new(stream).expect("lexer");
    let parser = Parser::new(lexer).expect("parser");
    let mut evaluator = ContentStreamEvaluator::new(parser);

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut total = 0;
    loop {
        match evaluator.read_operation() {
            Ok(Some(op)) => {
                let name = format!("{:?}", op.op);
                *counts.entry(name).or_default() += 1;
                total += 1;
            }
            _ => break,
        }
    }

    println!("Total operators: {total}");
    println!("\nTop operators:");
    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (op, count) in sorted.into_iter().take(20) {
        println!("  {:>6}  {}", count, op);
    }
}
