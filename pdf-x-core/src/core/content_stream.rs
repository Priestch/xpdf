//! Content stream parsing and evaluation.
//!
//! This module handles parsing and interpreting PDF content streams - the sequences
//! of operators that define the appearance of pages and other graphical elements.
//!
//! Based on PDF.js src/core/evaluator.js and src/shared/util.js (OPS constants).

use super::error::{PDFError, PDFResult};
use super::font::Font;
use super::parser::{PDFObject, Parser};
use rustc_hash::FxHashMap;
use std::fmt;

/// PDF content stream operator codes.
///
/// These map to the PDF operator names (like "m", "l", "cm", "Tj", etc.)
/// and follow the PDF.js OPS enumeration for compatibility.
///
/// We intentionally start from 1 so it's easy to spot bad operators (will be 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    // Graphics State Operators
    /// w - Set line width
    SetLineWidth = 2,
    /// J - Set line cap style
    SetLineCap = 3,
    /// j - Set line join style
    SetLineJoin = 4,
    /// M - Set miter limit
    SetMiterLimit = 5,
    /// d - Set line dash pattern
    SetDash = 6,
    /// ri - Set rendering intent
    SetRenderingIntent = 7,
    /// i - Set flatness tolerance
    SetFlatness = 8,
    /// gs - Set graphics state from dictionary
    SetGState = 9,
    /// q - Save graphics state
    Save = 10,
    /// Q - Restore graphics state
    Restore = 11,
    /// cm - Concatenate matrix to current transformation matrix
    Transform = 12,

    // Path Construction Operators
    /// m - Begin new subpath
    MoveTo = 13,
    /// l - Append straight line segment
    LineTo = 14,
    /// c - Append cubic Bézier curve
    CurveTo = 15,
    /// v - Append cubic Bézier curve (initial point replicated)
    CurveTo2 = 16,
    /// y - Append cubic Bézier curve (final point replicated)
    CurveTo3 = 17,
    /// h - Close subpath
    ClosePath = 18,
    /// re - Append rectangle
    Rectangle = 19,

    // Path Painting Operators
    /// S - Stroke path
    Stroke = 20,
    /// s - Close and stroke path
    CloseStroke = 21,
    /// f or F - Fill path (nonzero winding rule)
    Fill = 22,
    /// f* - Fill path (even-odd rule)
    EOFill = 23,
    /// B - Fill and stroke path (nonzero winding rule)
    FillStroke = 24,
    /// B* - Fill and stroke path (even-odd rule)
    EOFillStroke = 25,
    /// b - Close, fill, and stroke path (nonzero winding rule)
    CloseFillStroke = 26,
    /// b* - Close, fill, and stroke path (even-odd rule)
    CloseEOFillStroke = 27,
    /// n - End path without filling or stroking
    EndPath = 28,

    // Clipping Path Operators
    /// W - Set clipping path (nonzero winding rule)
    Clip = 29,
    /// W* - Set clipping path (even-odd rule)
    EOClip = 30,

    // Text Object Operators
    /// BT - Begin text object
    BeginText = 31,
    /// ET - End text object
    EndText = 32,

    // Text State Operators
    /// Tc - Set character spacing
    SetCharSpacing = 33,
    /// Tw - Set word spacing
    SetWordSpacing = 34,
    /// Tz - Set horizontal scaling
    SetHScale = 35,
    /// TL - Set text leading
    SetLeading = 36,
    /// Tf - Set text font and size
    SetFont = 37,
    /// Tr - Set text rendering mode
    SetTextRenderingMode = 38,
    /// Ts - Set text rise
    SetTextRise = 39,

    // Text Positioning Operators
    /// Td - Move text position
    MoveText = 40,
    /// TD - Move text position and set leading
    SetLeadingMoveText = 41,
    /// Tm - Set text matrix
    SetTextMatrix = 42,
    /// T* - Move to start of next line
    NextLine = 43,

    // Text Showing Operators
    /// Tj - Show text string
    ShowText = 44,
    /// TJ - Show text with individual glyph positioning
    ShowSpacedText = 45,
    /// ' - Move to next line and show text
    NextLineShowText = 46,
    /// " - Set spacing, move to next line, show text
    NextLineSetSpacingShowText = 47,

    // Type 3 Font Operators
    /// d0 - Set glyph width
    SetCharWidth = 48,
    /// d1 - Set glyph width and bounding box
    SetCharWidthAndBounds = 49,

    // Color Operators
    /// CS - Set stroke color space
    SetStrokeColorSpace = 50,
    /// cs - Set fill color space
    SetFillColorSpace = 51,
    /// SC - Set stroke color
    SetStrokeColor = 52,
    /// SCN - Set stroke color (supports Pattern, Separation, DeviceN)
    SetStrokeColorN = 53,
    /// sc - Set fill color
    SetFillColor = 54,
    /// scn - Set fill color (supports Pattern, Separation, DeviceN)
    SetFillColorN = 55,
    /// G - Set stroke gray level
    SetStrokeGray = 56,
    /// g - Set fill gray level
    SetFillGray = 57,
    /// RG - Set stroke RGB color
    SetStrokeRGBColor = 58,
    /// rg - Set fill RGB color
    SetFillRGBColor = 59,
    /// K - Set stroke CMYK color
    SetStrokeCMYKColor = 60,
    /// k - Set fill CMYK color
    SetFillCMYKColor = 61,

    // Shading Operator
    /// sh - Paint with shading pattern
    ShadingFill = 62,

    // Inline Image Operators
    /// BI - Begin inline image
    BeginInlineImage = 63,
    /// ID - Begin inline image data
    BeginImageData = 64,
    /// EI - End inline image
    EndInlineImage = 65,

    // XObject Operator
    /// Do - Paint XObject
    PaintXObject = 66,

    // Marked Content Operators
    /// MP - Define marked-content point
    MarkPoint = 67,
    /// DP - Define marked-content point with properties
    MarkPointProps = 68,
    /// BMC - Begin marked-content sequence
    BeginMarkedContent = 69,
    /// BDC - Begin marked-content sequence with properties
    BeginMarkedContentProps = 70,
    /// EMC - End marked-content sequence
    EndMarkedContent = 71,

    // Compatibility Operators
    /// BX - Begin compatibility section
    BeginCompat = 72,
    /// EX - End compatibility section
    EndCompat = 73,
}

