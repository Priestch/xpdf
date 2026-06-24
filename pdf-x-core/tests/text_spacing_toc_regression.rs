use pdf_x_core::rendering::TestDevice;
use pdf_x_core::PDFDocument;

fn push_object(pdf: &mut Vec<u8>, offsets: &mut Vec<usize>, number: u32, body: &[u8]) {
    offsets.push(pdf.len());
    pdf.extend_from_slice(format!("{} 0 obj\n", number).as_bytes());
    pdf.extend_from_slice(body);
    pdf.extend_from_slice(b"\nendobj\n");
}

fn build_toc_spacing_regression_pdf() -> Vec<u8> {
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");

    let mut offsets = Vec::new();

    push_object(
        &mut pdf,
        &mut offsets,
        1,
        br#"<< /Type /Catalog /Pages 2 0 R >>"#,
    );
    push_object(
        &mut pdf,
        &mut offsets,
        2,
        br#"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"#,
    );
    push_object(
        &mut pdf,
        &mut offsets,
        3,
        br#"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"#,
    );

    let stream_data = br#"BT
/F1 9.5 Tf
0 Tc
0.702 Tw
114 584.706 Td
[( 1 )44.6(Getting )673(Ready )-8(1)]TJ
0 Tw
0 -14 Td
[(Whence C? 1)]TJ
ET
"#;

    let mut stream_body = format!("<< /Length {} >>\nstream\n", stream_data.len()).into_bytes();
    stream_body.extend_from_slice(stream_data);
    stream_body.extend_from_slice(b"endstream");
    push_object(&mut pdf, &mut offsets, 4, &stream_body);

    push_object(
        &mut pdf,
        &mut offsets,
        5,
        br#"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"#,
    );

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            6, xref_offset
        )
        .as_bytes(),
    );

    pdf
}

fn extract_tw(op: &str) -> f64 {
    let tw = op
        .rsplit("tw=")
        .next()
        .expect("missing tw field")
        .trim_end_matches(')');
    tw.parse::<f64>().expect("invalid tw value")
}

#[test]
fn test_toc_style_word_spacing_flows_to_tj_text_runs() {
    let pdf_data = build_toc_spacing_regression_pdf();
    let mut doc = PDFDocument::open(pdf_data).expect("failed to parse test PDF");
    let page = doc.get_page(0).expect("failed to get page 0");
    let mut device = TestDevice::new(612.0, 792.0);

    page.render(&mut doc.xref_mut(), &mut device)
        .expect("failed to render page");

    let draw_ops: Vec<&str> = device
        .operations()
        .iter()
        .map(String::as_str)
        .filter(|op| op.starts_with("draw_text("))
        .collect();
    assert!(
        !draw_ops.is_empty(),
        "expected draw_text operations from spacing regression PDF"
    );

    let getting = draw_ops
        .iter()
        .copied()
        .find(|op| op.contains("[71, 101, 116, 116, 105, 110, 103, 32]"))
        .expect("expected draw_text call for 'Getting '");
    let ready = draw_ops
        .iter()
        .copied()
        .find(|op| op.contains("[82, 101, 97, 100, 121, 32]"))
        .expect("expected draw_text call for 'Ready '");
    let whence = draw_ops
        .iter()
        .copied()
        .find(|op| op.contains("[87, 104, 101, 110, 99, 101, 32, 67, 63, 32, 49]"))
        .expect("expected draw_text call for 'Whence C? 1'");

    assert!(
        (extract_tw(getting) - 0.702).abs() < 1e-6,
        "expected Tw=0.702 for 'Getting ': {getting}"
    );
    assert!(
        (extract_tw(ready) - 0.702).abs() < 1e-6,
        "expected Tw=0.702 for 'Ready ': {ready}"
    );
    assert!(
        extract_tw(whence).abs() < 1e-9,
        "expected Tw reset to 0 for next line: {whence}"
    );
}
