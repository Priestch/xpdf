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

use pdf_x_core::core::PDFDocument;
use pdf_x_core::core::error::{PDFError, PDFResult};
use pdf_x_core::core::resolved_page::ResolvedPage;
use pdf_x_core::rendering::{Device, interpret};

use std::sync::Arc;

/// Helper trait for font arena access (type-erased cache on PDFDocument).
trait FontCacheExt {
    fn ensure_font_cache(&mut self) -> &mut FontCache;
}

impl FontCacheExt for PDFDocument {
    fn ensure_font_cache(&mut self) -> &mut FontCache {
        if self.font_cache_mut().is_none() {
            self.set_font_cache(Box::new(FontCache::new()));
        }
        self.font_cache_mut()
            .as_mut()
            .expect("just initialized")
            .downcast_mut::<FontCache>()
            .expect("font_cache type mismatch — only tinyskia should set it")
    }
}

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

    /// Render multiple pages in parallel.
    ///
    /// Pages are resolved sequentially through `DocStore::resolve_page` (cache-aware,
    /// delta → base), then interpretation + rasterization run in parallel across
    /// threads. Each thread owns its own `Pixmap` and `SkiaDevice`; the shared
    /// `Arc<ResolvedPage>` snapshots are `Send + Sync`.
    ///
    /// Returns one result per input page index, in order.
    fn render_pages_to_images(
        &mut self,
        page_indices: &[usize],
        scale: Option<f32>,
    ) -> Vec<PDFResult<(u32, u32, Vec<u8>)>>;
}

impl RenderToImage for PDFDocument {
    fn render_page_to_image(
        &mut self,
        page_index: usize,
        scale: Option<f32>,
    ) -> PDFResult<(u32, u32, Vec<u8>)> {
        let resolved = self.resolve_page(page_index)?;
        let cache = self.ensure_font_cache();
        render_resolved_page(&resolved, scale, cache)
    }

    fn render_pages_to_images(
        &mut self,
        page_indices: &[usize],
        scale: Option<f32>,
    ) -> Vec<PDFResult<(u32, u32, Vec<u8>)>> {
        if page_indices.is_empty() {
            return Vec::new();
        }

        // Phase 1: resolve all pages sequentially (needs &mut self for DocStore).
        let resolved: Vec<PDFResult<Arc<ResolvedPage>>> = page_indices
            .iter()
            .map(|&idx| self.resolve_page(idx))
            .collect();

        // Phase 2: interpret + rasterize in parallel.
        // Each thread gets its own font cache (empty — fonts are parsed per-thread
        // on first use). The persistent cache on PDFDocument benefits the common
        // sequential `render_page_to_image` path; parallel rendering re-parses
        // because StoredFont (ttf_parser::Face) is not Clone.
        let results: Vec<PDFResult<(u32, u32, Vec<u8>)>> = std::thread::scope(|s| {
            let handles: Vec<_> = resolved
                .into_iter()
                .map(|r| {
                    s.spawn(move || {
                        let mut local_cache = FontCache::new();
                        match r {
                            Ok(page) => render_resolved_page(&page, scale, &mut local_cache),
                            Err(e) => Err(e),
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("render thread panicked"))
                .collect()
        });

        results
    }
}

/// The type stored in PDFDocument's `font_cache`: parsed fonts keyed by
/// resource name, persisting across renders so Type1/CFF parsing only
/// happens once per font.
pub type FontCache = std::collections::HashMap<String, skia_device::StoredFont>;

/// Pure render: interpret a `ResolvedPage` through a `SkiaDevice` and rasterize
/// to RGBA. The `font_cache` is borrowed from PDFDocument so parsed fonts
/// persist across renders.
fn render_resolved_page(
    resolved: &ResolvedPage,
    scale: Option<f32>,
    font_cache: &mut FontCache,
) -> PDFResult<(u32, u32, Vec<u8>)> {
    // Reference: pdf.js/src/core/document.js - Page.view and Page.rotate
    let [x0, y0, x1, y1] = resolved.view_box();
    let rotation = resolved.rotation();

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
    let mut device = SkiaDevice::new(pixmap.as_mut(), font_cache);

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

    // Render via the pure interpret path: the ResolvedPage already has a frozen
    // snapshot, so interpret runs with no live DocStore (decision C end-state).
    interpret(resolved, &mut device)?;

    // Extract pixel data
    let pixels = pixmap.data().to_vec();

    Ok((width, height, pixels))
}