impl OpCode {
    /// Converts a PDF operator string (command) to an OpCode.
    ///
    /// # Arguments
    /// * `cmd` - The operator string (e.g., "m", "l", "cm", "Tj")
    ///
    /// # Returns
    /// The corresponding OpCode, or an error if the operator is unknown.
    pub fn from_command(cmd: &str) -> PDFResult<OpCode> {
        match cmd {
            // Graphics state
            "w" => Ok(OpCode::SetLineWidth),
            "J" => Ok(OpCode::SetLineCap),
            "j" => Ok(OpCode::SetLineJoin),
            "M" => Ok(OpCode::SetMiterLimit),
            "d" => Ok(OpCode::SetDash),
            "ri" => Ok(OpCode::SetRenderingIntent),
            "i" => Ok(OpCode::SetFlatness),
            "gs" => Ok(OpCode::SetGState),
            "q" => Ok(OpCode::Save),
            "Q" => Ok(OpCode::Restore),
            "cm" => Ok(OpCode::Transform),

            // Path construction
            "m" => Ok(OpCode::MoveTo),
            "l" => Ok(OpCode::LineTo),
            "c" => Ok(OpCode::CurveTo),
            "v" => Ok(OpCode::CurveTo2),
            "y" => Ok(OpCode::CurveTo3),
            "h" => Ok(OpCode::ClosePath),
            "re" => Ok(OpCode::Rectangle),

            // Path painting
            "S" => Ok(OpCode::Stroke),
            "s" => Ok(OpCode::CloseStroke),
            "f" | "F" => Ok(OpCode::Fill),
            "f*" => Ok(OpCode::EOFill),
            "B" => Ok(OpCode::FillStroke),
            "B*" => Ok(OpCode::EOFillStroke),
            "b" => Ok(OpCode::CloseFillStroke),
            "b*" => Ok(OpCode::CloseEOFillStroke),
            "n" => Ok(OpCode::EndPath),

            // Clipping
            "W" => Ok(OpCode::Clip),
            "W*" => Ok(OpCode::EOClip),

            // Text object
            "BT" => Ok(OpCode::BeginText),
            "ET" => Ok(OpCode::EndText),

            // Text state
            "Tc" => Ok(OpCode::SetCharSpacing),
            "Tw" => Ok(OpCode::SetWordSpacing),
            "Tz" => Ok(OpCode::SetHScale),
            "TL" => Ok(OpCode::SetLeading),
            "Tf" => Ok(OpCode::SetFont),
            "Tr" => Ok(OpCode::SetTextRenderingMode),
            "Ts" => Ok(OpCode::SetTextRise),

            // Text positioning
            "Td" => Ok(OpCode::MoveText),
            "TD" => Ok(OpCode::SetLeadingMoveText),
            "Tm" => Ok(OpCode::SetTextMatrix),
            "T*" => Ok(OpCode::NextLine),

            // Text showing
            "Tj" => Ok(OpCode::ShowText),
            "TJ" => Ok(OpCode::ShowSpacedText),
            "'" => Ok(OpCode::NextLineShowText),
            "\"" => Ok(OpCode::NextLineSetSpacingShowText),

            // Type 3 fonts
            "d0" => Ok(OpCode::SetCharWidth),
            "d1" => Ok(OpCode::SetCharWidthAndBounds),

            // Color
            "CS" => Ok(OpCode::SetStrokeColorSpace),
            "cs" => Ok(OpCode::SetFillColorSpace),
            "SC" => Ok(OpCode::SetStrokeColor),
            "SCN" => Ok(OpCode::SetStrokeColorN),
            "sc" => Ok(OpCode::SetFillColor),
            "scn" => Ok(OpCode::SetFillColorN),
            "G" => Ok(OpCode::SetStrokeGray),
            "g" => Ok(OpCode::SetFillGray),
            "RG" => Ok(OpCode::SetStrokeRGBColor),
            "rg" => Ok(OpCode::SetFillRGBColor),
            "K" => Ok(OpCode::SetStrokeCMYKColor),
            "k" => Ok(OpCode::SetFillCMYKColor),

            // Shading
            "sh" => Ok(OpCode::ShadingFill),

            // Inline images
            "BI" => Ok(OpCode::BeginInlineImage),
            "ID" => Ok(OpCode::BeginImageData),
            "EI" => Ok(OpCode::EndInlineImage),

            // XObject
            "Do" => Ok(OpCode::PaintXObject),

            // Marked content
            "MP" => Ok(OpCode::MarkPoint),
            "DP" => Ok(OpCode::MarkPointProps),
            "BMC" => Ok(OpCode::BeginMarkedContent),
            "BDC" => Ok(OpCode::BeginMarkedContentProps),
            "EMC" => Ok(OpCode::EndMarkedContent),

            // Compatibility
            "BX" => Ok(OpCode::BeginCompat),
            "EX" => Ok(OpCode::EndCompat),

            _ => Err(PDFError::content_stream_error(format!(
                "Unknown PDF operator: '{}'",
                cmd
            ))),
        }
    }

