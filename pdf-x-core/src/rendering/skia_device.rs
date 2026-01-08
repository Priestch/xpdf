//! A tiny-skia based rendering device.

use tiny_skia::{
    PixmapMut, Transform, PathBuilder, Paint as SkiaPaint, Stroke, LineCap as SkiaLineCap,
    LineJoin as SkiaLineJoin, FillRule as SkiaFillRule, Pixmap, Mask, Path, Rect,
};
use crate::rendering::device::{Device, PathDrawMode, Paint, StrokeProps, ImageData};
use crate::core::error::{PDFResult, PDFError};
use crate::rendering::graphics_state::{Color, FillRule, LineCap, LineJoin};
use std::collections::HashMap;
use crate::rendering::font::Font;
use ttf_parser::OutlineBuilder;

// --- Conversion helpers ---

fn to_skia_color(color: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(color.r, color.g, color.b, color.a)
}

fn to_skia_paint(paint: &Paint) -> SkiaPaint {
    let mut sk_paint = SkiaPaint::default();
    match paint {
        Paint::Solid(color) => {
            sk_paint.set_color(to_skia_color(*color));
        }
    }
    sk_paint.anti_alias = true;
    sk_paint
}

fn to_skia_line_cap(line_cap: LineCap) -> SkiaLineCap {
    match line_cap {
        LineCap::Butt => SkiaLineCap::Butt,
        LineCap::Round => SkiaLineCap::Round,
        LineCap::ProjectingSquare => SkiaLineCap::Square,
    }
}

fn to_skia_line_join(line_join: LineJoin) -> SkiaLineJoin {
    match line_join {
        LineJoin::Miter => SkiaLineJoin::Miter,
        LineJoin::Round => SkiaLineJoin::Round,
        LineJoin::Bevel => SkiaLineJoin::Bevel,
    }
}

fn to_skia_fill_rule(fill_rule: FillRule) -> SkiaFillRule {
    match fill_rule {
        FillRule::NonZero => SkiaFillRule::Winding,
        FillRule::EvenOdd => SkiaFillRule::EvenOdd,
    }
}

fn to_skia_stroke(stroke_props: &StrokeProps) -> Stroke {
    Stroke {
        width: stroke_props.line_width as f32,
        miter_limit: stroke_props.miter_limit as f32,
        line_cap: to_skia_line_cap(stroke_props.line_cap),
        line_join: to_skia_line_join(stroke_props.line_join),
        dash: None, // TODO: Add dash pattern support
    }
}

#[derive(Clone)]
struct SkiaGraphicsState {
    transform: Transform,
    clip_path: Option<Path>,
}

impl Default for SkiaGraphicsState {
    fn default() -> Self {
        SkiaGraphicsState {
            transform: Transform::identity(),
            clip_path: None,
        }
    }
}

pub struct SkiaDevice<'a> {
    pixmap: PixmapMut<'a>,
    state_stack: Vec<SkiaGraphicsState>,
    path_builder: PathBuilder,
    font_cache: HashMap<String, Font>,
}

struct PathConverter(PathBuilder);

impl OutlineBuilder for PathConverter {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.0.close();
    }
}

impl<'a> SkiaDevice<'a> {
    pub fn new(pixmap: PixmapMut<'a>) -> Self {
        SkiaDevice {
            pixmap,
            state_stack: vec![SkiaGraphicsState::default()],
            path_builder: PathBuilder::new(),
            font_cache: HashMap::new(),
        }
    }

    pub fn load_font(&mut self, name: &str, data: &'static [u8]) -> PDFResult<()> {
        let font = Font::new(data).map_err(|e| PDFError::RenderingError(format!("Failed to load font: {}", e)))?;
        self.font_cache.insert(name.to_string(), font);
        Ok(())
    }

    fn current_state(&self) -> &SkiaGraphicsState {
        self.state_stack.last().unwrap()
    }

    fn current_state_mut(&mut self) -> &mut SkiaGraphicsState {
        self.state_stack.last_mut().unwrap()
    }

    fn get_clip_mask(&self) -> Option<Mask> {
        let mut mask = Mask::new(self.pixmap.width(), self.pixmap.height())?;
        if let Some(path) = &self.current_state().clip_path {
            mask.fill_path(path, SkiaFillRule::Winding, false, Transform::identity());
        }
        Some(mask)
    }
}

impl<'a> Device for SkiaDevice<'a> {
    fn begin_path(&mut self) {
        self.path_builder = PathBuilder::new();
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.path_builder.move_to(x as f32, y as f32);
    }

    fn line_to(&mut self, x: f64, y: f64) {
        self.path_builder.line_to(x as f32, y as f32);
    }

    fn curve_to(&mut self, cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64) {
        self.path_builder.cubic_to(cp1x as f32, cp1y as f32, cp2x as f32, cp2y as f32, x as f32, y as f32);
    }

    fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        self.path_builder.push_rect(Rect::from_xywh(x as f32, y as f32, width as f32, height as f32).unwrap());
    }

    fn close_path(&mut self) {
        self.path_builder.close();
    }

    fn draw_path(&mut self, mode: PathDrawMode, paint: &Paint, stroke_props: &StrokeProps) -> PDFResult<()> {
        let path = self.path_builder.finish().ok_or(PDFError::RenderingError("Invalid path".into()))?;
        self.path_builder = PathBuilder::new();

        let sk_paint = to_skia_paint(paint);
        let transform = self.current_state().transform;
        let clip_mask = self.get_clip_mask();

        match mode {
            PathDrawMode::Fill(fill_rule) => {
                self.pixmap.fill_path(
                    &path,
                    &sk_paint,
                    to_skia_fill_rule(fill_rule),
                    transform,
                    clip_mask.as_ref(),
                );
            }
            PathDrawMode::Stroke => {
                let sk_stroke = to_skia_stroke(stroke_props);
                self.pixmap.stroke_path(&path, &sk_paint, &sk_stroke, transform, clip_mask.as_ref());
            }
            PathDrawMode::FillStroke(fill_rule) => {
                self.pixmap.fill_path(
                    &path,
                    &sk_paint,
                    to_skia_fill_rule(fill_rule),
                    transform,
                    clip_mask.as_ref(),
                );
                let sk_stroke = to_skia_stroke(stroke_props);
                self.pixmap.stroke_path(&path, &sk_paint, &sk_stroke, transform, clip_mask.as_ref());
            }
        }

        Ok(())
    }

    fn clip_path(&mut self, rule: FillRule) -> PDFResult<()> {
        let path = self.path_builder.finish().ok_or(PDFError::RenderingError("Invalid path".into()))?;
        self.path_builder = PathBuilder::new();

        let new_clip_path = if let Some(old_clip) = &self.current_state().clip_path {
            // Intersect new path with old one
            if let Some(p) = old_clip.clone().intersect(&path, to_skia_fill_rule(rule)) {
                p
            } else {
                // No intersection, so everything is clipped
                PathBuilder::new().finish().unwrap()
            }
        } else {
            path
        };

        self.current_state_mut().clip_path = Some(new_clip_path);

        Ok(())
    }

    fn save_state(&mut self) {
        let current_state = self.current_state().clone();
        self.state_stack.push(current_state);
    }

    fn restore_state(&mut self) {
        if self.state_stack.len() > 1 {
            self.state_stack.pop();
        }
    }

    fn concat_matrix(&mut self, matrix: &[f64; 6]) {
        let transform = Transform::from_row(
            matrix[0] as f32, matrix[1] as f32, matrix[2] as f32,
            matrix[3] as f32, matrix[4] as f32, matrix[5] as f32
        );
        self.current_state_mut().transform = self.current_state().transform.post_concat(transform);
    }

    fn set_matrix(&mut self, matrix: &[f64; 6]) {
        self.current_state_mut().transform = Transform::from_row(
            matrix[0] as f32, matrix[1] as f32, matrix[2] as f32,
            matrix[3] as f32, matrix[4] as f32, matrix[5] as f32
        );
    }

    fn draw_text(
        &mut self,
        text: &str,
        font_name: &str,
        font_size: f64,
        paint: &Paint,
    ) -> PDFResult<()> {
        let font = self.font_cache.get(font_name).ok_or_else(|| {
            PDFError::RenderingError(format!("Font '{}' not found", font_name))
        })?;

        let shaped_buffer = font.shape(text);
        let glyph_infos = shaped_buffer.glyph_infos();
        let glyph_positions = shaped_buffer.glyph_positions();

        let mut text_path_builder = PathBuilder::new();
        let scale = font_size as f32 / font.face().units_per_em() as f32;

        let mut current_x = 0.0;
        let mut current_y = 0.0;

        for (info, pos) in glyph_infos.iter().zip(glyph_positions.iter()) {
            let mut converter = PathConverter(PathBuilder::new());
            let transform = Transform::from_scale(scale, -scale).post_translate(current_x, current_y);

            let _ = font.face().outline_glyph(info.glyph_id, &mut converter);

            if let Some(p) = converter.0.finish() {
                 if let Some(path_transformed) = p.transform(transform) {
                    text_path_builder.push_path(&path_transformed);
                 }
            }

            current_x += pos.x_advance as f32 * scale;
            current_y += pos.y_advance as f32 * scale;
        }

        if let Some(path) = text_path_builder.finish() {
            let sk_paint = to_skia_paint(paint);
            let transform = self.current_state().transform;
            let clip_mask = self.get_clip_mask();

            self.pixmap.fill_path(
                &path,
                &sk_paint,
                SkiaFillRule::Winding,
                transform,
                clip_mask.as_ref(),
            );
        }

        Ok(())
    }

    fn draw_image(&mut self, image: ImageData<'_>, transform: &[f64; 6]) -> PDFResult<()> {
        // For now, we only support RGBA data.
        // A full implementation would handle different color spaces and formats.
        if !image.has_alpha || image.bits_per_component != 8 {
            return Err(PDFError::RenderingError("Unsupported image format".into()));
        }

        let image_pixmap = Pixmap::from_vec(
            image.data.to_vec(),
            tiny_skia::IntSize::from_wh(image.width, image.height).unwrap(),
        )
        .ok_or(PDFError::RenderingError("Failed to create image pixmap".into()))?;

        let image_transform = Transform::from_row(
            transform[0] as f32, transform[1] as f32, transform[2] as f32,
            transform[3] as f32, transform[4] as f32, transform[5] as f32
        );

        let final_transform = self.current_state().transform.post_concat(image_transform);
        let clip_mask = self.get_clip_mask();

        self.pixmap.draw_pixmap(
            0,
            0,
            image_pixmap.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            final_transform,
            clip_mask.as_ref(),
        );

        Ok(())
    }

    fn page_bounds(&self) -> (f64, f64) {
        (self.pixmap.width() as f64, self.pixmap.height() as f64)
    }
}