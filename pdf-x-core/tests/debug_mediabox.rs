//! Debug MediaBox and CropBox
use pdf_x_core::PDFDocument;

#[test]
fn test_debug_mediabox() {
    let pdf_path = "/home/gp/Books/1807.03341v2.pdf";

    let pdf_bytes = match std::fs::read(pdf_path) {
        Ok(b) => b,
        Err(_) => {
            println!("Skipping test - PDF not found");
            return;
        }
    };

    let mut doc = match pdf_x_core::PDFDocument::open(pdf_bytes) {
        Ok(d) => d,
        Err(e) => {
            println!("Failed to parse PDF: {:?}", e);
            return;
        }
    };

    for page_idx in 0..3 {
        let page = match doc.get_page(page_idx) {
            Ok(p) => p,
            Err(_) => continue,
        };

        println!("Page {}:", page_idx);

        // MediaBox
        if let Some(pdf_x_core::PDFObject::Array(arr)) = page.media_box() {
            let get_value = |i: usize| -> f64 {
                match &**&arr[i] {
                    pdf_x_core::PDFObject::Number(n) => *n,
                    _ => 0.0,
                }
            };
            let x0 = get_value(0);
            let y0 = get_value(1);
            let x1 = get_value(2);
            let y1 = get_value(3);
            let width = x1 - x0;
            let height = y1 - y0;
            println!(
                "  MediaBox: [{}, {}, {}, {}] ({}x{})",
                x0, y0, x1, y1, width, height
            );
        }

        // CropBox
        if let Some(cropbox) = page.get("CropBox") {
            if let pdf_x_core::PDFObject::Array(arr) = cropbox {
                let get_value = |i: usize| -> f64 {
                    match &**&arr[i] {
                        pdf_x_core::PDFObject::Number(n) => *n,
                        _ => 0.0,
                    }
                };
                let x0 = get_value(0);
                let y0 = get_value(1);
                let x1 = get_value(2);
                let y1 = get_value(3);
                let width = x1 - x0;
                let height = y1 - y0;
                println!(
                    "  CropBox: [{}, {}, {}, {}] ({}x{})",
                    x0, y0, x1, y1, width, height
                );
            }
        } else {
            println!("  CropBox: not set (uses MediaBox)");
        }

        // BleedBox
        if let Some(bleedbox) = page.get("BleedBox") {
            if let pdf_x_core::PDFObject::Array(arr) = bleedbox {
                let get_value = |i: usize| -> f64 {
                    match &**&arr[i] {
                        pdf_x_core::PDFObject::Number(n) => *n,
                        _ => 0.0,
                    }
                };
                let x0 = get_value(0);
                let y0 = get_value(1);
                let x1 = get_value(2);
                let y1 = get_value(3);
                let width = x1 - x0;
                let height = y1 - y0;
                println!(
                    "  BleedBox: [{}, {}, {}, {}] ({}x{})",
                    x0, y0, x1, y1, width, height
                );
            }
        }

        // TrimBox
        if let Some(trimbox) = page.get("TrimBox") {
            if let pdf_x_core::PDFObject::Array(arr) = trimbox {
                let get_value = |i: usize| -> f64 {
                    match &**&arr[i] {
                        pdf_x_core::PDFObject::Number(n) => *n,
                        _ => 0.0,
                    }
                };
                let x0 = get_value(0);
                let y0 = get_value(1);
                let x1 = get_value(2);
                let y1 = get_value(3);
                let width = x1 - x0;
                let height = y1 - y0;
                println!(
                    "  TrimBox: [{}, {}, {}, {}] ({}x{})",
                    x0, y0, x1, y1, width, height
                );
            }
        }

        println!();
    }
}
