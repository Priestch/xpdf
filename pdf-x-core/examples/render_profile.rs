use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::RenderToImage;
use std::env;
use std::time::Instant;

fn main() {
    let path = env::args().nth(1).expect("need pdf path");
    let page: usize = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let data = std::fs::read(&path).expect("read");

    let mut doc = PDFDocument::open(data.clone()).expect("open");

    let n = 10;
    let times: Vec<f64> = (0..n)
        .map(|_| {
            let t = Instant::now();
            let _ = doc.render_page_to_image(page, Some(1.0));
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();

    for (i, t) in times.iter().enumerate() {
        println!("render {}: {:.1} ms", i, t);
    }
    let warm: f64 = times[1..].iter().sum::<f64>() / (n - 1) as f64;
    println!("warm avg: {:.1} ms", warm);
}
