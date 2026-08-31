//! Graphics state management for PDF rendering.
//!
//! This module handles the graphics state stack and all state properties
//! as defined in the PDF specification (section 8.4).

/// Line cap style (PDF spec 8.4.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    /// Butt cap (default) - stroke is squared off at the endpoint
    Butt = 0,
    /// Round cap - semicircular arc with center at endpoint
    Round = 1,
    /// Projecting square cap - stroke continues beyond endpoint
    ProjectingSquare = 2,
}

impl Default for LineCap {
    fn default() -> Self {
        LineCap::Butt
    }
}

/// Line join style (PDF spec 8.4.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoin {
    /// Miter join (default) - outer edges meet at a sharp point
    Miter = 0,
    /// Round join - circular arc between the edges
    Round = 1,
    /// Bevel join - outer edges meet at a beveled edge
    Bevel = 2,
}

impl Default for LineJoin {
    fn default() -> Self {
        LineJoin::Miter
    }
}

/// Stroke properties for path rendering.
#[derive(Debug, Clone)]
pub struct StrokeProps {
    /// Line width in user space units (default: 1.0)
    pub line_width: f64,

    /// Line cap style (default: Butt)
    pub line_cap: LineCap,

    /// Line join style (default: Miter)
    pub line_join: LineJoin,

    /// Miter limit (default: 10.0)
    /// The maximum ratio of miter length to line width before bevel is used
    pub miter_limit: f64,

    /// Dash pattern - array of dash lengths alternating on/off
    pub dash_array: Vec<f64>,

    /// Dash phase - offset into the dash pattern (default: 0)
    pub dash_offset: f64,

    /// Minimum stroke width in device pixels used by the raster backend.
    pub min_device_width: f64,
}

impl Default for StrokeProps {
    fn default() -> Self {
        StrokeProps {
            line_width: 1.0,
            line_cap: LineCap::default(),
            line_join: LineJoin::default(),
            miter_limit: 10.0,
            dash_array: Vec::new(),
            dash_offset: 0.0,
            min_device_width: 0.2,
        }
    }
}

/// Color in a specific color space.
///
/// For simplicity, we currently only support DeviceGray, DeviceRGB, and DeviceCMYK.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    /// Grayscale color (1 component: 0.0 = black, 1.0 = white)
    Gray(f64),
    /// RGB color (3 components: each 0.0-1.0)
    RGB(f64, f64, f64),
    /// CMYK color (4 components: each 0.0-1.0)
    CMYK(f64, f64, f64, f64),
}

impl Color {
    /// Create a black color (default stroke/fill color in PDF)
    pub fn black() -> Self {
        Color::Gray(0.0)
    }

    /// Create a white color
    pub fn white() -> Self {
        Color::Gray(1.0)
    }

    /// Create a red color
    pub fn red() -> Self {
        Color::RGB(1.0, 0.0, 0.0)
    }

    /// Create a green color
    pub fn green() -> Self {
        Color::RGB(0.0, 1.0, 0.0)
    }

    /// Create a blue color
    pub fn blue() -> Self {
        Color::RGB(0.0, 0.0, 1.0)
    }

    /// Create an RGB color from u8 values (0-255).
    ///
    /// This is a convenience method for creating colors with byte values
    /// instead of normalized floats.
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::RGB(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
    }

    /// Get RGBA components as u8 values.
    ///
    /// Returns (r, g, b, a) where each component is 0-255.
    pub fn rgba(&self) -> (u8, u8, u8, u8) {
        match self {
            Color::Gray(g) => {
                let v = (g.clamp(0.0, 1.0) * 255.0) as u8;
                (v, v, v, 255)
            }
            Color::RGB(r, g, b) => (
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
                255,
            ),
            Color::CMYK(c, m, y, k) => {
                #[cfg(feature = "rendering")]
                {
                    return {
                        let (r, g, b) = crate::rendering::cmyk_icc::cmyk_pixel_to_rgb(
                            *c as f32, *m as f32, *y as f32, *k as f32,
                        );
                        (r, g, b, 255)
                    };
                }
                #[cfg(not(feature = "rendering"))]
                {
                    cmyk_polynomial(*c, *m, *y, *k)
                }
            }
        }
    }

