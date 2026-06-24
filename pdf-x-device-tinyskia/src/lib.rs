//! tiny-skia `Device` backend adapter for [`pdf_x_core`](pdf_x_core).
//!
//! Phase 0 decision A: device backends are **adapter crates**, not hard dependencies
//! of `pdf-x-core`. This crate owns the tiny-skia CPU rasterizer and the
//! TrueType / Type1 / CFF font shaping it needs (`rustybuzz` / `ttf-parser` /
//! `hayro-font`). The core stays backend-free (`cargo build -p pdf-x-core
//! --no-default-features` links none of them); a shell picks this adapter — or a
//! future `pdf-x-device-vello` / `pdf-x-device-canvas2d` — for actual pixel output.
//!
//! [`pdf_x_core::rendering::interpret`] is a pure function over a `ResolvedPage`;
//! this adapter just supplies a `Device` (a `Pixmap`-backed `SkiaDevice`) and,
//! for convenience, the [`RenderToImage`] extension that renders a page to RGBA.

#![allow(dead_code)] // `font` (rustybuzz wrapper) is retained backend surface, not yet wired through.

mod font;
mod skia_device;
mod type1_font;

pub use skia_device::SkiaDevice;

use pdf_x_core::core::error::{PDFError, PDFResult};
use pdf_x_core::core::PDFDocument;
use pdf_x_core::rendering::{interpret, Device};

/// Render a page of a [`PDFDocument`] to RGBA pixels using the tiny-skia backend.
///
/// This is the adapter-side equivalent of the former `PDFDocument::render_page_to_image`.
/// Brought into scope via `use pdf_x_device_tinyskia::RenderToImage;` so call sites
/// read `doc.render_page_to_image(...)` unchanged.
///
/// Returns `(width, height, pixels)` where `pixels` is `[R, G, B, A, ...]` row-major,
/// top to bottom.
pub trait RenderToImage {
    fn render_page_to_image(
        &mut self,
        page_index: usize,
        scale: Option<f32>,
    ) -> PDFResult<(u32, u32, Vec<u8>)>;
}

impl RenderToImage for PDFDocument {
    fn render_page_to_image(
        &mut self,
        page_index: usize,
        scale: Option<f32>,
    ) -> PDFResult<(u32, u32, Vec<u8>)> {
        // Get the page and a DocStore (delta → base). The whole render resolves through
        // the store so delta overrides of /Contents (and page-level data) are honored.
        let page = self.get_page(page_index)?;
        let mut store = self.doc_store_mut();

        // Reference: pdf.js/src/core/document.js - Page.view and Page.rotate
        let [x0, y0, x1, y1] = page.resolve_view_box_for_rendering(store.xref_mut());
        let rotation = page.resolve_rotate_for_rendering(store.xref_mut());

        let page_width = x1 - x0;
        let page_height = y1 - y0;

        // Apply scale
        let scale = scale.unwrap_or(1.0);
        let (width, height) = if rotation % 180 == 0 {
            (
                (page_width as f32 * scale).ceil() as u32,
                (page_height as f32 * scale).ceil() as u32,
            )
        } else {
            (
                (page_height as f32 * scale).ceil() as u32,
                (page_width as f32 * scale).ceil() as u32,
            )
        };

        // Create a white pixmap
        let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or_else(|| {
            PDFError::Generic(format!("Failed to create {}x{} pixmap", width, height))
        })?;

        // Fill with white background
        pixmap.fill(tiny_skia::Color::WHITE);

        // Create rendering device
        let mut device = SkiaDevice::new(pixmap.as_mut());

        // Apply PDF.js-like viewport transform.
        // Reference: pdf.js/src/display/display_utils.js - PageViewport
        let center_x = (x0 + x1) / 2.0;
        let center_y = (y0 + y1) / 2.0;

        let (rotate_a, rotate_b, rotate_c, rotate_d) = match rotation {
            90 => (0.0, 1.0, 1.0, 0.0),
            180 => (-1.0, 0.0, 0.0, 1.0),
            270 => (0.0, -1.0, -1.0, 0.0),
            _ => (1.0, 0.0, 0.0, -1.0),
        };

        let offset_canvas_x;
        let offset_canvas_y;
        if rotate_a == 0.0 {
            offset_canvas_x = (center_y - y0).abs() * scale as f64;
            offset_canvas_y = (center_x - x0).abs() * scale as f64;
        } else {
            offset_canvas_x = (center_x - x0).abs() * scale as f64;
            offset_canvas_y = (center_y - y0).abs() * scale as f64;
        }

        device.set_matrix(&[
            rotate_a * scale as f64,
            rotate_b * scale as f64,
            rotate_c * scale as f64,
            rotate_d * scale as f64,
            offset_canvas_x - rotate_a * scale as f64 * center_x - rotate_c * scale as f64 * center_y,
            offset_canvas_y - rotate_b * scale as f64 * center_x - rotate_d * scale as f64 * center_y,
        ]);

        // Render via the pure interpret path: resolve to a snapshot (delta-aware), then
        // interpret with no live DocStore (decision C end-state).
        let resolved = store.resolve_page(&page)?;
        interpret(&resolved, &mut device)?;

        // Extract pixel data
        let pixels = pixmap.data().to_vec();

        Ok((width, height, pixels))
    }
}
