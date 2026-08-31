//! Device trait for rendering backend abstraction.
//!
//! This module defines the Device trait, which abstracts the rendering backend.
//! This allows different rendering implementations (e.g., CPU rendering, GPU rendering,
//! image export) without changing the content stream interpretation logic.

use super::graphics_state::{BlendMode, Color, FillRule, StrokeProps, TextRenderingMode};
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
    /// Fully transparent paint (draws nothing but still participates in layout measurement).
    Transparent,
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
    /// Bits per component (after expansion — may be 8 even for
    /// images that were originally 1-bpc).
    pub bits_per_component: u8,
    /// Original bits per component before any expansion (used to
    /// preserve bilevel semantics for the device's resampling heuristics).
    pub original_bits_per_component: u8,
    /// Whether this is a 1-bpp image mask (stencil).
    pub is_mask: bool,
    /// Image-mask polarity after applying the PDF /Decode array.
    /// When true, set bits paint; when false, unset bits paint.
    pub mask_paint_one: bool,
    /// Fill color for image masks (R, G, B in 0-255). Only used when is_mask.
    pub mask_color: [u8; 3],
    /// Whether the PDF requested image interpolation.
    pub interpolate: bool,
}

/// A decoded Type0/CID glyph in PDF glyph space.
#[derive(Debug, Clone, Copy)]
pub struct Type0Glyph {
    /// Original PDF character code read from the font CMap.
    pub code: u32,
    /// CID after CMap lookup. Identity-H/V use the code itself.
    pub cid: u16,
    /// Explicit glyph id when `/CIDToGIDMap` or a CID font table provides one.
    pub glyph_id: Option<u16>,
    /// Advance in PDF glyph-space units (1000 units/em).
    pub advance: (f64, f64),
    /// Glyph origin displacement in PDF glyph-space units.
    pub origin: (f64, f64),
}

/// A decoded Type0/CID text run.
#[derive(Debug, Clone)]
pub struct Type0GlyphRun {
    /// Decoded glyph records in drawing order.
    pub glyphs: Vec<Type0Glyph>,
    /// True for vertical writing mode.
    pub vertical: bool,
    /// Character spacing from `Tc`, in text-space units.
    pub character_spacing: f64,
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

    /// Set a clipping path from the stroked outline of the current path.
    ///
    /// Backends that cannot build a stroke mask may conservatively fall back to
    /// clipping the path fill. Raster backends should override this for Pattern
    /// and Shading stroke paints.
    fn clip_stroke_path(&mut self, stroke_props: &StrokeProps) -> PDFResult<()> {
        let _ = stroke_props;
        self.clip_path(FillRule::NonZero)
    }

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

    /// Set the current blend mode for subsequent drawing operations.
    ///
    /// Implementations that don't support blend modes can ignore this.
    fn set_blend_mode(&mut self, _mode: BlendMode) {}

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
    /// * `char_spacing` - Character spacing from `Tc` (applied per shown code)
    /// * `word_spacing` - Word spacing from `Tw` (applied to space code 0x20 in simple fonts)
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
        char_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<f64>;

    /// Draw text and return the text-matrix advance vector.
    ///
    /// Composite vertical fonts advance in Y, so a single width is not enough
    /// to update `Tm` correctly. Backends that only support horizontal text can
    /// rely on this default wrapper.
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
        let width = self.draw_text(
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
        Ok((width, 0.0))
    }

    /// Draw text according to the PDF text rendering mode and return the text-matrix advance.
    ///
    /// Backends with outline access should override this so stroke and fill-stroke text
    /// use the current stroke paint and stroke parameters instead of being filled.
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

    /// Draw a decoded Type0/CID glyph run and return the text-matrix advance.
    ///
    /// Coordinates in `run` are PDF glyph-space units, not font units. Backends
    /// with direct glyph-id rendering can use this to avoid rewriting PDF text
    /// bytes through `/ToUnicode`.
    fn draw_type0_glyph_run(
        &mut self,
        _run: &Type0GlyphRun,
        _font_name: &str,
        _font_size: f64,
        _paint: &Paint,
        _text_matrix: &[f64; 6],
        _horizontal_scaling: f64,
        _text_rise: f64,
    ) -> PDFResult<Option<(f64, f64)>> {
        Ok(None)
    }

    /// Draw a decoded Type0/CID glyph run according to the PDF text rendering mode.
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

    /// Whether this backend can map CIDFontType2 CIDs through native font charmaps
    /// when a PDF omits `/CIDToGIDMap`.
    fn supports_native_cid_to_gid(&self) -> bool {
        false
    }

