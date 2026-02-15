//! Font loading and text shaping.

use rustybuzz::{Face as BuzzFace, GlyphBuffer, UnicodeBuffer};
use ttf_parser::{Face, FaceParsingError};

pub struct Font {
    face: Face<'static>,
    buzz_face: BuzzFace<'static>,
}

impl Font {
    pub fn new(font_data: &'static [u8]) -> Result<Self, FaceParsingError> {
        let face = Face::parse(font_data, 0)?;
        let buzz_face = BuzzFace::from_slice(font_data, 0).unwrap();
        Ok(Font { face, buzz_face })
    }

    pub fn shape(&self, text: &str) -> GlyphBuffer {
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        rustybuzz::shape(&self.buzz_face, &[], buffer)
    }

    pub fn face(&self) -> &Face {
        &self.face
    }
}