    /// Get the red component as u8 (0-255).
    pub fn r(&self) -> u8 {
        self.rgba().0
    }

    /// Get the green component as u8 (0-255).
    pub fn g(&self) -> u8 {
        self.rgba().1
    }

    /// Get the blue component as u8 (0-255).
    pub fn b(&self) -> u8 {
        self.rgba().2
    }

    /// Get the alpha component as u8 (always 255 for now).
    pub fn a(&self) -> u8 {
        self.rgba().3
    }
}

impl Default for Color {
    fn default() -> Self {
        Color::black()
    }
}

/// Text rendering mode (PDF spec 9.3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRenderingMode {
    /// Fill text (default)
    Fill = 0,
    /// Stroke text
    Stroke = 1,
    /// Fill and stroke text
    FillStroke = 2,
    /// Invisible text (don't display)
    Invisible = 3,
    /// Fill text and add to path for clipping
    FillClip = 4,
    /// Stroke text and add to path for clipping
    StrokeClip = 5,
    /// Fill and stroke text and add to path for clipping
    FillStrokeClip = 6,
    /// Add text to path for clipping
    Clip = 7,
}

impl Default for TextRenderingMode {
    fn default() -> Self {
        TextRenderingMode::Fill
    }
}

#[cfg(not(feature = "rendering"))]
pub(crate) fn cmyk_polynomial(c: f64, m: f64, y: f64, k: f64) -> (u8, u8, u8, u8) {
    let c = c.clamp(0.0, 1.0);
    let m = m.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let k = k.clamp(0.0, 1.0);

    let r = 255.0
        + c * (-4.387332384609988 * c
            + 54.48615194189176 * m
            + 18.82290502165302 * y
            + 212.25662451639585 * k
            - 285.2331026137004)
        + m * (1.7149763477362134 * m
            - 5.6096736904047315 * y
            - 17.873870861415444 * k
            - 5.497006427196366)
        + y * (-2.5217340131683033 * y - 21.248923337353073 * k + 17.5119270841813)
        + k * (-21.86122147463605 * k - 189.48180835922747);

    let g = 255.0
        + c * (8.841041422036149 * c
            + 60.118027045597366 * m
            + 6.871425592049007 * y
            + 31.159100130055922 * k
            - 79.2970844816548)
        + m * (-15.310361306967817 * m + 17.575251261109482 * y + 131.35250912493976 * k
            - 190.9453302588951)
        + y * (4.444339102852739 * y + 9.8632861493405 * k - 24.86741582555878)
        + k * (-20.737325471181034 * k - 187.80453709719578);

    let b = 255.0
        + c * (0.8842522430003296 * c + 8.078677503112928 * m + 30.89978309703729 * y
            - 0.23883238689178934 * k
            - 14.183576799673286)
        + m * (10.49593273432072 * m + 63.02378494754052 * y + 50.606957656360734 * k
            - 112.23884253719248)
        + y * (0.03296041114873217 * y + 115.60384449646641 * k - 193.58209356861505)
        + k * (-22.33816807309886 * k - 180.12613974708367);

    (
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
        255,
    )
}

/// Fill rule for path filling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    /// Nonzero winding number rule (default for most operations)
    NonZero,
    /// Even-odd rule
    EvenOdd,
}

impl Default for FillRule {
    fn default() -> Self {
        FillRule::NonZero
    }
}

/// Blend mode for compositing painting operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    /// Normal source-over compositing.
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Default for BlendMode {
    fn default() -> Self {
        BlendMode::Normal
    }
}

/// Graphics state for PDF rendering.
///
/// This represents the current graphics state as defined in the PDF specification.
/// It includes transformation matrix, colors, line properties, and text state.
#[derive(Debug, Clone)]
pub struct GraphicsState {
    /// Current Transformation Matrix (CTM) - 6-element array [a b c d e f]
    /// representing the affine transform:
    /// | a c e |
    /// | b d f |
    /// | 0 0 1 |
    pub ctm: [f64; 6],

    /// Stroke color
    pub stroke_color: Color,