    /// Whether a loaded font has a native charmap usable for CID-to-GID lookup.
    fn font_has_native_cid_to_gid(&self, _name: &str) -> bool {
        false
    }

    /// Add the glyph outlines of a text run to the current clip path.
    ///
    /// Called for text rendering modes 4–7 (Clip variants) in addition to
    /// (or instead of) `draw_text`. The parameters mirror `draw_text`.
    /// Devices that don't support text clipping can no-op.
    fn clip_text(
        &mut self,
        _text_bytes: &[u8],
        _font_name: &str,
        _font_size: f64,
        _text_matrix: &[f64; 6],
        _horizontal_scaling: f64,
        _text_rise: f64,
        _char_spacing: f64,
        _word_spacing: f64,
    ) {
    }

    /// Apply any text clipping path accumulated during the current text object.
    fn flush_text_clip(&mut self) -> PDFResult<()> {
        Ok(())
    }

    /// Draw an image.
    ///
    /// # Arguments
    /// * `image` - The image data
    /// * `transform` - Transformation matrix for placing the image
    fn draw_image(&mut self, image: ImageData, transform: &[f64; 6]) -> PDFResult<()>;

    /// Mark that subsequent drawing operations belong to a Type 3 glyph.
    ///
    /// Devices can use this to apply glyph-specific image sampling rules. The
    /// default implementation is a no-op for backends that do not need it.
    fn begin_type3_glyph(&mut self) {}

    /// Leave a Type 3 glyph drawing scope.
    fn end_type3_glyph(&mut self) {}

    /// Draw a shading (Type 2 axial / Type 3 radial gradient).
    ///
    /// The shading is already parsed and sampled in device space via `ctm`.
    fn draw_shading(
        &mut self,
        shading: &crate::rendering::shading::Shading,
        ctm: &[f64; 6],
    ) -> PDFResult<()> {
        let _ = shading;
        let _ = ctm;
        Ok(())
    }

    /// Begin offscreen rendering: save current pixels, clear to transparent.
    fn begin_offscreen(&mut self) -> PDFResult<()> {
        Ok(())
    }

    /// End offscreen rendering: extract mask from rendered content, restore pixels.
    /// Returns (mask_data, width, height) or None if not supported.
    fn end_offscreen_as_mask(
        &mut self,
        _luminosity: bool,
        _backdrop: Option<[u8; 3]>,
    ) -> Option<(Vec<u8>, u32, u32)> {
        None
    }

    /// End offscreen rendering and extract the top-left RGBA region, restoring pixels.
    fn end_offscreen_as_rgba_region(&mut self, _width: u32, _height: u32) -> Option<Vec<u8>> {
        None
    }

    /// End offscreen rendering and composite the rendered group over the saved backdrop.
    fn end_offscreen_composite(&mut self, _opacity: f64) -> PDFResult<()> {
        Ok(())
    }

    /// Set or clear the soft mask for subsequent draw operations.
    fn set_soft_mask(&mut self, _mask: Option<(&[u8], u32, u32)>) -> PDFResult<()> {
        Ok(())
    }

    /// Set the fill opacity (from ExtGState /ca).
    fn set_opacity(&mut self, _opacity: f64) {}

    /// Whether this device can fill paths with a repeated RGBA pattern cell.
    fn supports_pattern_cell_fill(&self) -> bool {
        false
    }

    /// Fill the current path with a repeated RGBA pattern cell.
    fn fill_path_with_pattern_cell(
        &mut self,
        _rule: FillRule,
        _rgba: &[u8],
        _width: u32,
        _height: u32,
        _image_to_device: &[f64; 6],
        _pattern_to_device: &[f64; 6],
        _bbox: &[f64; 4],
        _x_step: f64,
        _y_step: f64,
        _tile_bounds: (i32, i32, i32, i32),
    ) -> PDFResult<bool> {
        Ok(false)
    }

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

    /// Resolve a non-embedded standard font before the core bundled fallback is used.
    ///
    /// Core deliberately performs no filesystem or platform font lookup here. Native
    /// adapters may provide system font bytes; WASM adapters can keep the default
    /// `None` and let the deterministic bundled fallback handle Base14 fonts.
    fn resolve_standard_font_data(&mut self, _base_font: &str) -> Option<Vec<u8>> {
        None
    }