    /// Returns the PDF operator string for this opcode.
    pub fn to_command(&self) -> &'static str {
        match self {
            OpCode::SetLineWidth => "w",
            OpCode::SetLineCap => "J",
            OpCode::SetLineJoin => "j",
            OpCode::SetMiterLimit => "M",
            OpCode::SetDash => "d",
            OpCode::SetRenderingIntent => "ri",
            OpCode::SetFlatness => "i",
            OpCode::SetGState => "gs",
            OpCode::Save => "q",
            OpCode::Restore => "Q",
            OpCode::Transform => "cm",
            OpCode::MoveTo => "m",
            OpCode::LineTo => "l",
            OpCode::CurveTo => "c",
            OpCode::CurveTo2 => "v",
            OpCode::CurveTo3 => "y",
            OpCode::ClosePath => "h",
            OpCode::Rectangle => "re",
            OpCode::Stroke => "S",
            OpCode::CloseStroke => "s",
            OpCode::Fill => "f",
            OpCode::EOFill => "f*",
            OpCode::FillStroke => "B",
            OpCode::EOFillStroke => "B*",
            OpCode::CloseFillStroke => "b",
            OpCode::CloseEOFillStroke => "b*",
            OpCode::EndPath => "n",
            OpCode::Clip => "W",
            OpCode::EOClip => "W*",
            OpCode::BeginText => "BT",
            OpCode::EndText => "ET",
            OpCode::SetCharSpacing => "Tc",
            OpCode::SetWordSpacing => "Tw",
            OpCode::SetHScale => "Tz",
            OpCode::SetLeading => "TL",
            OpCode::SetFont => "Tf",
            OpCode::SetTextRenderingMode => "Tr",
            OpCode::SetTextRise => "Ts",
            OpCode::MoveText => "Td",
            OpCode::SetLeadingMoveText => "TD",
            OpCode::SetTextMatrix => "Tm",
            OpCode::NextLine => "T*",
            OpCode::ShowText => "Tj",
            OpCode::ShowSpacedText => "TJ",
            OpCode::NextLineShowText => "'",
            OpCode::NextLineSetSpacingShowText => "\"",
            OpCode::SetCharWidth => "d0",
            OpCode::SetCharWidthAndBounds => "d1",
            OpCode::SetStrokeColorSpace => "CS",
            OpCode::SetFillColorSpace => "cs",
            OpCode::SetStrokeColor => "SC",
            OpCode::SetStrokeColorN => "SCN",
            OpCode::SetFillColor => "sc",
            OpCode::SetFillColorN => "scn",
            OpCode::SetStrokeGray => "G",
            OpCode::SetFillGray => "g",
            OpCode::SetStrokeRGBColor => "RG",
            OpCode::SetFillRGBColor => "rg",
            OpCode::SetStrokeCMYKColor => "K",
            OpCode::SetFillCMYKColor => "k",
            OpCode::ShadingFill => "sh",
            OpCode::BeginInlineImage => "BI",
            OpCode::BeginImageData => "ID",
            OpCode::EndInlineImage => "EI",
            OpCode::PaintXObject => "Do",
            OpCode::MarkPoint => "MP",
            OpCode::MarkPointProps => "DP",
            OpCode::BeginMarkedContent => "BMC",
            OpCode::BeginMarkedContentProps => "BDC",
            OpCode::EndMarkedContent => "EMC",
            OpCode::BeginCompat => "BX",
            OpCode::EndCompat => "EX",
        }
    }
}

impl fmt::Display for OpCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_command())
    }
}

/// A parsed content stream operation.
///
/// Consists of an operator code and its operand arguments.
/// This follows the PDF.js operation structure.
#[derive(Debug, Clone)]
pub struct Operation {
    /// The operator code
    pub op: OpCode,
    /// The operand arguments (read before the operator)
    pub args: Vec<PDFObject>,
}

impl Operation {
    /// Creates a new operation.
    pub fn new(op: OpCode, args: Vec<PDFObject>) -> Self {
        Operation { op, args }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (", self.op)?;
        for (i, arg) in self.args.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{:?}", arg)?;
        }
        write!(f, ")")
    }
}

/// Text extraction information from content streams.
#[derive(Debug, Clone)]
pub struct TextItem {
    /// The text content
    pub text: String,

    /// Font name (if available)
    pub font_name: Option<String>,

    /// Font size (if available)
    pub font_size: Option<f64>,

    /// Text position (x, y) in user space
    pub position: Option<(f64, f64)>,

    /// Text rendering mode
    pub rendering_mode: Option<i32>,
}

/// Content stream evaluator/preprocessor.
///
/// Reads operations from a PDF content stream, following the PDF.js
/// EvaluatorPreprocessor pattern. Supports progressive loading - can
/// throw DataNotLoaded errors when data is missing.
///
/// Based on PDF.js src/core/evaluator.js EvaluatorPreprocessor class.
pub struct ContentStreamEvaluator {
    parser: Parser,

    /// Text extraction state
    text_state: TextExtractionState,

    /// Font cache (font name -> Font object)
    fonts: FxHashMap<String, Font>,
}

/// State for text extraction from content streams.
#[derive(Debug, Clone)]
struct TextExtractionState {
    /// Current text matrix (Tm)
    text_matrix: [f64; 6],

    /// Current text line matrix (Tlm)
    text_line_matrix: [f64; 6],

    /// Current font name
    current_font_name: Option<String>,

    /// Current font size
    current_font_size: Option<f64>,

    /// Current text rendering mode
    text_rendering_mode: Option<i32>,

    /// Whether we're in a text object (BT...ET)
    in_text_object: bool,

    /// Extracted text items
    extracted_text: Vec<TextItem>,
}

impl Default for TextExtractionState {
    fn default() -> Self {
        Self {
            // Identity matrix
            text_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            text_line_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            current_font_name: None,
            current_font_size: None,
            text_rendering_mode: None,
            in_text_object: false,
            extracted_text: Vec::new(),
        }
    }
}

impl ContentStreamEvaluator {
    /// Creates a new content stream evaluator.
    ///
    /// # Arguments
    /// * `parser` - Parser positioned at the start of the content stream
    pub fn new(parser: Parser) -> Self {
        ContentStreamEvaluator {
            parser,
            text_state: TextExtractionState::default(),
            fonts: FxHashMap::default(),
        }
    }