    /// Fill color
    pub fill_color: Color,

    /// Current blend mode from ExtGState (`/BM`), defaults to Normal.
    pub blend_mode: BlendMode,

    /// Fill alpha from ExtGState (`/ca`), defaults to 1.0.
    pub fill_alpha: f64,

    /// Stroke alpha from ExtGState (`/CA`), defaults to 1.0.
    pub stroke_alpha: f64,

    /// Stroke properties
    pub stroke_props: StrokeProps,

    /// Text rendering mode
    pub text_rendering_mode: TextRenderingMode,

    /// Text leading (for T* and TD operators) in user space units
    pub text_leading: f64,

    /// Text rise (for Ts operator) in user space units
    pub text_rise: f64,

    /// Character spacing (for Tc operator) in user space units
    pub character_spacing: f64,

    /// Word spacing (for Tw operator) in user space units
    pub word_spacing: f64,

    /// Horizontal text scaling (for Tz operator) as percentage (default: 100)
    pub text_horizontal_scaling: f64,

    /// Current text matrix (Tm)
    pub text_matrix: [f64; 6],

    /// Current text line matrix (Tlm)
    pub text_line_matrix: [f64; 6],

    /// Current font name (reference to font in resources)
    pub font_name: Option<String>,

    /// Current font size
    pub font_size: Option<f64>,
}

impl Default for GraphicsState {
    fn default() -> Self {
        GraphicsState {
            // Identity matrix
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            stroke_color: Color::black(),
            fill_color: Color::black(),
            blend_mode: BlendMode::default(),
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            stroke_props: StrokeProps::default(),
            text_rendering_mode: TextRenderingMode::default(),
            text_leading: 0.0,
            text_rise: 0.0,
            character_spacing: 0.0,
            word_spacing: 0.0,
            text_horizontal_scaling: 100.0,
            text_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            text_line_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            font_name: None,
            font_size: None,
        }
    }
}

impl GraphicsState {
    /// Create a new graphics state with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Save a copy of this state.
    pub fn save(&self) -> Self {
        self.clone()
    }

    /// Concatenate a transformation matrix to the CTM.
    ///
    /// # Arguments
    /// * `transform` - 6-element array [a b c d e f] representing the matrix to concatenate
    pub fn concat_matrix(&mut self, transform: &[f64; 6]) {
        // Matrix multiplication: CTM = CTM * transform
        let [a, b, c, d, e, f] = *transform;
        let [ctm_a, ctm_b, ctm_c, ctm_d, ctm_e, ctm_f] = self.ctm;

        self.ctm = [
            ctm_a * a + ctm_c * b,
            ctm_b * a + ctm_d * b,
            ctm_a * c + ctm_c * d,
            ctm_b * c + ctm_d * d,
            ctm_a * e + ctm_c * f + ctm_e,
            ctm_b * e + ctm_d * f + ctm_f,
        ];
    }

    /// Set the CTM to a specific matrix.
    pub fn set_matrix(&mut self, matrix: &[f64; 6]) {
        self.ctm = *matrix;
    }

    /// Reset the CTM to identity.
    pub fn reset_matrix(&mut self) {
        self.ctm = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }

    /// Transform a point by the CTM.
    ///
    /// # Arguments
    /// * `x` - X coordinate in user space
    /// * `y` - Y coordinate in user space
    ///
    /// # Returns
    /// Transformed (x, y) coordinates
    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.ctm;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// Set the text matrix.
    pub fn set_text_matrix(&mut self, matrix: &[f64; 6]) {
        self.text_matrix = *matrix;
        self.text_line_matrix = *matrix;
    }

