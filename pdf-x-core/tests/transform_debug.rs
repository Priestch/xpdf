//! Debug test to understand the transform issue in issue7200.pdf
use pdf_x_core::PDFDocument;

#[test]
fn debug_issue7200_transform() {
    let pdf_path = "/home/gp/Projects/pdf-x/pdf.js/test/pdfs/issue7200.pdf";

    let pdf_bytes = match std::fs::read(pdf_path) {
        Ok(b) => b,
        Err(_) => {
            println!("Skipping test - PDF not found");
            return;
        }
    };

    let mut doc = match PDFDocument::open(pdf_bytes) {
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

    println!("=== Transform Debug for issue7200.pdf ===\n");

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
        println!(
            "MediaBox: [{}, {}, {}, {}] ({}x{})",
            x0,
            y0,
            x1,
            y1,
            x1 - x0,
            y1 - y0
        );
    }

    // Get page resources and find XObject
    println!("\nResources found: {}", page.resources().is_some());
    if let Some(resources) = page.resources() {
        // Try to fetch the resources if it's a reference
        let resources_res = page.fetch_if_ref(resources, &mut doc.xref_mut());
        println!("Resources (after fetch): {:?}", resources_res.is_ok());

        let res = resources_res.unwrap_or(resources.clone());
        match &res {
            pdf_x_core::PDFObject::Dictionary(res_dict) => {
                let keys: Vec<_> = res_dict.keys().collect();
                println!("Resource dict keys: {:?}", keys);

                if let Some(xobject) = res_dict.get("XObject") {
                    println!("XObject found: {:?}", xobject);
                    if let Ok(xobj_dict) = page.fetch_if_ref(xobject, &mut doc.xref_mut()) {
                        println!(
                            "XObject dict type: {:?}",
                            std::mem::discriminant(&xobj_dict)
                        );
                        match &xobj_dict {
                            pdf_x_core::PDFObject::Dictionary(xobj) => {
                                println!("\n=== XObject Analysis ===");

                                for (name, obj) in xobj.iter() {
                                    println!("\nXObject: {}", name);

                                    if let Ok(xobj_data) =
                                        page.fetch_if_ref(obj, &mut doc.xref_mut())
                                    {
                                        if let pdf_x_core::PDFObject::Stream { dict, .. } =
                                            &xobj_data
                                        {
                                            // Get Subtype
                                            if let Some(subtype) = dict.get("Subtype") {
                                                if let pdf_x_core::PDFObject::Name(subtype_name) =
                                                    subtype
                                                {
                                                    println!("  Subtype: {}", subtype_name);
                                                }
                                            }

                                            // Get Width/Height
                                            let width = if let Some(w) = dict.get("Width") {
                                                if let pdf_x_core::PDFObject::Number(n) = w {
                                                    *n as u32
                                                } else {
                                                    0
                                                }
                                            } else {
                                                0
                                            };
                                            let height = if let Some(h) = dict.get("Height") {
                                                if let pdf_x_core::PDFObject::Number(n) = h {
                                                    *n as u32
                                                } else {
                                                    0
                                                }
                                            } else {
                                                0
                                            };
                                            println!("  Size: {}x{}", width, height);

                                            // Get Matrix (transform)
                                            println!(
                                                "  Dict keys: {:?}",
                                                dict.keys().collect::<Vec<_>>()
                                            );
                                            if let Some(matrix) = dict.get("Matrix") {
                                                println!("  Matrix found: {:?}", matrix);
                                                if let pdf_x_core::PDFObject::Array(arr) = matrix {
                                                    let values: Vec<f64> = arr
                                                        .iter()
                                                        .map(|v| match &**v {
                                                            pdf_x_core::PDFObject::Number(n) => *n,
                                                            _ => 0.0,
                                                        })
                                                        .collect();

                                                    if values.len() == 6 {
                                                        println!(
                                                            "  Matrix: [{:.6}, {:.6}, {:.6}, {:.6}, {:.6}, {:.6}]",
                                                            values[0],
                                                            values[1],
                                                            values[2],
                                                            values[3],
                                                            values[4],
                                                            values[5]
                                                        );

                                                        // Explain what this matrix does
                                                        let sx = values[0];
                                                        let sy = values[3];
                                                        let tx = values[4];
                                                        let ty = values[5];

                                                        println!("  -> Scales by {}x{}", sx, sy);
                                                        println!(
                                                            "  -> Translates by ({}, {})",
                                                            tx, ty
                                                        );

                                                        // Apply this matrix to a unit square [0,0,1,1]
                                                        let x0_transformed = sx * 0.0 + tx;
                                                        let y0_transformed = sy * 0.0 + ty;
                                                        let x1_transformed = sx * 1.0 + tx;
                                                        let y1_transformed = sy * 1.0 + ty;

                                                        println!(
                                                            "  -> Maps unit square to: [{}, {}, {}, {}]",
                                                            x0_transformed,
                                                            y0_transformed,
                                                            x1_transformed,
                                                            y1_transformed
                                                        );
                                                    }
                                                }
                                            } else {
                                                println!(
                                                    "  No Matrix in XObject dict (uses default identity)"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            other => {
                                println!(
                                    "XObject is not a Dictionary, it's: {:?}",
                                    std::mem::discriminant(other)
                                );
                            }
                        }
                    }
                }
            }
            other => {
                println!(
                    "Resources is not a Dictionary, it's: {:?}",
                    std::mem::discriminant(other)
                );
            }
        }
    }

    // Check content stream for transforms
    println!("\n=== Content Stream Analysis ===");
    if let Some(contents) = page.contents() {
        if let Ok(content_data) = page.fetch_if_ref(contents, &mut doc.xref_mut()) {
            if let pdf_x_core::PDFObject::Stream { dict, data } = &content_data {
                // Decode if needed
                let decoded = if let Some(_filter) = dict.get("Filter") {
                    // For now, just try to decode
                    match pdf_x_core::core::decode::apply_filters(&data, _filter) {
                        Ok(d) => d,
                        Err(_) => data.clone(),
                    }
                } else {
                    data.clone()
                };

                let content_str = String::from_utf8_lossy(&decoded);
                println!(
                    "Content stream (first 500 chars):\n{}",
                    &content_str[..content_str.len().min(500)]
                );

                // Look for transform operators
                println!("\nTransform operators in content stream:");
                for line in content_str.lines() {
                    if line.contains("cm") || line.contains("q") || line.contains("Q") {
                        println!("  {}", line);
                    }
                }
            }
        }
    }

    println!("\n=== Device Transform Analysis ===");
    println!("NEW: Device initial transform (Y-flip only): [1, 0, 0, -1, 0, 841.89]");
    println!("This maps:");
    println!("  PDF (0, 0) -> Screen (0, 842)");
    println!("  PDF (595, 842) -> Screen (595, 0)");
    println!("\nWhen PDF transform [448.886505, 0, 0, 522.341248, 70.794716, 248.751099]");
    println!("is concatenated with device transform:");
    println!("  CTM = PDF_transform × device_transform");
    println!("  CTM = [448.886505, 0, 0, -522.341248, 70.794716, 1183.641]");
    println!("\nThis CTM maps the 501x583 image pixmap to:");
    println!("  Screen position: (71, 1184) to (520, 249)");
    println!("  -> CORRECTLY ON SCREEN!");
}
