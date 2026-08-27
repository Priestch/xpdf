//! Unit tests for the rendering context subsystems.

use super::*;
use crate::core::parser::PDFObject;
use crate::core::parser::Ref;
use crate::rendering::device::TestDevice;
use smallvec::smallvec;
use std::sync::Arc;

struct TestResolver {
    objects: HashMap<(u32, u32), Arc<PDFObject>>,
}

impl TestResolver {
    fn new(objects: HashMap<(u32, u32), PDFObject>) -> Self {
        Self {
            objects: objects
                .into_iter()
                .map(|(key, value)| (key, Arc::new(value)))
                .collect(),
        }
    }
}

impl PageResolver for TestResolver {
    fn resolve_if_ref(&mut self, obj: &PDFObject) -> PDFResult<Arc<PDFObject>> {
        match obj {
            PDFObject::Ref(r) => self.resolve_ref(r.num, r.generation),
            _ => Ok(Arc::new(obj.clone())),
        }
    }

    fn resolve_ref(&mut self, num: u32, generation: u32) -> PDFResult<Arc<PDFObject>> {
        self.objects
            .get(&(num, generation))
            .cloned()
            .ok_or_else(|| PDFError::Generic(format!("missing test object {num} {generation}")))
    }
}

#[test]
fn test_context_creation() {
    let mut device = TestDevice::new(612.0, 792.0);
    let ctx = RenderingContext::new(&mut device);
    assert_eq!(ctx.state_stack.len(), 1);
    assert!(ctx.current_path.is_empty());
}

#[test]
fn test_save_restore() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.current_state_mut().stroke_color = Color::red();
    ctx.save().unwrap();

    ctx.current_state_mut().stroke_color = Color::blue();
    assert_eq!(ctx.current_state().stroke_color, Color::RGB(0.0, 0.0, 1.0));

    ctx.restore().unwrap();
    assert_eq!(ctx.current_state().stroke_color, Color::RGB(1.0, 0.0, 0.0));
}

#[test]
fn separation_tint_resolves_indirect_icc_alternate_profile() {
    let mut objects = HashMap::new();
    objects.insert(
        (15, 0),
        PDFObject::Stream {
            dict: HashMap::from([("N".to_string(), PDFObject::Number(3.0))]),
            data: Vec::new(),
        },
    );
    let mut resolver_impl = TestResolver::new(objects);
    let mut resolver: Option<&mut dyn PageResolver> = Some(&mut resolver_impl);

    let color_space = PDFObject::Array(smallvec![
        Box::new(PDFObject::Name("Separation".to_string())),
        Box::new(PDFObject::Name("Spot".to_string())),
        Box::new(PDFObject::Array(smallvec![
            Box::new(PDFObject::Name("ICCBased".to_string())),
            Box::new(PDFObject::Ref(Ref {
                num: 15,
                generation: 0,
            })),
        ])),
        Box::new(PDFObject::Dictionary(HashMap::from([
            ("FunctionType".to_string(), PDFObject::Number(2.0)),
            (
                "C0".to_string(),
                PDFObject::Array(smallvec![
                    Box::new(PDFObject::Number(0.0)),
                    Box::new(PDFObject::Number(0.2)),
                    Box::new(PDFObject::Number(0.4)),
                ]),
            ),
            (
                "C1".to_string(),
                PDFObject::Array(smallvec![
                    Box::new(PDFObject::Number(0.5)),
                    Box::new(PDFObject::Number(0.6)),
                    Box::new(PDFObject::Number(0.7)),
                ]),
            ),
            ("N".to_string(), PDFObject::Number(1.0)),
        ]))),
    ]);

    let color = color_from_color_space(&color_space, &[PDFObject::Number(1.0)], &mut resolver);

    assert_eq!(color, Some(Color::RGB(0.5, 0.6, 0.7)));
}

#[test]
fn color_space_initial_color_matches_pdf_defaults() {
    let mut resolver = None;

    assert_eq!(
        initial_color_for_color_space(
            Some(PDFObject::Name("DeviceGray".to_string())),
            &mut resolver
        ),
        Some(Color::Gray(0.0))
    );
    assert_eq!(
        initial_color_for_color_space(
            Some(PDFObject::Name("DeviceRGB".to_string())),
            &mut resolver
        ),
        Some(Color::RGB(0.0, 0.0, 0.0))
    );
    assert_eq!(
        initial_color_for_color_space(
            Some(PDFObject::Name("DeviceCMYK".to_string())),
            &mut resolver
        ),
        Some(Color::CMYK(0.0, 0.0, 0.0, 1.0))
    );
}

