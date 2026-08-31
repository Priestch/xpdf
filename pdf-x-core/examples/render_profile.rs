use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::RenderToImage;
use std::env;
use std::time::Instant;

fn main() {
    let path = env::args().nth(1).expect("need pdf path");
    let page: usize = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let data = std::fs::read(&path).expect("read");
    let mut doc = PDFDocument::open(data).expect("open");

    let _ = doc.render_page_to_image(page, Some(1.0));

    for &scale in &[0.5, 1.0, 1.5, 2.0] {
        let n = 10;
        let t = Instant::now();
        for _ in 0..n {
            let _ = doc.render_page_to_image(page, Some(scale));
        }
        let avg = t.elapsed().as_secs_f64() / n as f64 * 1000.0;
        println!("scale {:.1}: {:6.1} ms", scale, avg);
    }
}