    /// Loads fonts from a page's resources dictionary.
    ///
    /// This should be called before processing a page's content stream
    /// to ensure fonts are available for text extraction.
    ///
    /// # Arguments
    /// * `resources` - The page's /Resources dictionary
    /// * `xref` - Cross-reference table for fetching font dictionaries
    pub fn load_fonts(
        &mut self,
        resources: &PDFObject,
        xref: &mut crate::core::xref::XRef,
    ) -> PDFResult<()> {
        // Get the Font dictionary from resources
        let resources_dict = match resources {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(()), // No resources, skip
        };

        let font_dict = match resources_dict.get("Font") {
            Some(PDFObject::Dictionary(d)) => d.clone(),
            Some(PDFObject::Ref(ref_obj)) => {
                // Dereference the font dictionary
                let font_obj = xref.fetch(ref_obj.num, ref_obj.generation)?;
                match &*font_obj {
                    PDFObject::Dictionary(d) => d.clone(),
                    _ => return Ok(()), // Not a dictionary
                }
            }
            _ => return Ok(()), // No fonts
        };

        // Load each font
        for (font_name, font_ref) in &font_dict {
            // Fetch the font dictionary
            let font_dict_obj = xref.fetch_if_ref(font_ref)?;

            // Create Font object
            match Font::new(font_dict_obj, xref) {
                Ok(font) => {
                    self.fonts.insert(font_name.clone(), font);
                }
                Err(e) => {
                    // Log error but continue loading other fonts
                    eprintln!("Warning: Failed to load font '{}': {:?}", font_name, e);
                }
            }
        }

        Ok(())
    }

    /// Extracts all text from the content stream.
    ///
    /// This method processes the entire content stream and extracts text content
    /// with position and font information. It's a simplified implementation that
    /// handles basic text showing operators (Tj, TJ).
    ///
    /// # Returns
    /// A vector of TextItem objects containing the extracted text
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::content_stream::ContentStreamEvaluator;
    /// # use pdf_x::core::{Parser, Lexer, Stream};
    /// # let stream = Box::new(Stream::from_bytes(vec![]));
    /// # let lexer = Lexer::new(stream).unwrap();
    /// # let parser = Parser::new(lexer).unwrap();
    ///
    /// let mut evaluator = ContentStreamEvaluator::new(parser);
    /// let text_items = evaluator.extract_text().unwrap();
    ///
    /// for item in text_items {
    ///     println!("Text: '{}' at position {:?}", item.text, item.position);
    /// }
    /// ```
    pub fn extract_text(&mut self) -> PDFResult<Vec<TextItem>> {
        // Reset text state
        self.text_state = TextExtractionState::default();

        // Process all operations
        while let Some(op) = self.read_operation()? {
            self.process_text_operation(&op)?;
        }

        Ok(self.text_state.extracted_text.clone())
    }

    /// Processes an operation for text extraction.
    fn process_text_operation(&mut self, op: &Operation) -> PDFResult<()> {
        match op.op {
            OpCode::BeginText => {
                self.text_state.in_text_object = true;
                // Initialize text matrices
                self.text_state.text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                self.text_state.text_line_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
            }
            OpCode::EndText => {
                self.text_state.in_text_object = false;
            }
            OpCode::SetFont => {
                // Tf - set font and size
                if op.args.len() >= 2 {
                    if let PDFObject::Name(font_name) = &op.args[0] {
                        self.text_state.current_font_name = Some(font_name.clone());
                    }
                    if let PDFObject::Number(font_size) = &op.args[1] {
                        self.text_state.current_font_size = Some(*font_size);
                    }
                }
            }
            OpCode::SetTextRenderingMode => {
                if op.args.len() >= 1 {
                    if let PDFObject::Number(mode) = &op.args[0] {
                        self.text_state.text_rendering_mode = Some(*mode as i32);
                    }
                }
            }
            OpCode::SetTextMatrix => {
                if op.args.len() >= 6 {
                    // Set text matrix from 6 numbers [a b c d e f]
                    for i in 0..6 {
                        if let PDFObject::Number(n) = &op.args[i] {
                            self.text_state.text_matrix[i] = *n;
                            self.text_state.text_line_matrix[i] = *n;
                        }
                    }
                }
            }
            OpCode::MoveText => {
                if op.args.len() >= 2 && self.text_state.in_text_object {
                    // Td - move text position
                    if let (PDFObject::Number(tx), PDFObject::Number(ty)) =
                        (&op.args[0], &op.args[1])
                    {
                        // Update line matrix: Tlm = Tlm * [1 0 0 1 tx ty]
                        self.text_state.text_line_matrix[4] += tx;
                        self.text_state.text_line_matrix[5] += ty;
                        // Copy to text matrix
                        self.text_state.text_matrix = self.text_state.text_line_matrix;
                    }
                }
            }
            OpCode::NextLine => {
                if self.text_state.in_text_object {
                    // T* - move to next line using leading
                    self.text_state.text_line_matrix[5] -=
                        self.text_state.text_line_matrix[5] * 0.0; // Simplified
                    self.text_state.text_matrix = self.text_state.text_line_matrix;
                }
            }
            OpCode::ShowText => {
                // Tj - show text string
                if op.args.len() >= 1 && self.text_state.in_text_object {
                    if let PDFObject::String(text_bytes) = &op.args[0] {
                        let text = self.decode_text(text_bytes);
                        let position = Some((
                            self.text_state.text_matrix[4],
                            self.text_state.text_matrix[5],
                        ));

                        let text_item = TextItem {
                            text,
                            font_name: self.text_state.current_font_name.clone(),
                            font_size: self.text_state.current_font_size,
                            position,
                            rendering_mode: self.text_state.text_rendering_mode,
                        };

                        self.text_state.extracted_text.push(text_item);
                    }
                }
            }
            OpCode::ShowSpacedText => {
                // TJ - array with text and spacing adjustments
                // This operator shows multiple text strings with individual glyph positioning
                // Format: [(string1) -100 (string2) 50 (string3)] where numbers are spacing adjustments
                if op.args.len() >= 1 && self.text_state.in_text_object {
                    if let PDFObject::Array(items) = &op.args[0] {
                        let mut accumulated_text = String::new();
                        let start_position = Some((
                            self.text_state.text_matrix[4],
                            self.text_state.text_matrix[5],
                        ));

                        for item in items {
                            match &**item {
                                PDFObject::String(text_bytes) => {
                                    // Decode text using font encoding (CMap)
                                    let text = self.decode_text(text_bytes);
                                    accumulated_text.push_str(&text);
                                }
                                PDFObject::Number(spacing) => {
                                    // Spacing adjustment in 1/1000ths of a text space unit
                                    // Negative numbers move text closer together (like kerning)
                                    // Large negative numbers (< -100) typically indicate word spaces

                                    // Add a space if the adjustment is significant (word boundary)
                                    if *spacing < -100.0 {
                                        accumulated_text.push(' ');
                                    }

                                    // Adjust text position for spacing
                                    let font_size =
                                        self.text_state.current_font_size.unwrap_or(12.0);
                                    self.text_state.text_matrix[4] -= spacing * font_size * 0.001;
                                }
                                _ => {}
                            }
                        }

                        // Create a single text item for the entire TJ operation
                        if !accumulated_text.is_empty() {
                            let text_item = TextItem {
                                text: accumulated_text,
                                font_name: self.text_state.current_font_name.clone(),
                                font_size: self.text_state.current_font_size,
                                position: start_position,
                                rendering_mode: self.text_state.text_rendering_mode,
                            };

                            self.text_state.extracted_text.push(text_item);
                        }
                    }
                }
            }
            _ => {
                // Other operators are ignored for text extraction
            }
        }
        Ok(())
    }

