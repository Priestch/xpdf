use pdf_x_core::core::PDFDocument;
use pdf_x_device_tinyskia::RenderToImage;
use std::env;
use std::fs;
use std::io::Write;

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = &args[1];
    let page: usize = args[2].parse().expect("page number");
    let scale: f32 = args[3].parse().unwrap_or(1.0);
    let out = &args[4];

    let data = fs::read(path).expect("read pdf");
    let mut doc = PDFDocument::open(data).expect("open");
    let (w, h, pixels) = doc.render_page_to_image(page, Some(scale)).expect("render");

    let mut buf = Vec::with_capacity(8 + pixels.len());
    buf.extend_from_slice(&w.to_le_bytes());
    buf.extend_from_slice(&h.to_le_bytes());
    buf.extend_from_slice(&pixels);

    if out == "-" {
        std::io::stdout().write_all(&buf).expect("write");
    } else {
        fs::write(out, &buf).expect("write");
    }
}
