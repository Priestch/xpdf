//! Device trait for rendering backend abstraction.
//!
//! This module defines the Device trait, which abstracts the rendering backend.
//! This allows different rendering implementations (e.g., CPU rendering, GPU rendering,
//! image export) without changing the content stream interpretation logic.

use super::graphics_state::{Color, FillRule, StrokeProps};
use crate::core::error::PDFResult;
use crate::core::parser::PDFObject;

/// How to draw a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathDrawMode {
    /// Fill the path
    Fill(FillRule),
    /// Stroke the path outline
    Stroke,
    /// Fill and stroke the path
    FillStroke(FillRule),
}

/// Paint for drawing operations.
///
/// This represents how a shape should be filled/stroked.
/// For now, we support solid colors. In the future, this will support
/// patterns, shadings, and images.
#[derive(Debug, Clone)]
pub enum Paint {
    /// Solid color
    Solid(Color),
    // TODO: Add Pattern, Shading, Image support
}

impl Paint {
    /// Create a solid black paint.
    pub fn black() -> Self {
        Paint::Solid(Color::black())
    }

    /// Create a solid white paint.
    pub fn white() -> Self {
        Paint::Solid(Color::white())
    }

    /// Create a solid paint from a color.
    pub fn from_color(color: Color) -> Self {
        Paint::Solid(color)
    }
}

impl Default for Paint {
    fn default() -> Self {
        Paint::black()
    }
}

/// Image data for rendering.
///
/// This represents image data that can be drawn by a device.
#[derive(Debug, Clone)]
pub struct ImageData {
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Image data (RGB or RGBA)
    pub data: Vec<u8>,
    /// Whether the image has an alpha channel
    pub has_alpha: bool,
    /// Bits per component
    pub bits_per_component: u8,
}

/// A device that can render PDF drawing operations.
///
/// This trait abstracts the rendering backend, allowing different implementations
/// for different use cases (screen display, image export, GPU rendering, etc.).
///
/// The design follows the Device trait from hayro, which is inspired by PDF.js's
/// operator execution pattern.
pub trait Device {
    /// Begin a new path.
    fn begin_path(&mut self);

    /// Move the current point to (x, y) starting a new subpath.
    fn move_to(&mut self, x: f64, y: f64);

    /// Add a straight line segment from the current point to (x, y).
    fn line_to(&mut self, x: f64, y: f64);

    /// Add a cubic Bézier curve from the current point.
    ///
    /// # Arguments
    /// * `cp1x, cp1y` - First control point
    /// * `cp2x, cp2y` - Second control point
    /// * `x, y` - End point
    fn curve_to(&mut self, cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64);

    /// Add a rectangle to the path.
    fn rect(&mut self, x: f64, y: f64, width: f64, height: f64);

    /// Close the current subpath.
    fn close_path(&mut self);

    /// Draw the current path.
    ///
    /// # Arguments
    /// * `mode` - How to draw the path (fill, stroke, or both)
    /// * `paint` - The paint/color to use
    /// * `stroke_props` - Stroke properties (only used for stroking)
    fn draw_path(
        &mut self,
        mode: PathDrawMode,
        paint: &Paint,
        stroke_props: &StrokeProps,
    ) -> PDFResult<()>;

    /// Set a clipping path.
    ///
    /// Subsequent drawing operations will be clipped to this path.
    ///
    /// # Arguments
    /// * `rule` - Fill rule to use for the clipping path
    fn clip_path(&mut self, rule: FillRule) -> PDFResult<()>;

    /// Save the graphics state.
    fn save_state(&mut self);

    /// Restore the graphics state.
    fn restore_state(&mut self);

    /// Concatenate a transformation matrix to the current CTM.
    ///
    /// # Arguments
    /// * `matrix` - 6-element array [a b c d e f] representing the transform
    fn concat_matrix(&mut self, matrix: &[f64; 6]);

    /// Set the transformation matrix.
    ///
    /// # Arguments
    /// * `matrix` - 6-element array [a b c d e f] representing the transform
    fn set_matrix(&mut self, matrix: &[f64; 6]);