    /// Get the text position from the text matrix.
    pub fn text_position(&self) -> (f64, f64) {
        (self.text_matrix[4], self.text_matrix[5])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state() {
        let state = GraphicsState::default();
        assert_eq!(state.ctm, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(state.stroke_color, Color::black());
        assert_eq!(state.fill_color, Color::black());
        assert_eq!(state.stroke_props.line_width, 1.0);
    }

    #[test]
    fn test_concat_matrix() {
        let mut state = GraphicsState::default();

        // Translate by (10, 20)
        state.concat_matrix(&[1.0, 0.0, 0.0, 1.0, 10.0, 20.0]);
        assert_eq!(state.ctm, [1.0, 0.0, 0.0, 1.0, 10.0, 20.0]);

        // Scale by 2x
        state.concat_matrix(&[2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
        // The correct result of [1,0,0,1,10,20] * [2,0,0,2,0,0] is [2,0,0,2,10,20]
        assert_eq!(state.ctm, [2.0, 0.0, 0.0, 2.0, 10.0, 20.0]);
    }

    #[test]
    fn test_transform_point() {
        let state = GraphicsState::default();

        // Identity transform
        assert_eq!(state.transform_point(10.0, 20.0), (10.0, 20.0));

        // Translate by (5, 10)
        let mut state = GraphicsState::default();
        state.concat_matrix(&[1.0, 0.0, 0.0, 1.0, 5.0, 10.0]);
        assert_eq!(state.transform_point(10.0, 20.0), (15.0, 30.0));

        // Scale by 2x
        let mut state = GraphicsState::default();
        state.concat_matrix(&[2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
        assert_eq!(state.transform_point(10.0, 20.0), (20.0, 40.0));
    }

    #[test]
    fn test_save_restore() {
        let mut state = GraphicsState::default();
        state.stroke_color = Color::red();
        state.stroke_props.line_width = 5.0;

        let saved = state.save();

        // Modify state
        state.stroke_color = Color::blue();
        state.stroke_props.line_width = 10.0;

        assert_eq!(state.stroke_color, Color::blue());
        assert_eq!(state.stroke_props.line_width, 10.0);

        // Restore would use the saved state
        assert_eq!(saved.stroke_color, Color::red());
        assert_eq!(saved.stroke_props.line_width, 5.0);
    }

    #[test]
    fn test_text_matrix() {
        let mut state = GraphicsState::default();
        state.set_text_matrix(&[1.0, 0.0, 0.0, 1.0, 100.0, 200.0]);

        assert_eq!(state.text_position(), (100.0, 200.0));
    }

    #[test]
    fn test_stroke_props_default() {
        let props = StrokeProps::default();
        assert_eq!(props.line_width, 1.0);
        assert_eq!(props.line_cap, LineCap::Butt);
        assert_eq!(props.line_join, LineJoin::Miter);
        assert_eq!(props.miter_limit, 10.0);
        assert!(props.dash_array.is_empty());
        assert_eq!(props.dash_offset, 0.0);
    }

    #[cfg(feature = "rendering")]
    #[test]
    fn test_cmyk_to_rgb_icc() {
        assert_eq!(Color::CMYK(0.0, 0.0, 0.0, 0.0).rgba(), (255, 255, 255, 255));
        assert_eq!(Color::CMYK(0.0, 0.0, 0.0, 1.0).rgba(), (35, 31, 32, 255));
        assert_eq!(Color::CMYK(1.0, 0.0, 0.0, 0.0).rgba(), (0, 174, 239, 255));
        assert_eq!(Color::CMYK(0.0, 1.0, 0.0, 0.0).rgba(), (236, 0, 140, 255));
        assert_eq!(Color::CMYK(0.0, 0.0, 1.0, 0.0).rgba(), (255, 234, 0, 255));
    }

    #[cfg(not(feature = "rendering"))]
    #[test]
    fn test_cmyk_to_rgb_polynomial() {
        assert_eq!(Color::CMYK(0.0, 0.0, 0.0, 0.0).rgba(), (255, 255, 255, 255));
        assert_eq!(Color::CMYK(0.0, 0.0, 0.0, 1.0).rgba(), (43, 46, 52, 255));
        assert_eq!(Color::CMYK(1.0, 0.0, 0.0, 0.0).rgba(), (0, 184, 241, 255));
        assert_eq!(Color::CMYK(0.0, 1.0, 0.0, 0.0).rgba(), (251, 48, 153, 255));
        assert_eq!(Color::CMYK(0.0, 0.0, 1.0, 0.0).rgba(), (255, 234, 61, 255));
    }
}