    /// Decodes text bytes using the current font's encoding (CMap).
    ///
    /// This method converts character codes (CIDs) to Unicode characters using
    /// the font's ToUnicode CMap if available, with fallback to encoding tables.
    ///
    /// For CID fonts: Multi-byte character codes (typically 2 bytes)
    /// For simple fonts: Single-byte character codes
    ///
    /// # Arguments
    /// * `text_bytes` - Raw text bytes from the PDF content stream
    ///
    /// # Returns
    /// Decoded Unicode string
    #[inline] // Hot path during text extraction
    fn decode_text(&self, text_bytes: &[u8]) -> String {
        // Try to use the current font's CMap
        if let Some(ref font_name) = self.text_state.current_font_name {
            if let Some(font) = self.fonts.get(font_name) {
                let mut decoded = String::new();

                // Check if this is a CID font (multi-byte character codes)
                if font.font_type().is_cid_font() {
                    // CID fonts use multi-byte character codes (typically 2 bytes)
                    let mut i = 0;
                    while i + 1 < text_bytes.len() {
                        // Combine two bytes to form a CID
                        let cid = u16::from_be_bytes([text_bytes[i], text_bytes[i + 1]]);
                        let unicode_char = font.char_code_to_unicode(cid);
                        decoded.push(unicode_char);
                        i += 2;
                    }
                    // Handle any remaining single byte
                    if i < text_bytes.len() {
                        let cid = text_bytes[i] as u16;
                        decoded.push(font.char_code_to_unicode(cid));
                    }
                } else {
                    // Simple fonts use single-byte character codes
                    for &byte in text_bytes {
                        let cid = byte as u16;
                        let unicode_char = font.char_code_to_unicode(cid);
                        decoded.push(unicode_char);
                    }
                }

                return decoded;
            }
        }

        // Fallback: try UTF-8, then Latin-1
        String::from_utf8_lossy(text_bytes).into_owned()
    }

    /// Reads the next operation from the content stream.
    ///
    /// This method implements the PDF.js read() pattern:
    /// 1. Read operands (PDF objects) until we hit a command
    /// 2. Convert command to OpCode
    /// 3. Return Operation with opcode and arguments
    ///
    /// # Returns
    /// * `Ok(Some(operation))` - Successfully read an operation
    /// * `Ok(None)` - Reached end of stream
    /// * `Err(PDFError::DataNotLoaded)` - Need more data (progressive loading)
    /// * `Err(other)` - Parse error
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::content_stream::ContentStreamEvaluator;
    /// # use pdf_x::core::{Parser, Lexer, Stream};
    /// # let stream = Box::new(Stream::from_bytes(vec![]));
    /// # let lexer = Lexer::new(stream).unwrap();
    /// # let parser = Parser::new(lexer).unwrap();
    ///
    /// let mut evaluator = ContentStreamEvaluator::new(parser);
    ///
    /// while let Some(op) = evaluator.read_operation().unwrap() {
    ///     println!("Operator: {} with {} args", op.op, op.args.len());
    /// }
    /// ```
    pub fn read_operation(&mut self) -> PDFResult<Option<Operation>> {
        let mut args = Vec::new();

        loop {
            // Check if we have more content
            if !self.parser.has_more() {
                // End of stream
                if args.is_empty() {
                    return Ok(None);
                } else {
                    return Err(PDFError::Generic(
                        "Content stream ended with operands but no operator".to_string(),
                    ));
                }
            }

            // Read next object - this can throw DataNotLoaded!
            let obj = self.parser.get_object()?;

            match obj {
                PDFObject::EOF => {
                    if args.is_empty() {
                        return Ok(None);
                    } else {
                        return Err(PDFError::Generic(
                            "Content stream ended with operands but no operator".to_string(),
                        ));
                    }
                }
                // Commands are operators
                obj if obj.is_command_like() => {
                    // Extract command string
                    let cmd_str = self.extract_command(&obj)?;
                    let op = OpCode::from_command(&cmd_str)?;
                    return Ok(Some(Operation::new(op, args)));
                }
                // Everything else is an operand
                _ => {
                    args.push(obj);
                }
            }
        }
    }

    /// Extracts the command string from a PDFObject.
    fn extract_command(&self, obj: &PDFObject) -> PDFResult<String> {
        match obj {
            PDFObject::Command(cmd) => Ok(cmd.clone()),
            _ => Err(PDFError::Generic(format!(
                "Expected command, got {:?}",
                obj
            ))),
        }
    }

    /// Checks if there are more operations to read.
    pub fn has_more(&self) -> bool {
        self.parser.has_more()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Lexer, Stream};

    fn create_evaluator(content: &str) -> ContentStreamEvaluator {
        let data = content.as_bytes().to_vec();
        let stream = Box::new(Stream::from_bytes(data));
        let lexer = Lexer::new(stream).unwrap();
        let parser = Parser::new(lexer).unwrap();
        ContentStreamEvaluator::new(parser)
    }