    /// Draw text at the current position.
    ///
    /// # Arguments
    /// * `text_bytes` - Raw text bytes using the font's encoding (NOT UTF-8)
    /// * `font_name` - Name of the font to use
    /// * `font_size` - Font size in points
    /// * `paint` - The paint/color to use
    /// * `text_matrix` - Text transformation matrix (for positioning text in user space)
    /// * `horizontal_scaling` - Horizontal text scaling as percentage (default: 100.0)
    /// * `text_rise` - Text rise in user space units (for superscript/subscript)
    ///
    /// # Returns
    /// The total rendered width in text space units
    fn draw_text(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        paint: &Paint,
        text_matrix: &[f64; 6],
        horizontal_scaling: f64,
        text_rise: f64,
    ) -> PDFResult<f64>;

    /// Draw an image.
    ///
    /// # Arguments
    /// * `image` - The image data
    /// * `transform` - Transformation matrix for placing the image
    fn draw_image(&mut self, image: ImageData, transform: &[f64; 6]) -> PDFResult<()>;

    /// Get the current page bounds.
    ///
    /// Returns (width, height) in user space units.
    fn page_bounds(&self) -> (f64, f64);

    /// Load font data for rendering.
    ///
    /// This method allows loading font data (TrueType, CFF, etc.) for text rendering.
    /// Devices that don't support text rendering can ignore this.
    ///
    /// # Arguments
    /// * `name` - The font name/identifier (e.g., "F1", "Helvetica")
    /// * `data` - Raw font data
    /// * `encoding` - Optional PDF Encoding dictionary for custom glyph name mappings
    ///
    /// # Returns
    /// Ok(()) if successful, or an error if loading failed
    fn load_font_data(
        &mut self,
        name: &str,
        data: Vec<u8>,
        encoding: Option<&PDFObject>,
    ) -> PDFResult<()> {
        // Default implementation ignores font loading
        // (for devices that don't support text rendering)
        let _ = name;
        let _ = data;
        let _ = encoding;
        Ok(())
    }
}

/// A simple CPU-based device implementation for testing.
///
/// This is a minimal implementation that records drawing operations
/// but doesn't actually produce visual output. It's useful for testing
/// and as a reference implementation.
#[derive(Debug, Default)]
pub struct TestDevice {
    /// Page width in user space units
    page_width: f64,
    /// Page height in user space units
    page_height: f64,
    /// Graphics state stack
    state_stack: Vec<TestGraphicsState>,
    /// Recorded operations for testing
    operations: Vec<String>,
}

#[derive(Debug, Clone)]
struct TestGraphicsState {
    ctm: [f64; 6],
    stroke_color: Color,
    fill_color: Color,
}

impl Default for TestGraphicsState {
    fn default() -> Self {
        TestGraphicsState {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            stroke_color: Color::black(),
            fill_color: Color::black(),
        }
    }
}

impl TestDevice {
    /// Create a new test device with the given page dimensions.
    pub fn new(width: f64, height: f64) -> Self {
        TestDevice {
            page_width: width,
            page_height: height,
            state_stack: vec![TestGraphicsState::default()],
            operations: Vec::new(),
        }
    }

    /// Get the recorded operations.
    pub fn operations(&self) -> &[String] {
        &self.operations
    }

    /// Clear the recorded operations.
    pub fn clear_operations(&mut self) {
        self.operations.clear();
    }
}

impl Device for TestDevice {
    fn begin_path(&mut self) {
        self.operations.push("begin_path".to_string());
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.operations.push(format!("move_to({},{})", x, y));
    }

    fn line_to(&mut self, x: f64, y: f64) {
        self.operations.push(format!("line_to({},{})", x, y));
    }

    fn curve_to(&mut self, cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64) {
        self.operations.push(format!(
            "curve_to({},{},{},{},{},{})",
            cp1x, cp1y, cp2x, cp2y, x, y
        ));
    }

    fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        self.operations
            .push(format!("rect({},{},{},{})", x, y, width, height));
    }

    fn close_path(&mut self) {
        self.operations.push("close_path".to_string());
    }

    fn draw_path(
        &mut self,
        mode: PathDrawMode,
        _paint: &Paint,
        _stroke_props: &StrokeProps,
    ) -> PDFResult<()> {
        match mode {
            PathDrawMode::Fill(rule) => {
                self.operations.push(format!("draw_path(fill, {:?})", rule));
            }
            PathDrawMode::Stroke => {
                self.operations.push("draw_path(stroke)".to_string());
            }
            PathDrawMode::FillStroke(rule) => {
                self.operations
                    .push(format!("draw_path(fill_stroke, {:?})", rule));
            }
        }
        Ok(())
    }

    fn clip_path(&mut self, rule: FillRule) -> PDFResult<()> {
        self.operations.push(format!("clip_path({:?})", rule));
        Ok(())
    }

    fn save_state(&mut self) {
        let current = self.state_stack.last().unwrap().clone();
        self.state_stack.push(current);
        self.operations.push("save_state".to_string());
    }

    fn restore_state(&mut self) {
        if self.state_stack.len() > 1 {
            self.state_stack.pop();
        }
        self.operations.push("restore_state".to_string());
    }

    fn concat_matrix(&mut self, matrix: &[f64; 6]) {
        if let Some(state) = self.state_stack.last_mut() {
            // Concatenate matrix (simplified)
            let [a, b, c, d, e, f] = *matrix;
            let [ctm_a, ctm_b, ctm_c, ctm_d, ctm_e, ctm_f] = state.ctm;
            state.ctm = [
                ctm_a * a + ctm_c * b,
                ctm_b * a + ctm_d * b,
                ctm_a * c + ctm_c * d,
                ctm_b * c + ctm_d * d,
                ctm_a * e + ctm_c * f + ctm_e,
                ctm_b * e + ctm_d * f + ctm_f,
            ];
        }
        self.operations.push(format!("concat_matrix({:?})", matrix));
    }

    fn set_matrix(&mut self, matrix: &[f64; 6]) {
        if let Some(state) = self.state_stack.last_mut() {
            state.ctm = *matrix;
        }
        self.operations.push(format!("set_matrix({:?})", matrix));
    }

    fn draw_text(
        &mut self,
        text_bytes: &[u8],
        font_name: &str,
        font_size: f64,
        _paint: &Paint,
        _text_matrix: &[f64; 6],
        _horizontal_scaling: f64,
        _text_rise: f64,
    ) -> PDFResult<f64> {
        self.operations.push(format!(
            "draw_text({}, {}, {:?})",
            font_name, font_size, text_bytes
        ));
        // Return approximate width for testing
        let num_chars = text_bytes.len() as f64;
        let width = (num_chars * 500.0 * font_size) / 1000.0;
        Ok(width)
    }

    fn draw_image(&mut self, image: ImageData, transform: &[f64; 6]) -> PDFResult<()> {
        self.operations.push(format!(
            "draw_image({}x{}, {:?})",
            image.width, image.height, transform
        ));
        Ok(())
    }

    fn page_bounds(&self) -> (f64, f64) {
        (self.page_width, self.page_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_operations() {
        let mut device = TestDevice::new(612.0, 792.0);

        device.begin_path();
        device.move_to(100.0, 200.0);
        device.line_to(300.0, 400.0);
        device
            .draw_path(
                PathDrawMode::Stroke,
                &Paint::black(),
                &StrokeProps::default(),
            )
            .unwrap();

        let ops = device.operations();
        assert_eq!(ops[0], "begin_path");
        assert_eq!(ops[1], "move_to(100,200)");
        assert_eq!(ops[2], "line_to(300,400)");
        assert_eq!(ops[3], "draw_path(stroke)");
    }

    #[test]
    fn test_state_save_restore() {
        let mut device = TestDevice::new(612.0, 792.0);

        device.save_state();
        device.concat_matrix(&[2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
        device.restore_state();

        let ops = device.operations();
        assert_eq!(ops[0], "save_state");
        assert_eq!(ops[1], "concat_matrix([2.0, 0.0, 0.0, 2.0, 0.0, 0.0])");
        assert_eq!(ops[2], "restore_state");
    }
}