#[test]
fn type3_differences_preserve_base_encoding_names() {
    let encoding = PDFObject::Dictionary(HashMap::from([
        (
            "BaseEncoding".to_string(),
            PDFObject::Name("WinAnsiEncoding".to_string()),
        ),
        (
            "Differences".to_string(),
            PDFObject::Array(smallvec![
                Box::new(PDFObject::Number(65.0)),
                Box::new(PDFObject::Name("CustomA".to_string())),
            ]),
        ),
    ]));

    let names = type3_difference_names(&encoding).expect("differences parsed");

    assert_eq!(names[65], Some("CustomA".to_string()));
    assert_eq!(names[66], Some("B".to_string()));
    assert_eq!(names[b' ' as usize], Some("space".to_string()));
}

#[test]
fn indexed_4bpc_expansion_resets_at_row_boundaries() {
    let lut = [
        0, 0, 0, // 0
        10, 0, 0, // 1
        20, 0, 0, // 2
        30, 0, 0, // 3
        40, 0, 0, // 4
        50, 0, 0, // 5
        60, 0, 0, // 6
    ];
    let data = [0x12, 0x30, 0x45, 0x60];

    let expanded = expand_indexed(&data, &lut, 3, 4, 3, 2, 6);

    assert_eq!(
        expanded,
        vec![10, 0, 0, 20, 0, 0, 30, 0, 0, 40, 0, 0, 50, 0, 0, 60, 0, 0,]
    );
}

#[test]
fn test_move_to_operator() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    let op = Operation::new(
        OpCode::MoveTo,
        vec![PDFObject::Number(10.0), PDFObject::Number(20.0)],
    );

    ctx.process_operation(&op).unwrap();
    assert_eq!(ctx.current_path.current_point(), Some((10.0, 20.0)));
}

#[test]
fn test_show_text_advances_text_matrix_only() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().text_matrix = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
    ctx.current_state_mut().text_line_matrix = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];

    let op = Operation::new(OpCode::ShowText, vec![PDFObject::String(vec![b'A', b'B'])]);
    ctx.process_operation(&op).unwrap();

    assert_eq!(
        ctx.current_state().text_matrix,
        [1.0, 0.0, 0.0, 1.0, 22.0, 20.0]
    );
    assert_eq!(
        ctx.current_state().text_line_matrix,
        [1.0, 0.0, 0.0, 1.0, 10.0, 20.0]
    );
}

#[test]
fn test_show_text_hex_string_advances_text_matrix_only() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().text_matrix = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
    ctx.current_state_mut().text_line_matrix = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];

    let op = Operation::new(
        OpCode::ShowText,
        vec![PDFObject::HexString(vec![b'A', b'B'])],
    );
    ctx.process_operation(&op).unwrap();

    assert_eq!(
        ctx.current_state().text_matrix,
        [1.0, 0.0, 0.0, 1.0, 22.0, 20.0]
    );
    assert_eq!(
        ctx.current_state().text_line_matrix,
        [1.0, 0.0, 0.0, 1.0, 10.0, 20.0]
    );
}

#[test]
fn test_show_text_respects_rotated_text_matrix() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().text_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];
    ctx.current_state_mut().text_line_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];

    let op = Operation::new(OpCode::ShowText, vec![PDFObject::String(vec![b'A'])]);
    ctx.process_operation(&op).unwrap();

    assert_eq!(
        ctx.current_state().text_matrix,
        [0.0, 1.0, -1.0, 0.0, 10.0, 26.0]
    );
}

#[test]
fn test_show_text_applies_char_and_word_spacing() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().font_size = Some(10.0);
    ctx.current_state_mut().character_spacing = 0.5;
    ctx.current_state_mut().word_spacing = 1.0;

    let op = Operation::new(
        OpCode::ShowText,
        vec![PDFObject::String(vec![b'A', b' ', b'B'])],
    );
    ctx.process_operation(&op).unwrap();

    // Width from TestDevice:
    // glyphs: (3 * 500 * 10) / 1000 = 15.0
    // spacing: (Tc * 3) + (Tw * 1) = 2.5
    // total: 17.5
    assert!((ctx.current_state().text_matrix[4] - 17.5).abs() < 1e-6);
    assert_eq!(ctx.current_state().text_line_matrix[4], 0.0);
}

#[test]
fn test_show_text_applies_hscale_to_char_and_word_spacing() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().font_size = Some(10.0);
    ctx.current_state_mut().character_spacing = 0.5;
    ctx.current_state_mut().word_spacing = 1.0;
    ctx.current_state_mut().text_horizontal_scaling = 50.0;

    let op = Operation::new(
        OpCode::ShowText,
        vec![PDFObject::String(vec![b'A', b' ', b'B'])],
    );
    ctx.process_operation(&op).unwrap();

    // Unscaled width: 17.5 (see test above), with 50% horizontal scale => 8.75.
    assert!((ctx.current_state().text_matrix[4] - 8.75).abs() < 1e-6);
}

