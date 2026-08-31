//! Font loading and metrics (`Device` methods).

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn supports_native_cid_to_gid(&self) -> bool {
        cfg!(feature = "native-freetype")
    }

    pub(super) fn font_has_native_cid_to_gid(&self, name: &str) -> bool {
        #[cfg(feature = "native-freetype")]
        {
            self.font_cache
                .get(name)
                .map(|font| font.native_charmap_available)
                .unwrap_or(false)
        }
        #[cfg(not(feature = "native-freetype"))]
        {
            let _ = name;
            false
        }
    }

    pub(super) fn load_font_data(
        &mut self,
        name: &str,
        data: Vec<u8>,
        encoding: Option<&pdf_x_core::core::parser::PDFObject>,
    ) -> PDFResult<()> {
        // Skip re-parsing if already cached (cross-render persistence).
        if self.font_cache.contains_key(name) {
            return Ok(());
        }
        self.load_font(name, data, encoding)
    }

    pub(super) fn resolve_standard_font_data(&mut self, base_font: &str) -> Option<Vec<u8>> {
        self.standard_font_resolver
            .and_then(|resolver| resolver(base_font))
    }

    pub(super) fn set_font_widths(
        &mut self,
        name: &str,
        first_char: u32,
        widths: &[f64],
        default_width: f64,
    ) -> PDFResult<()> {
        if let Some(font) = self.font_cache.get_mut(name) {
            font.set_pdf_widths(first_char, widths, default_width);

            #[cfg(feature = "debug-logging")]
            eprintln!(
                "DEBUG: Set PDF widths for font '{}': first_char={}, count={}, default_width={}",
                name,
                first_char,
                widths.len(),
                default_width
            );
        }

        Ok(())
    }

    pub(super) fn set_font_raw_cid_codes(&mut self, name: &str, enabled: bool) -> PDFResult<()> {
        if let Some(font) = self.font_cache.get_mut(name) {
            font.set_raw_cid_codes(enabled);
        }
        Ok(())
    }

    pub(super) fn has_font(&self, name: &str) -> bool {
        self.font_cache.contains_key(name)
    }

    pub(super) fn font_wants_raw_cid_codes(&self, name: &str) -> bool {
        self.font_cache
            .get(name)
            .map(StoredFont::uses_raw_cid_codes)
            .unwrap_or(false)
    }

    pub(super) fn font_is_vertical(&self, name: &str) -> bool {
        self.font_cache
            .get(name)
            .map(|font| font.cid_vertical())
            .unwrap_or(false)
    }

    pub(super) fn set_font_cid_vertical_metrics(
        &mut self,
        name: &str,
        vertical: bool,
        dw2: (f64, f64),
        widths2: &[(u32, f64, f64, f64)],
    ) -> PDFResult<()> {
        if let Some(font) = self.font_cache.get_mut(name) {
            font.set_cid_vertical_metrics(vertical, dw2, widths2);
        }
        Ok(())
    }

    pub(super) fn set_font_cid_to_gid_map(&mut self, name: &str, map: &[u16]) -> PDFResult<()> {
        if let Some(font) = self.font_cache.get_mut(name) {
            font.set_cid_to_gid_map(map);
        }
        Ok(())
    }
}
