//! Recording device for rendering correctness testing.
//!
//! Records every device call with full parameters as structured, serializable data.
//! Output is deterministic (fixed-precision floats) and diffable — one line per call.
//!
//! Use with [`serialize()`][RecordingDevice::serialize] to produce a text snapshot
//! that can be committed and compared across code changes.

use std::fmt;

use super::device::{Device, ImageData, Paint, PathDrawMode};
use super::graphics_state::{BlendMode, Color, FillRule, StrokeProps, TextRenderingMode};
use super::shading::Shading;
use crate::core::error::PDFResult;
use crate::core::parser::PDFObject;
use crate::rendering::device::Type0GlyphRun;

// ── Deterministic formatting helpers ──────────────────────────────────────────

fn fmt_matrix(m: &[f64; 6]) -> String {
    format!(
        "[{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}]",
        m[0], m[1], m[2], m[3], m[4], m[5]
    )
}

fn fmt_paint(p: &Paint) -> String {
    match p {
        Paint::Solid(c) => fmt_color(c),
        Paint::Transparent => "Transparent".to_string(),
    }
}

fn fmt_color(c: &Color) -> String {
    match c {
        Color::Gray(g) => format!("Gray({:.6})", g),
        Color::RGB(r, g, b) => format!("RGB({:.6},{:.6},{:.6})", r, g, b),
        Color::CMYK(c, m, y, k) => format!("CMYK({:.6},{:.6},{:.6},{:.6})", c, m, y, k),
    }
}

fn fmt_stroke(s: &StrokeProps) -> String {
    let dash = format!(
        "[{}]",
        s.dash_array
            .iter()
            .map(|v| format!("{:.6}", v))
            .collect::<Vec<_>>()
            .join(",")
    );
    format!(
        "lw={:.6},cap={:?},join={:?},ml={:.6},dash={},doff={:.6}",
        s.line_width, s.line_cap, s.line_join, s.miter_limit, dash, s.dash_offset
    )
}

fn fmt_fill_rule(r: FillRule) -> String {
    match r {
        FillRule::NonZero => "NonZero".to_string(),
        FillRule::EvenOdd => "EvenOdd".to_string(),
    }
}

fn fmt_path_mode(m: PathDrawMode) -> String {
    match m {
        PathDrawMode::Fill(r) => format!("fill({})", fmt_fill_rule(r)),
        PathDrawMode::Stroke => "stroke".to_string(),
        PathDrawMode::FillStroke(r) => format!("fill_stroke({})", fmt_fill_rule(r)),
    }
}

fn fmt_blend(m: BlendMode) -> String {
    format!("{:?}", m)
}

// ── DeviceCall ────────────────────────────────────────────────────────────────

/// A single recorded device call with all its parameters.
#[derive(Debug, Clone)]
pub enum DeviceCall {
    // Path construction
    BeginPath,
    MoveTo(f64, f64),
    LineTo(f64, f64),
    CurveTo(f64, f64, f64, f64, f64, f64),
    Rect(f64, f64, f64, f64),
    ClosePath,

    // Path painting / clipping
    DrawPath {
        mode: String,
        paint: String,
        stroke: String,
    },
    ClipPath(String),
    ClipStrokePath,

    // State / transforms
    SaveState,
    RestoreState,
    ConcatMatrix([f64; 6]),
    SetMatrix([f64; 6]),
    SetBlendMode(String),
    SetOpacity(f64),

    // Text
    DrawText {
        font: String,
        size: f64,
        paint: String,
        text_matrix: [f64; 6],
        hscale: f64,
        rise: f64,
        cs: f64,
        ws: f64,
        byte_len: usize,
    },
    DrawType0GlyphRun {
        font: String,
        size: f64,
        paint: String,
        glyph_count: usize,
        vertical: bool,
        text_matrix: [f64; 6],
        hscale: f64,
        rise: f64,
    },
    ClipText {
        font: String,
        size: f64,
        byte_len: usize,
    },
    FlushTextClip,

    // Images
    DrawImage {
        w: u32,
        h: u32,
        transform: [f64; 6],
        bpc: u8,
        has_alpha: bool,
        is_mask: bool,
    },

    // Shading
    DrawShading {
        n_components: usize,
    },

    // Type3
    BeginType3Glyph,
    EndType3Glyph,