#[test]
fn test_show_spaced_text_offset_sign_and_scaling() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().font_size = Some(10.0);
    ctx.current_state_mut().text_horizontal_scaling = 50.0;

    let op = Operation::new(
        OpCode::ShowSpacedText,
        vec![PDFObject::Array(smallvec![
            Box::new(PDFObject::String(vec![b'A'])),
            Box::new(PDFObject::Number(120.0))
        ])],
    );

    ctx.process_operation(&op).unwrap();

    // Width from TestDevice draw_text: ((1 * 500 * 10) / 1000) * 0.5 = 2.5
    // TJ adjustment: -(120 * 10 * 0.5) / 1000 = -0.6
    assert!((ctx.current_state().text_matrix[4] - 1.9).abs() < 1e-6);
    assert_eq!(ctx.current_state().text_line_matrix[4], 0.0);
}

#[test]
fn test_show_spaced_text_vertical_tj_adjusts_ty_without_hscale() {
    // Identity-V: TJ numbers advance along ty and ignore Tz.
    // Source: hayro TextState::apply_adjustment
    let mut device = TestDevice::new(612.0, 792.0);
    device.set_vertical_writing(true);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().font_name = Some("F1".to_string());
    ctx.current_state_mut().font_size = Some(10.0);
    // Tz must not affect vertical TJ adjustments.
    ctx.current_state_mut().text_horizontal_scaling = 50.0;

    let op = Operation::new(
        OpCode::ShowSpacedText,
        vec![PDFObject::Array(smallvec![Box::new(PDFObject::Number(
            120.0
        ))])],
    );
    ctx.process_operation(&op).unwrap();

    // adjust = -(120 * 10 * 1.0) / 1000 = -1.2 on ty, tx unchanged.
    assert!((ctx.current_state().text_matrix[4]).abs() < 1e-6);
    assert!((ctx.current_state().text_matrix[5] - (-1.2)).abs() < 1e-6);
    assert_eq!(ctx.current_state().text_line_matrix[5], 0.0);
}

#[test]
fn test_show_spaced_text_applies_char_word_spacing_and_tj_adjustment() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().font_size = Some(10.0);
    ctx.current_state_mut().character_spacing = 0.5;
    ctx.current_state_mut().word_spacing = 1.0;

    let op = Operation::new(
        OpCode::ShowSpacedText,
        vec![PDFObject::Array(smallvec![
            Box::new(PDFObject::String(vec![b'A', b' ', b'B'])),
            Box::new(PDFObject::Number(100.0))
        ])],
    );
    ctx.process_operation(&op).unwrap();

    // String width: 17.5 (with Tc/Tw), TJ adjustment: -(100 * 10 * 1.0) / 1000 = -1.0
    assert!((ctx.current_state().text_matrix[4] - 16.5).abs() < 1e-6);
}

#[test]
fn test_show_spaced_text_hex_string_offset_sign_and_scaling() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().font_size = Some(10.0);
    ctx.current_state_mut().text_horizontal_scaling = 50.0;

    let op = Operation::new(
        OpCode::ShowSpacedText,
        vec![PDFObject::Array(smallvec![
            Box::new(PDFObject::HexString(vec![b'A'])),
            Box::new(PDFObject::Number(120.0))
        ])],
    );

    ctx.process_operation(&op).unwrap();

    // Width from TestDevice draw_text: ((1 * 500 * 10) / 1000) * 0.5 = 2.5
    // TJ adjustment: -(120 * 10 * 0.5) / 1000 = -0.6
    assert!((ctx.current_state().text_matrix[4] - 1.9).abs() < 1e-6);
    assert_eq!(ctx.current_state().text_line_matrix[4], 0.0);
}

#[test]
fn test_move_text_post_concats_translation() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().text_line_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];
    ctx.current_state_mut().text_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];

    let op = Operation::new(
        OpCode::MoveText,
        vec![PDFObject::Number(5.0), PDFObject::Number(3.0)],
    );
    ctx.process_operation(&op).unwrap();

    assert_eq!(
        ctx.current_state().text_matrix,
        [0.0, 1.0, -1.0, 0.0, 7.0, 25.0]
    );
    assert_eq!(
        ctx.current_state().text_line_matrix,
        [0.0, 1.0, -1.0, 0.0, 7.0, 25.0]
    );
}

#[test]
fn test_next_line_post_concats_leading() {
    let mut device = TestDevice::new(612.0, 792.0);
    let mut ctx = RenderingContext::new(&mut device);

    ctx.begin_text().unwrap();
    ctx.current_state_mut().text_line_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];
    ctx.current_state_mut().text_matrix = [0.0, 1.0, -1.0, 0.0, 10.0, 20.0];
    ctx.current_state_mut().text_leading = 4.0;

    ctx.next_line().unwrap();

    assert_eq!(
        ctx.current_state().text_matrix,
        [0.0, 1.0, -1.0, 0.0, 14.0, 20.0]
    );
    assert_eq!(
        ctx.current_state().text_line_matrix,
        [0.0, 1.0, -1.0, 0.0, 14.0, 20.0]
    );
}

