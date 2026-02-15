//! Deep investigation of issue7200.pdf coordinate transforms
use pdf_x_core::PDFDocument;

#[test]
fn investigate_issue7200_transforms() {
    let pdf_path = "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf";

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

    let page = match doc.get_page(0) {
        Ok(p) => p,
        Err(_) => return,
    };

    println!("=== Page 0 Analysis ===\n");

    // MediaBox
    if let Some(pdf_x_core::PDFObject::Array(arr)) = page.media_box() {
        println!(
            "MediaBox: {:?}",
            arr.iter()
                .map(|o| {
                    match &**o {
                        pdf_x_core::PDFObject::Number(n) => format!("{:.2}", n),
                        _ => "?".to_string(),
                    }
                })
                .collect::<Vec<_>>()
        );
    }

    // Resources
    if let Some(resources) = page.resources() {
        println!("\nResources found");

        // Check for XObjects
        if let pdf_x_core::PDFObject::Dictionary(res_dict) = resources {
            if let Some(xobject) = res_dict.get("XObject") {
                println!("XObject present: {:?}", xobject);

                // Fetch the XObject dictionary
                if let Ok(xobj_dict) = page.fetch_if_ref(xobject, &mut doc.xref_mut()) {
                    if let pdf_x_core::PDFObject::Dictionary(xobj) = &xobj_dict {
                        println!("\nXObject entries:");
                        for (name, obj) in xobj.iter().take(5) {
                            println!("  {} -> {:?}", name, obj);

                            // Try to fetch the XObject
                            if let Ok(xobj_data) = page.fetch_if_ref(obj, &mut doc.xref_mut()) {
                                if let pdf_x_core::PDFObject::Stream { dict, .. } = &xobj_data {
                                    if let Some(subtype) = dict.get("Subtype") {
                                        println!("    Subtype: {:?}", subtype);
                                    }
                                    if let Some(bbox) = dict.get("BBox") {
                                        println!("    BBox: {:?}", bbox);
                                    }
                                    if let Some(matrix) = dict.get("Matrix") {
                                        println!("    Matrix: {:?}", matrix);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Print content stream (first 500 bytes)
    if let Some(contents) = page.contents() {
        if let Ok(content_data) = page.fetch_if_ref(contents, &mut doc.xref_mut()) {
            if let pdf_x_core::PDFObject::Stream { dict, data } = &content_data {
                println!(
                    "\n=== Content Stream (first {} bytes) ===",
                    data.len().min(500)
                );

                // Decode if needed
                let decoded = if let Some(filter) = dict.get("Filter") {
                    match pdf_x_core::core::decode::apply_filters(&data, filter) {
                        Ok(d) => d,
                        Err(_) => data.clone(),
                    }
                } else {
                    data.clone()
                };

                let preview = String::from_utf8_lossy(&decoded[..decoded.len().min(500)]);
                println!("{}", preview);

                // Look for transform operators
                println!("\n=== Transform Operators ===");
                let content_str = String::from_utf8_lossy(&decoded);
                for line in content_str.lines() {
                    if line.contains("cm") || line.contains("q") || line.contains("Q") {
                        println!("{}", line);
                    }
                }
            }
        }
    }
}