    // Offscreen / transparency
    BeginOffscreen,
    EndOffscreenAsMask {
        luminosity: bool,
    },
    EndOffscreenAsRgbaRegion {
        width: u32,
        height: u32,
    },
    EndOffscreenComposite {
        opacity: f64,
    },
    SetSoftMask {
        has_mask: bool,
    },

    // Patterns
    FillPathWithPatternCell {
        rule: String,
        rgba_len: usize,
        cell_w: u32,
        cell_h: u32,
        x_step: f64,
        y_step: f64,
    },

    // Font management
    LoadFontData {
        name: String,
        data_len: usize,
    },
    SetFontWidths {
        name: String,
        first_char: u32,
        count: usize,
    },
    SetFontRawCidCodes {
        name: String,
        enabled: bool,
    },
    SetFontCidVerticalMetrics {
        name: String,
        vertical: bool,
        widths2_count: usize,
    },
    SetFontCidToGidMap {
        name: String,
        map_len: usize,
    },
}

impl fmt::Display for DeviceCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceCall::BeginPath => write!(f, "begin_path"),
            DeviceCall::MoveTo(x, y) => write!(f, "move_to({:.6},{:.6})", x, y),
            DeviceCall::LineTo(x, y) => write!(f, "line_to({:.6},{:.6})", x, y),
            DeviceCall::CurveTo(cp1x, cp1y, cp2x, cp2y, x, y) => write!(
                f,
                "curve_to({:.6},{:.6},{:.6},{:.6},{:.6},{:.6})",
                cp1x, cp1y, cp2x, cp2y, x, y
            ),
            DeviceCall::Rect(x, y, w, h) => {
                write!(f, "rect({:.6},{:.6},{:.6},{:.6})", x, y, w, h)
            }
            DeviceCall::ClosePath => write!(f, "close_path"),
            DeviceCall::DrawPath { mode, paint, stroke } => {
                write!(f, "draw_path({}, paint={}, stroke={})", mode, paint, stroke)
            }
            DeviceCall::ClipPath(rule) => write!(f, "clip_path({})", rule),
            DeviceCall::ClipStrokePath => write!(f, "clip_stroke_path"),
            DeviceCall::SaveState => write!(f, "save_state"),
            DeviceCall::RestoreState => write!(f, "restore_state"),
            DeviceCall::ConcatMatrix(m) => write!(f, "concat_matrix({})", fmt_matrix(m)),
            DeviceCall::SetMatrix(m) => write!(f, "set_matrix({})", fmt_matrix(m)),
            DeviceCall::SetBlendMode(m) => write!(f, "set_blend_mode({})", m),
            DeviceCall::SetOpacity(o) => write!(f, "set_opacity({:.6})", o),
            DeviceCall::DrawText {
                font,
                size,
                paint,
                text_matrix,
                hscale,
                rise,
                cs,
                ws,
                byte_len,
            } => write!(
                f,
                "draw_text(font={}, size={:.6}, paint={}, tm={}, hscale={:.6}, rise={:.6}, cs={:.6}, ws={:.6}, bytes={})",
                font, size, paint, fmt_matrix(text_matrix), hscale, rise, cs, ws, byte_len
            ),
            DeviceCall::DrawType0GlyphRun {
                font,
                size,
                paint,
                glyph_count,
                vertical,
                text_matrix,
                hscale,
                rise,
            } => write!(
                f,
                "draw_type0_glyph_run(font={}, size={:.6}, paint={}, glyphs={}, vertical={}, tm={}, hscale={:.6}, rise={:.6})",
                font, size, paint, glyph_count, vertical, fmt_matrix(text_matrix), hscale, rise
            ),
            DeviceCall::ClipText { font, size, byte_len } => write!(
                f,
                "clip_text(font={}, size={:.6}, bytes={})",
                font, size, byte_len
            ),
            DeviceCall::FlushTextClip => write!(f, "flush_text_clip"),
            DeviceCall::DrawImage { w, h, transform, bpc, has_alpha, is_mask } => write!(
                f,
                "draw_image({}x{}, bpc={}, alpha={}, mask={}, tm={})",
                w, h, bpc, has_alpha, is_mask, fmt_matrix(transform)
            ),
            DeviceCall::DrawShading { n_components } => {
                write!(f, "draw_shading(n={})", n_components)
            }
            DeviceCall::BeginType3Glyph => write!(f, "begin_type3_glyph"),
            DeviceCall::EndType3Glyph => write!(f, "end_type3_glyph"),
            DeviceCall::BeginOffscreen => write!(f, "begin_offscreen"),
            DeviceCall::EndOffscreenAsMask { luminosity } => {
                write!(f, "end_offscreen_as_mask(luminosity={})", luminosity)
            }
            DeviceCall::EndOffscreenAsRgbaRegion { width, height } => {
                write!(f, "end_offscreen_as_rgba_region({}x{})", width, height)
            }
            DeviceCall::EndOffscreenComposite { opacity } => {
                write!(f, "end_offscreen_composite(opacity={:.6})", opacity)
            }
            DeviceCall::SetSoftMask { has_mask } => {
                write!(f, "set_soft_mask(has_mask={})", has_mask)
            }
            DeviceCall::FillPathWithPatternCell { rule, rgba_len, cell_w, cell_h, x_step, y_step } => {
                write!(
                    f,
                    "fill_path_with_pattern_cell(rule={}, rgba_len={}, cell={}x{}, step={:.6},{:.6})",
                    rule, rgba_len, cell_w, cell_h, x_step, y_step
                )
            }
            DeviceCall::LoadFontData { name, data_len } => {
                write!(f, "load_font_data(name={}, bytes={})", name, data_len)
            }
            DeviceCall::SetFontWidths { name, first_char, count } => {
                write!(f, "set_font_widths(name={}, first={}, count={})", name, first_char, count)
            }
            DeviceCall::SetFontRawCidCodes { name, enabled } => {
                write!(f, "set_font_raw_cid_codes(name={}, enabled={})", name, enabled)
            }
            DeviceCall::SetFontCidVerticalMetrics { name, vertical, widths2_count } => {
                write!(f, "set_font_cid_vertical_metrics(name={}, vertical={}, w2_count={})", name, vertical, widths2_count)
            }
            DeviceCall::SetFontCidToGidMap { name, map_len } => {
                write!(f, "set_font_cid_to_gid_map(name={}, len={})", name, map_len)
            }
        }
    }
}