    /// Set per-character width metrics for a loaded font.
    ///
    /// Width values are expressed in glyph space units (typically 1000 units/em
    /// for PDF simple fonts). Implementations can use this to improve text
    /// advance accuracy for fonts whose native width tables are unavailable.
    ///
    /// # Arguments
    /// * `name` - The loaded font name/identifier (e.g. "F1")
    /// * `first_char` - First character code corresponding to `widths[0]`
    /// * `widths` - Width array from the PDF font dictionary
    /// * `default_width` - Width fallback for codes not present in `widths`
    fn set_font_widths(
        &mut self,
        name: &str,
        first_char: u32,
        widths: &[f64],
        default_width: f64,
    ) -> PDFResult<()> {
        let _ = name;
        let _ = first_char;
        let _ = widths;
        let _ = default_width;
        Ok(())
    }

    /// Request that a loaded composite font be rendered from raw CIDs.
    ///
    /// Backends that can address embedded CID fonts by glyph id can use this
    /// when `/Encoding /Identity-H` and `/CIDToGIDMap /Identity` make the PDF
    /// character codes usable as glyph ids. Text extraction still uses
    /// `/ToUnicode`; this hook is only for rendering.
    fn set_font_raw_cid_codes(&mut self, _name: &str, _enabled: bool) -> PDFResult<()> {
        Ok(())
    }

    /// Set CID vertical-writing metrics for a loaded composite font.
    ///
    /// `widths2` entries are `(cid, w1, v1, v2)` from the PDF `/W2` array.
    /// `dw2` is `(v1, w1)` from `/DW2`, matching the PDF array order.
    fn set_font_cid_vertical_metrics(
        &mut self,
        _name: &str,
        _vertical: bool,
        _dw2: (f64, f64),
        _widths2: &[(u32, f64, f64, f64)],
    ) -> PDFResult<()> {
        Ok(())
    }

    /// Set `/CIDToGIDMap` for CIDFontType2 fonts.
    ///
    /// Entries are indexed by CID and contain the TrueType glyph id. Missing
    /// entries map to `.notdef` just like hayro/PDFium.
    fn set_font_cid_to_gid_map(&mut self, _name: &str, _map: &[u16]) -> PDFResult<()> {
        Ok(())
    }

    /// Check whether a font has already been loaded into the device.
    ///
    /// Called by the rendering context before extracting and parsing font data,
    /// so that fonts loaded in a previous render are reused without re-parsing.
    /// Devices that don't support text rendering return `false` (default).
    fn has_font(&self, _name: &str) -> bool {
        false
    }

    /// Whether this device expects composite font bytes to remain as raw CIDs.
    ///
    /// Some backends can draw CID-keyed embedded fonts directly by CID. In that
    /// case /ToUnicode must not be used for rendering, since it maps character
    /// codes to semantic Unicode rather than glyph ids.
    fn font_wants_raw_cid_codes(&self, _name: &str) -> bool {
        false
    }

    /// Whether the named font is currently in vertical writing mode.
    ///
    /// Used by the interpreter for operators that advance the text matrix
    /// differently in vertical vs horizontal mode (e.g. `TJ` number entries).
    /// Matches PDF 32000-1:2008 §9.4.4 and hayro's `font_horizontal()` check.
    fn font_is_vertical(&self, _name: &str) -> bool {
        false
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
    /// Simulated vertical writing mode (Identity-V) for unit tests.
    vertical_writing: bool,
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
            vertical_writing: false,
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

    /// Simulate vertical writing mode for `TJ` adjustment tests.
    pub fn set_vertical_writing(&mut self, vertical: bool) {
        self.vertical_writing = vertical;
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

    fn clip_stroke_path(&mut self, _stroke_props: &StrokeProps) -> PDFResult<()> {
        self.operations.push("clip_stroke_path".to_string());
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
        horizontal_scaling: f64,
        _text_rise: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> PDFResult<f64> {
        self.operations.push(format!(
            "draw_text({}, {}, {:?}, hscale={}, tc={}, tw={})",
            font_name, font_size, text_bytes, horizontal_scaling, char_spacing, word_spacing
        ));
        // Return approximate width for testing with Tc/Tw support.
        let num_chars = text_bytes.len() as f64;
        let num_spaces = text_bytes.iter().filter(|&&b| b == b' ').count() as f64;
        let glyph_width = (num_chars * 500.0 * font_size) / 1000.0;
        let spacing = char_spacing * num_chars + word_spacing * num_spaces;
        let width = (glyph_width + spacing) * (horizontal_scaling / 100.0);
        Ok(width)
    }

    fn font_is_vertical(&self, _name: &str) -> bool {
        self.vertical_writing
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