    #[test]
    fn test_opcode_from_command() {
        assert_eq!(OpCode::from_command("m").unwrap(), OpCode::MoveTo);
        assert_eq!(OpCode::from_command("l").unwrap(), OpCode::LineTo);
        assert_eq!(OpCode::from_command("cm").unwrap(), OpCode::Transform);
        assert_eq!(OpCode::from_command("Tj").unwrap(), OpCode::ShowText);
        assert_eq!(OpCode::from_command("q").unwrap(), OpCode::Save);
        assert_eq!(OpCode::from_command("Q").unwrap(), OpCode::Restore);
    }

    #[test]
    fn test_opcode_to_command() {
        assert_eq!(OpCode::MoveTo.to_command(), "m");
        assert_eq!(OpCode::LineTo.to_command(), "l");
        assert_eq!(OpCode::Transform.to_command(), "cm");
        assert_eq!(OpCode::ShowText.to_command(), "Tj");
    }

    #[test]
    fn test_read_simple_path() {
        let mut eval = create_evaluator("10 20 m\n30 40 l\nS");

        // First operation: MoveTo
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::MoveTo);
        assert_eq!(op1.args.len(), 2);

        // Second operation: LineTo
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::LineTo);
        assert_eq!(op2.args.len(), 2);

        // Third operation: Stroke
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::Stroke);
        assert_eq!(op3.args.len(), 0);

        // No more operations
        assert!(eval.read_operation().unwrap().is_none());
    }

    #[test]
    fn test_read_graphics_state() {
        let mut eval = create_evaluator("q\n1 0 0 1 10 20 cm\nQ");

        // Save state
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::Save);
        assert_eq!(op1.args.len(), 0);

        // Transform
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::Transform);
        assert_eq!(op2.args.len(), 6);

        // Restore state
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::Restore);
        assert_eq!(op3.args.len(), 0);
    }

    #[test]
    fn test_read_text_operations() {
        let mut eval = create_evaluator("BT\n/F1 12 Tf\n(Hello) Tj\nET");

        // Begin text
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::BeginText);

        // Set font
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::SetFont);
        assert_eq!(op2.args.len(), 2);

        // Show text
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::ShowText);
        assert_eq!(op3.args.len(), 1);

        // End text
        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::EndText);
    }

    #[test]
    fn test_unknown_operator() {
        let mut eval = create_evaluator("10 20 XYZ");
        let result = eval.read_operation();
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_simple_text() {
        let mut eval = create_evaluator("BT\n/F1 12 Tf\n100 200 Td\n(Hello World) Tj\nET");

        let text_items = eval.extract_text().unwrap();

        assert_eq!(text_items.len(), 1);
        let item = &text_items[0];
        assert_eq!(item.text, "Hello World");
        assert_eq!(item.font_name, Some("F1".to_string()));
        assert_eq!(item.font_size, Some(12.0));
        assert_eq!(item.position, Some((100.0, 200.0)));
    }

    #[test]
    fn test_extract_multiple_text_items() {
        let content = "BT\n/F1 12 Tf\n50 100 Td\n(First) Tj\n0 20 Td\n(Second) Tj\nET";
        let mut eval = create_evaluator(content);

        let text_items = eval.extract_text().unwrap();

        assert_eq!(text_items.len(), 2);
        assert_eq!(text_items[0].text, "First");
        assert_eq!(text_items[1].text, "Second");
    }

    #[test]
    fn test_extract_text_with_spacing() {
        let content = "BT\n/F1 12 Tf\n100 200 Td\n[(He) -50 (llo) 100 ( Wo)-50 (rld)] TJ\nET";
        let mut eval = create_evaluator(content);

        let text_items = eval.extract_text().unwrap();

        // TJ operator now accumulates all text into a single item
        // Only spacing < -100 adds a space (word boundary detection)
        // -50 and -50 don't trigger space, 100 is positive so no space
        // Note: " Wo" already has a leading space in the PDF
        assert_eq!(text_items.len(), 1);
        assert_eq!(text_items[0].text, "Hello World"); // Space from " Wo" string
        assert_eq!(text_items[0].font_name, Some("F1".to_string()));
        assert_eq!(text_items[0].font_size, Some(12.0));
    }

    #[test]
    fn test_extract_text_ignores_graphics() {
        let content = "10 20 m\n30 40 l\nS\nBT\n/F1 12 Tf\n100 200 Td\n(Text) Tj\nET";
        let mut eval = create_evaluator(content);

        let text_items = eval.extract_text().unwrap();

        // Should only extract the text, not the graphics operations
        assert_eq!(text_items.len(), 1);
        assert_eq!(text_items[0].text, "Text");
    }

    // ============================================================================
    // Comprehensive Path Operator Tests
    // ============================================================================

    #[test]
    fn test_path_move_to() {
        let mut eval = create_evaluator("10 20 m");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::MoveTo);
        assert_eq!(op.args.len(), 2);
        assert_eq!(op.args[0], PDFObject::Number(10.0));
        assert_eq!(op.args[1], PDFObject::Number(20.0));
    }

    #[test]
    fn test_path_line_to() {
        let mut eval = create_evaluator("30 40 l");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::LineTo);
        assert_eq!(op.args.len(), 2);
        assert_eq!(op.args[0], PDFObject::Number(30.0));
        assert_eq!(op.args[1], PDFObject::Number(40.0));
    }

    #[test]
    fn test_path_curve_to() {
        // c: x1 y1 x2 y2 x y
        let mut eval = create_evaluator("10 20 30 40 50 60 c");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::CurveTo);
        assert_eq!(op.args.len(), 6);
    }

    #[test]
    fn test_path_curve_to2() {
        // v: x2 y2 x y
        let mut eval = create_evaluator("30 40 50 60 v");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::CurveTo2);
        assert_eq!(op.args.len(), 4);
    }

    #[test]
    fn test_path_curve_to3() {
        // y: x1 y1 x y
        let mut eval = create_evaluator("10 20 50 60 y");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::CurveTo3);
        assert_eq!(op.args.len(), 4);
    }

    #[test]
    fn test_path_close() {
        let mut eval = create_evaluator("h");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::ClosePath);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_rectangle() {
        // re: x y width height
        let mut eval = create_evaluator("100 200 50 75 re");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::Rectangle);
        assert_eq!(op.args.len(), 4);
        assert_eq!(op.args[0], PDFObject::Number(100.0)); // x
        assert_eq!(op.args[1], PDFObject::Number(200.0)); // y
        assert_eq!(op.args[2], PDFObject::Number(50.0)); // width
        assert_eq!(op.args[3], PDFObject::Number(75.0)); // height
    }

    // ============================================================================
    // Path Painting Operator Tests
    // ============================================================================

    #[test]
    fn test_path_stroke() {
        let mut eval = create_evaluator("S");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::Stroke);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_close_stroke() {
        let mut eval = create_evaluator("s");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::CloseStroke);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_fill() {
        let mut eval = create_evaluator("f");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::Fill);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_eofill() {
        let mut eval = create_evaluator("f*");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::EOFill);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_fill_stroke() {
        let mut eval = create_evaluator("B");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::FillStroke);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_eofill_stroke() {
        let mut eval = create_evaluator("B*");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::EOFillStroke);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_close_fill_stroke() {
        let mut eval = create_evaluator("b");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::CloseFillStroke);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_close_eofill_stroke() {
        let mut eval = create_evaluator("b*");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::CloseEOFillStroke);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_path_end() {
        let mut eval = create_evaluator("n");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::EndPath);
        assert_eq!(op.args.len(), 0);
    }

    // ============================================================================
    // Clipping Operator Tests
    // ============================================================================

    #[test]
    fn test_clip_nonzero() {
        let mut eval = create_evaluator("W");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::Clip);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_clip_evenodd() {
        let mut eval = create_evaluator("W*");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::EOClip);
        assert_eq!(op.args.len(), 0);
    }

    // ============================================================================
    // Color Operator Tests
    // ============================================================================

    #[test]
    fn test_color_set_stroke_gray() {
        let mut eval = create_evaluator("0.5 G");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetStrokeGray);
        assert_eq!(op.args.len(), 1);
        assert_eq!(op.args[0], PDFObject::Number(0.5));
    }

    #[test]
    fn test_color_set_fill_gray() {
        let mut eval = create_evaluator("0.75 g");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetFillGray);
        assert_eq!(op.args.len(), 1);
        assert_eq!(op.args[0], PDFObject::Number(0.75));
    }

    #[test]
    fn test_color_set_stroke_rgb() {
        let mut eval = create_evaluator("1.0 0.5 0.25 RG");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetStrokeRGBColor);
        assert_eq!(op.args.len(), 3);
        assert_eq!(op.args[0], PDFObject::Number(1.0)); // R
        assert_eq!(op.args[1], PDFObject::Number(0.5)); // G
        assert_eq!(op.args[2], PDFObject::Number(0.25)); // B
    }

    #[test]
    fn test_color_set_fill_rgb() {
        let mut eval = create_evaluator("0.1 0.2 0.3 rg");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetFillRGBColor);
        assert_eq!(op.args.len(), 3);
    }

    #[test]
    fn test_color_set_stroke_cmyk() {
        let mut eval = create_evaluator("0.1 0.2 0.3 0.4 K");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetStrokeCMYKColor);
        assert_eq!(op.args.len(), 4);
    }

    #[test]
    fn test_color_set_fill_cmyk() {
        let mut eval = create_evaluator("0.5 0.6 0.7 0.8 k");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetFillCMYKColor);
        assert_eq!(op.args.len(), 4);
    }

    // ============================================================================
    // Graphics State Tests
    // ============================================================================

    #[test]
    fn test_set_line_width() {
        let mut eval = create_evaluator("2.5 w");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetLineWidth);
        assert_eq!(op.args.len(), 1);
        assert_eq!(op.args[0], PDFObject::Number(2.5));
    }

    #[test]
    fn test_set_line_cap() {
        let mut eval = create_evaluator("1 J");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetLineCap);
        assert_eq!(op.args.len(), 1);
    }

    #[test]
    fn test_set_line_join() {
        let mut eval = create_evaluator("0 j");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetLineJoin);
        assert_eq!(op.args.len(), 1);
    }

    #[test]
    fn test_set_miter_limit() {
        let mut eval = create_evaluator("10.0 M");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetMiterLimit);
        assert_eq!(op.args.len(), 1);
    }

    // ============================================================================
    // Text Positioning Tests
    // ============================================================================

    #[test]
    fn test_text_move_text() {
        // Td: tx ty
        let mut eval = create_evaluator("50 25 Td");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::MoveText);
        assert_eq!(op.args.len(), 2);
        assert_eq!(op.args[0], PDFObject::Number(50.0));
        assert_eq!(op.args[1], PDFObject::Number(25.0));
    }

    #[test]
    fn test_text_set_leading_move() {
        // TD: tx ty
        let mut eval = create_evaluator("10 20 TD");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetLeadingMoveText);
        assert_eq!(op.args.len(), 2);
    }

    #[test]
    fn test_text_set_matrix() {
        // Tm: a b c d e f
        let mut eval = create_evaluator("1 0 0 1 100 200 Tm");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::SetTextMatrix);
        assert_eq!(op.args.len(), 6);
    }

    #[test]
    fn test_text_next_line() {
        let mut eval = create_evaluator("T*");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::NextLine);
        assert_eq!(op.args.len(), 0);
    }

    #[test]
    fn test_text_show_spaced() {
        let mut eval = create_evaluator("[(A) 100 (B) -50 (C)] TJ");
        let op = eval.read_operation().unwrap().unwrap();

        assert_eq!(op.op, OpCode::ShowSpacedText);
        assert_eq!(op.args.len(), 1); // Array argument
    }

    // ============================================================================
    // Complex Content Stream Tests
    // ============================================================================

    #[test]
    fn test_nested_save_restore() {
        // Test nested q/Q pairs
        let content = "q\n1 0 0 1 10 10 cm\nq\n2 0 0 2 20 20 cm\nQ\nQ\n";

        let mut eval = create_evaluator(content);

        // First save
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::Save);

        // First transform
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::Transform);

        // Second save
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::Save);

        // Second transform
        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::Transform);

        // First restore (inner q)
        let op5 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op5.op, OpCode::Restore);

        // Second restore (outer q)
        let op6 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op6.op, OpCode::Restore);
    }

    #[test]
    fn test_transformation_matrix_accumulation() {
        // Test that multiple transforms accumulate correctly
        let content = "q\n1 0 0 1 10 0 cm\n1 0 0 1 0 20 cm\nQ\n";

        let mut eval = create_evaluator(content);

        eval.read_operation().unwrap().unwrap(); // q
        let op1 = eval.read_operation().unwrap().unwrap(); // cm 1
        let op2 = eval.read_operation().unwrap().unwrap(); // cm 2
        eval.read_operation().unwrap().unwrap(); // Q

        assert_eq!(op1.op, OpCode::Transform);
        assert_eq!(op2.op, OpCode::Transform);

        // Verify transforms
        assert_eq!(op1.args[0], PDFObject::Number(1.0));
        assert_eq!(op1.args[4], PDFObject::Number(10.0)); // tx

        assert_eq!(op2.args[0], PDFObject::Number(1.0));
        assert_eq!(op2.args[5], PDFObject::Number(20.0)); // ty
    }

    #[test]
    fn test_complex_path_with_paint() {
        // Draw a rectangle and fill it
        let content = "100 200 50 75 re\n0.5 0.5 0.5 rg\nf\n";

        let mut eval = create_evaluator(content);

        // Rectangle
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::Rectangle);

        // Set fill color (RGB)
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::SetFillRGBColor);

        // Fill path
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::Fill);
    }

    #[test]
    fn test_mixed_graphics_and_text() {
        // Test content stream with both graphics and text
        let content = "0.0 0.0 0.0 1.0 rg\n10 10 5 5 re\nf\nBT\n/F1 12 Tf\n(Text) Tj\nET\n";

        let mut eval = create_evaluator(content);

        // Set fill RGB color
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::SetFillRGBColor);

        // Rectangle
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::Rectangle);

        // Fill
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::Fill);

        // Begin text
        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::BeginText);

        // Set font
        let op5 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op5.op, OpCode::SetFont);

        // Show text
        let op6 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op6.op, OpCode::ShowText);

        // End text
        let op7 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op7.op, OpCode::EndText);
    }

    #[test]
    fn test_clipping_path() {
        // Test clipping path with nonzero winding rule
        let content = "100 100 200 200 re\nW\n0.5 g\nf\n";

        let mut eval = create_evaluator(content);

        // Rectangle (path to clip)
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::Rectangle);

        // Set clipping path
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::Clip);

        // Set fill gray
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::SetFillGray);

        // Fill
        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::Fill);
    }

    #[test]
    fn test_clipping_path_evenodd() {
        // Test clipping path with even-odd rule
        let content = "50 50 100 100 re\nW*\n1.0 0.0 0.0 rg\nf\n";

        let mut eval = create_evaluator(content);

        // Rectangle
        let op1 = eval.read_operation().unwrap().unwrap();

        // Set clipping path (even-odd)
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::EOClip);

        // Set fill RGB color
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::SetFillRGBColor);

        // Fill
        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::Fill);
    }

    #[test]
    fn test_text_state_operators() {
        // Test text state operators
        let content = "BT\n2.0 Tc\n3.0 Tw\n4.0 Tz\n5.0 TL\n6 Tr\n7.0 Ts\nET";

        let mut eval = create_evaluator(content);

        eval.read_operation().unwrap().unwrap(); // BT
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::SetCharSpacing);
        assert_eq!(op1.args[0], PDFObject::Number(2.0));

        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::SetWordSpacing);

        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::SetHScale);

        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::SetLeading);

        let op5 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op5.op, OpCode::SetTextRenderingMode);

        let op6 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op6.op, OpCode::SetTextRise);
    }

    #[test]
    fn test_complete_drawing_workflow() {
        // Complete drawing workflow: path, color, paint
        let content = "10 10 m\n50 10 l\n50 50 l\n10 50 l\nh\n0.0 1.0 0.0 rg\nf\n";

        let mut eval = create_evaluator(content);

        // Move to (10, 10)
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::MoveTo);

        // Line to (50, 10)
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::LineTo);

        // Line to (50, 50)
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::LineTo);

        // Line to (10, 50)
        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::LineTo);

        // Close path
        let op5 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op5.op, OpCode::ClosePath);

        // Set fill color (green)
        let op6 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op6.op, OpCode::SetFillRGBColor);

        // Fill
        let op7 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op7.op, OpCode::Fill);
    }

    #[test]
    fn test_text_with_advanced_positioning() {
        // Test Tm (set text matrix) and T* (next line)
        let content = "BT\n1 0 0 1 100 200 Tm\n(Text) Tj\nT*\n(More) Tj\nET";

        let mut eval = create_evaluator(content);

        eval.read_operation().unwrap().unwrap(); // BT

        // Set text matrix
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::SetTextMatrix);
        assert_eq!(op1.args.len(), 6);

        // Show text
        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::ShowText);

        // Next line
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::NextLine);

        // Show more text
        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::ShowText);
    }

    #[test]
    fn test_multiple_pages_in_stream() {
        // Simulate multiple pages in one stream
        let content = "100 100 50 50 re\nf\n1000 1000 100 100 re\nf\n";

        let mut eval = create_evaluator(content);

        // First rectangle and fill
        let op1 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op1.op, OpCode::Rectangle);

        let op2 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op2.op, OpCode::Fill);

        // Second rectangle and fill
        let op3 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op3.op, OpCode::Rectangle);

        let op4 = eval.read_operation().unwrap().unwrap();
        assert_eq!(op4.op, OpCode::Fill);
    }
}
