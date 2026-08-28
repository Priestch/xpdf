//! Type3 glyph begin/end (`Device` methods).

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn begin_type3_glyph(&mut self) {
        self.type3_glyph_depth = self.type3_glyph_depth.saturating_add(1);
    }

    pub(super) fn end_type3_glyph(&mut self) {
        self.type3_glyph_depth = self.type3_glyph_depth.saturating_sub(1);
    }
}
