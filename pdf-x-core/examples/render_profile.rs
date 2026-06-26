use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::RenderToImage;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Count operations to understand what's happening during render.
fn main() {
    let path = env::args().nth(1).expect("need pdf path");
    let page: usize = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let data = std::fs::read(&path).expect("read");

    let mut doc = PDFDocument::open(data).expect("open");

    // Warm caches
    let _ = doc.render_page_to_image(page, Some(1.0));

    // Measure warm renders with different scales to isolate rasterization cost
    let scales = [0.5, 1.0, 1.5, 2.0];
    for &scale in &scales {
        let n = 5;
        let t = Instant::now();
        for _ in 0..n {
            let _ = doc.render_page_to_image(page, Some(scale));
        }
        let avg = t.elapsed().as_secs_f64() / n as f64 * 1000.0;
        let (w, h, _) = doc.render_page_to_image(page, Some(scale)).unwrap();
        println!(
            "scale {:.1}: {:6.1} ms  ({}x{} = {} px)",
            scale,
            avg,
            w,
            h,
            w as u64 * h as u64
        );
    }

    // Also measure: what fraction is resolve vs interpret+rasterize?
    let n = 10;
    let t = Instant::now();
    for _ in 0..n {
        let _ = doc.resolve_page(page);
    }
    let resolve = t.elapsed().as_secs_f64() / n as f64 * 1000.0;

    let t = Instant::now();
    for _ in 0..n {
        let _ = doc.render_page_to_image(page, Some(1.0));
    }
    let total = t.elapsed().as_secs_f64() / n as f64 * 1000.0;

    println!("\nresolve only:  {:.1} ms", resolve);
    println!("total render:  {:.1} ms", total);
    println!("interpret+raster: {:.1} ms", total - resolve);
}