// ── RecordingDevice ──────────────────────────────────────────────────────────

/// A [`Device`] implementation that records every call with full parameters.
///
/// Produces deterministic, diffable text output via [`serialize()`][Self::serialize].
/// Does not produce visual output — designed for regression testing of the
/// interpretation layer (Layer 2).
pub struct RecordingDevice {
    calls: Vec<DeviceCall>,
    page_width: f64,
    page_height: f64,
    /// Track which fonts have been "loaded" so `has_font` works correctly.
    fonts_loaded: std::collections::HashSet<String>,
    /// Optional standard-font resolver — mirrors `Device::resolve_standard_font_data`.
    /// Set via [`Self::with_standard_font_resolver`].
    standard_font_resolver: Option<fn(&str) -> Option<Vec<u8>>>,
}

impl RecordingDevice {
    /// Create a new recording device for the given page dimensions.
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            calls: Vec::new(),
            page_width: width,
            page_height: height,
            fonts_loaded: std::collections::HashSet::new(),
            standard_font_resolver: None,
        }
    }

    /// Install a custom standard-font resolver (e.g. Liberation TTF).
    /// When set, `resolve_standard_font_data` delegates to this function
    /// instead of returning `None`.
    pub fn with_standard_font_resolver(mut self, resolver: fn(&str) -> Option<Vec<u8>>) -> Self {
        self.standard_font_resolver = Some(resolver);
        self
    }

    /// Get the recorded calls.
    pub fn calls(&self) -> &[DeviceCall] {
        &self.calls
    }

    /// Serialize all recorded calls to a deterministic text format.
    ///
    /// Each call is one line. Floats use 6 decimal places for cross-platform
    /// determinism. The output is suitable for snapshot testing.
    pub fn serialize(&self) -> String {
        self.calls
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Clear all recorded calls.
    pub fn clear(&mut self) {
        self.calls.clear();
        self.fonts_loaded.clear();
    }

    /// Number of recorded calls.
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    /// Whether no calls have been recorded.
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// Synthetic text width — same formula as TestDevice.
    /// Records the returned width transparently; the actual value is only
    /// used for text-matrix advancement in the interpreter.
    fn synthetic_text_width(
        text_bytes: &[u8],
        font_size: f64,
        horizontal_scaling: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> f64 {
        let num_chars = text_bytes.len() as f64;
        let num_spaces = text_bytes.iter().filter(|&&b| b == b' ').count() as f64;
        let glyph_width = (num_chars * 500.0 * font_size) / 1000.0;
        let spacing = char_spacing * num_chars + word_spacing * num_spaces;
        (glyph_width + spacing) * (horizontal_scaling / 100.0)
    }
}

impl Device for RecordingDevice {
    fn begin_path(&mut self) {
        self.calls.push(DeviceCall::BeginPath);
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.calls.push(DeviceCall::MoveTo(x, y));
    }

    fn line_to(&mut self, x: f64, y: f64) {
        self.calls.push(DeviceCall::LineTo(x, y));
    }

    fn curve_to(
        &mut self,
        cp1x: f64,
        cp1y: f64,
        cp2x: f64,
        cp2y: f64,
        x: f64,
        y: f64,
    ) {
        self.calls
            .push(DeviceCall::CurveTo(cp1x, cp1y, cp2x, cp2y, x, y));
    }

    fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        self.calls.push(DeviceCall::Rect(x, y, width, height));
    }

    fn close_path(&mut self) {
        self.calls.push(DeviceCall::ClosePath);
    }

    fn draw_path(
        &mut self,
        mode: PathDrawMode,
        paint: &Paint,
        stroke_props: &StrokeProps,
    ) -> PDFResult<()> {
        self.calls.push(DeviceCall::DrawPath {
            mode: fmt_path_mode(mode),
            paint: fmt_paint(paint),
            stroke: fmt_stroke(stroke_props),
        });
        Ok(())
    }

    fn clip_path(&mut self, rule: FillRule) -> PDFResult<()> {
        self.calls
            .push(DeviceCall::ClipPath(fmt_fill_rule(rule)));
        Ok(())
    }

    fn clip_stroke_path(&mut self, _stroke_props: &StrokeProps) -> PDFResult<()> {
        self.calls.push(DeviceCall::ClipStrokePath);
        Ok(())
    }

    fn save_state(&mut self) {
        self.calls.push(DeviceCall::SaveState);
    }

    fn restore_state(&mut self) {
        self.calls.push(DeviceCall::RestoreState);
    }

    fn concat_matrix(&mut self, matrix: &[f64; 6]) {
        self.calls.push(DeviceCall::ConcatMatrix(*matrix));
    }

    fn set_matrix(&mut self, matrix: &[f64; 6]) {
        self.calls.push(DeviceCall::SetMatrix(*matrix));
    }

    fn set_blend_mode(&mut self, mode: BlendMode) {
        self.calls
            .push(DeviceCall::SetBlendMode(fmt_blend(mode)));
    }

    fn set_opacity(&mut self, opacity: f64) {
        self.calls.push(DeviceCall::SetOpacity(opacity));
    }

    fn draw_text(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        paint: &Paint,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<f64> {
        self.calls.push(DeviceCall::DrawText {
            font: font_name.to_string(),
            size: font_size,
            paint: fmt_paint(paint),
            text_matrix: *text_matrix,
            hscale: horizontal_scaling,
            rise: text_rise,
            cs: char_spacing,
            ws: word_spacing,
            byte_len: text_bytes.len(),
        });
        Ok(Self::synthetic_text_width(
            text_bytes,
            font_size,
            horizontal_scaling,
            char_spacing,
            word_spacing,
        ))
    }

    fn draw_text_advance(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        paint: &Paint,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<(f64, f64)> {
        let w = self.draw_text(
            text_bytes,
            font_name,
            font_size,
            paint,
            text_matrix,
            horizontal_scaling,
            text_rise,
            char_spacing,
            word_spacing,
        )?;
        Ok((w, 0.0))
    }

    fn draw_text_advance_with_mode(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        fill_paint: &Paint,
        stroke_paint: &Paint,
        stroke_props: &StrokeProps,
        rendering_mode: TextRenderingMode,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<(f64, f64)> {
        // Record which mode was used. The default dispatch picks paint by mode,
        // but we record the mode-transparent info so the snapshot is complete.
        let _stroke_props = stroke_props;
        let paint = match rendering_mode {
            TextRenderingMode::Stroke | TextRenderingMode::StrokeClip => stroke_paint,
            TextRenderingMode::Invisible | TextRenderingMode::Clip => &Paint::Transparent,
            _ => fill_paint,
        };
        self.draw_text_advance(
            text_bytes,
            font_name,
            font_size,
            paint,
            text_matrix,
            horizontal_scaling,
            text_rise,
            char_spacing,
            word_spacing,
        )
    }

    fn draw_type0_glyph_run(
        &mut self,
        run: &Type0GlyphRun,
        font_name: &str,
        font_size: f64,
        paint: &Paint,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
    ) -> PDFResult<Option<(f64, f64)>> {
        self.calls.push(DeviceCall::DrawType0GlyphRun {
            font: font_name.to_string(),
            size: font_size,
            paint: fmt_paint(paint),
            glyph_count: run.glyphs.len(),
            vertical: run.vertical,
            text_matrix: *text_matrix,
            hscale: horizontal_scaling,
            rise: text_rise,
        });
        // Return None so the interpreter falls back to byte-level draw_text.
        Ok(None)
    }

    fn draw_type0_glyph_run_with_mode(
        &mut self,
        run: &Type0GlyphRun,
        font_name: &str,
        font_size: f64,
        fill_paint: &Paint,
        stroke_paint: &Paint,
        stroke_props: &StrokeProps,
        rendering_mode: TextRenderingMode,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
    ) -> PDFResult<Option<(f64, f64)>> {
        let _stroke_props = stroke_props;
        let paint = match rendering_mode {
            TextRenderingMode::Stroke | TextRenderingMode::StrokeClip => stroke_paint,
            TextRenderingMode::Invisible | TextRenderingMode::Clip => &Paint::Transparent,
            _ => fill_paint,
        };
        self.draw_type0_glyph_run(
            run,
            font_name,
            font_size,
            paint,
            text_matrix,
            horizontal_scaling,
            text_rise,
        )
    }

    fn clip_text(
        &mut self, text_bytes: &[u8], font_name: &str, font_size: f64, text_matrix: &[f64; 6], horizontal_scaling: f64, text_rise: f64, char_spacing: f64, word_spacing: f64) {
        let _ = (text_matrix, horizontal_scaling, text_rise, char_spacing, word_spacing);
        self.calls.push(DeviceCall::ClipText {
            font: font_name.to_string(),
            size: font_size,
            byte_len: text_bytes.len(),
        });
    }

    fn flush_text_clip(&mut self) -> PDFResult<()> {
        self.calls.push(DeviceCall::FlushTextClip);
        Ok(())
    }

    fn draw_image(&mut self, image: ImageData, transform: &[f64; 6]) -> PDFResult<()> {
        self.calls.push(DeviceCall::DrawImage {
            w: image.width,
            h: image.height,
            transform: *transform,
            bpc: image.bits_per_component,
            has_alpha: image.has_alpha,
            is_mask: image.is_mask,
        });
        Ok(())
    }

    fn begin_type3_glyph(&mut self) {
        self.calls.push(DeviceCall::BeginType3Glyph);
    }

    fn end_type3_glyph(&mut self) {
        self.calls.push(DeviceCall::EndType3Glyph);
    }

    fn draw_shading(&mut self, shading: &Shading, _ctm: &[f64; 6]) -> PDFResult<()> {
        self.calls.push(DeviceCall::DrawShading {
            n_components: shading.n_components,
        });
        Ok(())
    }

    fn begin_offscreen(&mut self) -> PDFResult<()> {
        self.calls.push(DeviceCall::BeginOffscreen);
        Ok(())
    }

    fn end_offscreen_as_mask(
        &mut self,
        luminosity: bool,
        _backdrop: Option<[u8; 3]>,
    ) -> Option<(Vec<u8>, u32, u32)> {
        self.calls
            .push(DeviceCall::EndOffscreenAsMask { luminosity });
        // Return a dummy mask so the interpreter doesn't fall back.
        Some((vec![0u8; 4], 1, 1))
    }

    fn end_offscreen_as_rgba_region(&mut self, width: u32, height: u32) -> Option<Vec<u8>> {
        self.calls
            .push(DeviceCall::EndOffscreenAsRgbaRegion { width, height });
        Some(vec![0u8; (width as usize) * (height as usize) * 4])
    }

    fn end_offscreen_composite(&mut self, opacity: f64) -> PDFResult<()> {
        self.calls
            .push(DeviceCall::EndOffscreenComposite { opacity });
        Ok(())
    }

    fn set_soft_mask(&mut self, mask: Option<(&[u8], u32, u32)>) -> PDFResult<()> {
        self.calls.push(DeviceCall::SetSoftMask {
            has_mask: mask.is_some(),
        });
        Ok(())
    }

    fn supports_pattern_cell_fill(&self) -> bool {
        true // Record the fast-path pattern fill
    }

    fn fill_path_with_pattern_cell(
        &mut self,
        rule: FillRule,
        rgba: &[u8],
        width: u32,
        height: u32,
        _image_to_device: &[f64; 6],
        _pattern_to_device: &[f64; 6],
        _bbox: &[f64; 4],
        x_step: f64,
        y_step: f64,
        _tile_bounds: (i32, i32, i32, i32),
    ) -> PDFResult<bool> {
        self.calls.push(DeviceCall::FillPathWithPatternCell {
            rule: fmt_fill_rule(rule),
            rgba_len: rgba.len(),
            cell_w: width,
            cell_h: height,
            x_step,
            y_step,
        });
        Ok(true)
    }

    fn page_bounds(&self) -> (f64, f64) {
        (self.page_width, self.page_height)
    }

    fn load_font_data(
        &mut self,
        name: &str,
        data: Vec<u8>,
        _encoding: Option<&PDFObject>,
    ) -> PDFResult<()> {
        self.fonts_loaded.insert(name.to_string());
        self.calls.push(DeviceCall::LoadFontData {
            name: name.to_string(),
            data_len: data.len(),
        });
        Ok(())
    }

    fn set_font_widths(
        &mut self,
        name: &str,
        first_char: u32,
        widths: &[f64],
        _default_width: f64,
    ) -> PDFResult<()> {
        self.calls.push(DeviceCall::SetFontWidths {
            name: name.to_string(),
            first_char,
            count: widths.len(),
        });
        Ok(())
    }

    fn set_font_raw_cid_codes(&mut self, name: &str, enabled: bool) -> PDFResult<()> {
        self.calls.push(DeviceCall::SetFontRawCidCodes {
            name: name.to_string(),
            enabled,
        });
        Ok(())
    }

    fn set_font_cid_vertical_metrics(
        &mut self,
        name: &str,
        vertical: bool,
        _dw2: (f64, f64),
        widths2: &[(u32, f64, f64, f64)],
    ) -> PDFResult<()> {
        self.calls.push(DeviceCall::SetFontCidVerticalMetrics {
            name: name.to_string(),
            vertical,
            widths2_count: widths2.len(),
        });
        Ok(())
    }

    fn set_font_cid_to_gid_map(&mut self, name: &str, map: &[u16]) -> PDFResult<()> {
        self.calls.push(DeviceCall::SetFontCidToGidMap {
            name: name.to_string(),
            map_len: map.len(),
        });
        Ok(())
    }

    fn has_font(&self, name: &str) -> bool {
        self.fonts_loaded.contains(name)
    }

    fn resolve_standard_font_data(&mut self, base_font: &str) -> Option<Vec<u8>> {
        self.standard_font_resolver.and_then(|r| r(base_font))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_path_and_draw() {
        let mut dev = RecordingDevice::new(612.0, 792.0);
        dev.begin_path();
        dev.move_to(10.0, 20.0);
        dev.line_to(100.0, 200.0);
        dev.draw_path(
            PathDrawMode::Fill(FillRule::NonZero),
            &Paint::from_color(Color::RGB(1.0, 0.0, 0.0)),
            &StrokeProps::default(),
        )
        .unwrap();

        let s = dev.serialize();
        assert!(s.contains("begin_path"));
        assert!(s.contains("move_to(10.000000,20.000000)"));
        assert!(s.contains("line_to(100.000000,200.000000)"));
        assert!(s.contains("draw_path(fill(NonZero), paint=RGB(1.000000,0.000000,0.000000)"));
    }

    #[test]
    fn test_draw_path_captures_stroke_props() {
        use super::super::graphics_state::LineCap;
        let mut dev = RecordingDevice::new(100.0, 100.0);
        dev.begin_path();
        dev.rect(0.0, 0.0, 50.0, 50.0);
        let mut sp = StrokeProps::default();
        sp.line_width = 2.5;
        sp.line_cap = LineCap::Round;
        sp.dash_array = vec![3.0, 2.0];
        dev.draw_path(PathDrawMode::Stroke, &Paint::black(), &sp)
            .unwrap();

        let s = dev.serialize();
        assert!(s.contains("lw=2.500000"));
        assert!(s.contains("cap=Round"));
        assert!(s.contains("dash=[3.000000,2.000000]"));
    }

    #[test]
    fn test_draw_text_captures_all_params() {
        let mut dev = RecordingDevice::new(612.0, 792.0);
        let w = dev
            .draw_text(
                b"Hello",
                "F1",
                12.0,
                &Paint::from_color(Color::Gray(0.5)),
                &[1.0, 0.0, 0.0, 1.0, 100.0, 700.0],
                100.0,
                0.0,
                1.5,
                2.0,
            )
            .unwrap();

        assert!(w > 0.0);
        let s = dev.serialize();
        assert!(s.contains("font=F1"));
        assert!(s.contains("size=12.000000"));
        assert!(s.contains("paint=Gray(0.500000)"));
        assert!(s.contains("bytes=5"));
        assert!(s.contains("cs=1.500000"));
        assert!(s.contains("ws=2.000000"));
    }

    #[test]
    fn test_opacity_and_blend_recorded() {
        let mut dev = RecordingDevice::new(100.0, 100.0);
        dev.set_opacity(0.5);
        dev.set_blend_mode(BlendMode::Multiply);

        let s = dev.serialize();
        assert!(s.contains("set_opacity(0.500000)"));
        assert!(s.contains("set_blend_mode(Multiply)"));
    }

    #[test]
    fn test_shading_recorded() {
        use super::super::shading::{Shading, ShadingColorSpace, ShadingFunction, ShadingKind};
        let shading = Shading {
            kind: ShadingKind::Axial {
                x0: 0.0, y0: 0.0, x1: 100.0, y1: 100.0,
                t0: 0.0, t1: 1.0,
                function: ShadingFunction::Exponential {
                    c0: vec![0.0, 0.0, 0.0],
                    c1: vec![1.0, 1.0, 1.0],
                    n: 1.0,
                },
                extend_start: true,
                extend_end: true,
            },
            n_components: 3,
            color_space: ShadingColorSpace::DeviceRGB,
            bbox: None,
            matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            background: None,
            use_root_ctm: false,
        };
        let mut dev = RecordingDevice::new(100.0, 100.0);
        dev.draw_shading(&shading, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
            .unwrap();
        let s = dev.serialize();
        assert!(s.contains("draw_shading(n=3)"));
    }

    #[test]
    fn test_deterministic_float_formatting() {
        let mut dev = RecordingDevice::new(100.0, 100.0);
        dev.move_to(1.0 / 3.0, 2.0 / 3.0);
        let s = dev.serialize();
        // Must be exactly 6 decimal places — no scientific notation, no trailing noise
        assert!(s.contains("move_to(0.333333,0.666667)"));
    }

    #[test]
    fn test_font_management_roundtrip() {
        let mut dev = RecordingDevice::new(100.0, 100.0);
        assert!(!dev.has_font("F1"));

        dev.load_font_data("F1", vec![0u8; 100], None).unwrap();
        assert!(dev.has_font("F1"));

        dev.set_font_widths("F1", 32, &[500.0, 600.0], 250.0)
            .unwrap();

        let s = dev.serialize();
        assert!(s.contains("load_font_data(name=F1, bytes=100)"));
        assert!(s.contains("set_font_widths(name=F1, first=32, count=2)"));
    }

    #[test]
    fn test_cmyk_paint_recorded() {
        let mut dev = RecordingDevice::new(100.0, 100.0);
        dev.begin_path();
        dev.draw_path(
            PathDrawMode::Fill(FillRule::EvenOdd),
            &Paint::from_color(Color::CMYK(0.1, 0.2, 0.3, 0.4)),
            &StrokeProps::default(),
        )
        .unwrap();
        let s = dev.serialize();
        assert!(s.contains("paint=CMYK(0.100000,0.200000,0.300000,0.400000)"));
    }
}