#[test]
fn test_remap_text_bytes_with_cmap_shifted_single_byte_codes() {
    let cmap = CMap::parse(
        br#"
4 beginbfchar
<2A> <0047>
<44> <0061>
<50> <006D>
<48> <0065>
endbfchar
"#,
    )
    .unwrap();

    let remapped = remap_text_bytes_with_cmap(b"*DPH", &cmap).unwrap();
    assert_eq!(remapped, b"Game");
}

#[test]
fn test_remap_text_bytes_with_cmap_prefers_two_byte_codes() {
    let cmap = CMap::parse(
        br#"
2 beginbfchar
<0102> <0041>
<0304> <0042>
endbfchar
"#,
    )
    .unwrap();

    let remapped = remap_text_bytes_with_cmap(&[0x01, 0x02, 0x03, 0x04], &cmap).unwrap();
    assert_eq!(remapped, b"AB");
}

#[test]
fn test_remap_text_bytes_with_cmap_unchanged_returns_none() {
    let cmap = CMap::parse(
        br#"
1 beginbfchar
<41> <0041>
endbfchar
"#,
    )
    .unwrap();

    assert!(remap_text_bytes_with_cmap(b"A", &cmap).is_none());
}

#[test]
fn test_font_needs_to_unicode_remap_for_simple_type1_is_false() {
    let info = crate::core::font::FontDict {
        font_type: crate::core::font::FontType::Type1,
        base_font: "Test".to_string(),
        encoding: Some(PDFObject::Name("WinAnsiEncoding".to_string())),
        to_unicode: None,
        font_descriptor: None,
        widths: None,
        first_char: None,
        last_char: None,
        default_width: 250.0,
        descendant_fonts: None,
    };

    assert!(!font_needs_to_unicode_remap(&info));
}

#[test]
fn test_font_needs_to_unicode_remap_for_descendant_font_is_true() {
    let info = crate::core::font::FontDict {
        font_type: crate::core::font::FontType::Unknown,
        base_font: "Test".to_string(),
        encoding: None,
        to_unicode: None,
        font_descriptor: None,
        widths: None,
        first_char: None,
        last_char: None,
        default_width: 250.0,
        descendant_fonts: Some(PDFObject::Array(smallvec![])),
    };

    assert!(font_needs_to_unicode_remap(&info));
}

#[test]
fn test_parse_blend_mode_from_name() {
    let mode = parse_blend_mode(&PDFObject::Name("SoftLight".to_string()));
    assert_eq!(mode, Some(crate::rendering::BlendMode::SoftLight));
}

#[test]
fn test_parse_blend_mode_from_array() {
    let mode = parse_blend_mode(&PDFObject::Array(smallvec![
        Box::new(PDFObject::Name("Multiply".to_string())),
        Box::new(PDFObject::Name("Normal".to_string())),
    ]));
    assert_eq!(mode, Some(crate::rendering::BlendMode::Multiply));
}

#[test]
fn test_text_paint_for_invisible_mode_is_transparent() {
    let paint = text_paint_for_rendering_mode(
        crate::rendering::TextRenderingMode::Invisible,
        Color::RGB(0.0, 0.0, 0.0),
    );
    assert!(matches!(paint, Paint::Transparent));
}

#[test]
fn test_text_paint_for_fill_mode_uses_fill_color() {
    let paint = text_paint_for_rendering_mode(
        crate::rendering::TextRenderingMode::Fill,
        Color::RGB(1.0, 0.0, 0.0),
    );
    assert!(matches!(paint, Paint::Solid(Color::RGB(1.0, 0.0, 0.0))));
}

#[test]
fn test_pick_fallback_standard_font_respects_style() {
    // Unembedded Garamond faces must not all collapse to Times-Roman.
    assert_eq!(
        pick_fallback_standard_font_key("Garamond-Bold"),
        "Times-Bold"
    );
    assert_eq!(
        pick_fallback_standard_font_key("Garamond-LightItalic"),
        "Times-Italic"
    );
    assert_eq!(
        pick_fallback_standard_font_key("Garamond-Book"),
        "Times-Roman"
    );
    assert_eq!(
        pick_fallback_standard_font_key("MySans-BoldOblique"),
        "Helvetica-BoldOblique"
    );
    assert_eq!(
        pick_fallback_standard_font_key("CourierNewPS-BoldMT"),
        "Courier-Bold"
    );
}
