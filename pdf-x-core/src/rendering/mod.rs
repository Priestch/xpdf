//! PDF rendering layer.
//!
//! This module provides the rendering infrastructure for PDF content streams.
//! It follows a similar architecture to PDF.js and hayro, with:
//! - A Device trait for backend abstraction
//! - Graphics state management
//! - Path construction and rendering
//! - Text rendering support

pub mod context;
pub mod device;
pub mod graphics_state;
pub mod path;

#[cfg(feature = "rendering")]
pub mod cmyk_icc;

#[cfg(feature = "rendering")]
pub mod lab_icc;

pub mod shading;

// Re-export key types
pub use context::{RenderingContext, is_text_trace_enabled, set_text_trace_enabled};
pub use device::{Device, ImageData, Paint, PathDrawMode, TestDevice};
pub use graphics_state::{
    BlendMode, Color, FillRule, GraphicsState, LineCap, LineJoin, StrokeProps, TextRenderingMode,
};
pub use path::{Path, PathBuilder, PathElement};

// The tiny-skia `Device` backend (`SkiaDevice`) and the TrueType/Type1/CFF font
// shaping it needs live in the `pdf-x-device-tinyskia` adapter crate (Phase 0
// decision A: backends are adapters, not core hard-deps). This module keeps only
// the backend-agnostic surface: the `Device` trait, graphics state, paths, the
// `RenderingContext` interpreter, and the pure `interpret` function.

/// Pure interpretation of a [`ResolvedPage`] onto a [`Device`] (decision C).
///
/// This is the interpret half of resolve-then-interpret: it takes only an immutable,
/// fully-resolved page and a device — **no `DocStore`, no `Need`, no I/O** — so it cannot
/// block or hang a `drive` loop, and a native shell can run it on another thread. Resources
/// are read from the page's pre-resolved `objects` snapshot via a [`SnapshotPageResolver`].
///
/// Uses a 1.0 viewport scale (72 DPI user-space mapping). Prefer
/// [`interpret_with_scale`] when the shell has already sized the surface for a
/// non-1 scale — previously `render_resolved_page` set a scaled device CTM that
/// this function immediately overwrote with the unscaled page transform.
pub fn interpret<D: Device>(
    page: &crate::core::resolved_page::ResolvedPage,
    device: &mut D,
) -> crate::core::error::PDFResult<()> {
    interpret_with_scale(page, device, 1.0)
}

/// Like [`interpret`], but multiplies the page viewport CTM by `scale`
/// (device pixels per PDF user unit). Shells that allocate a pixmap of size
/// `ceil(page_size * scale)` must pass the same scale here so content fills
/// the surface (pdf.js `PageViewport` scale).
pub fn interpret_with_scale<D: Device>(
    page: &crate::core::resolved_page::ResolvedPage,
    device: &mut D,
    scale: f32,
) -> crate::core::error::PDFResult<()> {
    use crate::core::base_stream::BaseStream;
    use crate::core::content_stream::ContentStreamEvaluator;
    use crate::core::lexer::Lexer;
    use crate::core::parser::{PDFObject, Parser};
    use crate::core::resolved_page::SnapshotPageResolver;
    use crate::core::stream::Stream;
    use crate::rendering::context::RenderingContext;
    use crate::rendering::graphics_state::FillRule;

    // Validate scale parameter (C.3: render budgets and input validation)
    if !scale.is_finite() {
        return Err(crate::core::error::PDFError::Generic(format!(
            "Scale must be finite, got {}",
            scale
        )));
    }
    if scale <= 0.0 {
        return Err(crate::core::error::PDFError::Generic(format!(
            "Scale must be positive, got {}",
            scale
        )));
    }

    // Calculate and validate output dimensions
    let [x0, y0, x1, y1] = page.view_box();
    let page_width = x1 - x0;
    let page_height = y1 - y0;

    // Apply reasonable limits to prevent extreme memory/rendering costs
    const MAX_OUTPUT_DIMENSION: f64 = 65536.0; // 64K pixels
    const MAX_OUTPUT_AREA: f64 = 512.0 * 1024.0 * 1024.0; // 512 megapixels

    let output_width = page_width * scale as f64;
    let output_height = page_height * scale as f64;
    let output_area = output_width * output_height;

    if output_width > MAX_OUTPUT_DIMENSION || output_height > MAX_OUTPUT_DIMENSION {
        return Err(crate::core::error::PDFError::Generic(format!(
            "Output dimension too large: {}x{} exceeds maximum {} pixels",
            output_width, output_height, MAX_OUTPUT_DIMENSION
        )));
    }

    if output_area > MAX_OUTPUT_AREA {
        return Err(crate::core::error::PDFError::Generic(format!(
            "Output area too large: {:.1} megapixels exceeds maximum {} megapixels",
            output_area / (1024.0 * 1024.0),
            MAX_OUTPUT_AREA / (1024.0 * 1024.0)
        )));
    }

    // Set initial CTM and clip to the view box (matches Page::render's pre-amble).
    // Scale all six matrix components so both linear and translation terms map
    // user space → device pixels (equivalent to pdf.js PageViewport scale).
    let [x0, y0, x1, y1] = page.view_box();
    let mut page_transform =
        crate::core::page::page_initial_transform(page.view_box(), page.rotation());
    let s = scale as f64;
    if (s - 1.0).abs() > f64::EPSILON {
        for v in &mut page_transform {
            *v *= s;
        }
    }
    device.set_matrix(&page_transform);
    device.begin_path();
    device.move_to(x0, y0);
    device.line_to(x1, y0);
    device.line_to(x1, y1);
    device.line_to(x0, y1);
    device.close_path();
    device.clip_path(FillRule::NonZero)?;

    // Parse the decoded content stream into operations.
    let stream = Box::new(Stream::from_bytes(page.contents().to_vec())) as Box<dyn BaseStream>;
    let lexer = Lexer::new(stream)?;
    let parser = Parser::new(lexer)?;
    let mut evaluator = ContentStreamEvaluator::new(parser);

    // Pure interpretation: drive RenderingContext with a snapshot-backed resolver — no
    // live DocStore, no fetches beyond the frozen `objects` map. (No explicit save/restore:
    // interpret is a single pass, and the content's own q/Q ops manage graphics state.)
    let mut resolver = SnapshotPageResolver::new(page.objects());
    let mut ctx = RenderingContext::new(device);
    ctx.current_state_mut().set_matrix(&page_transform);
    ctx.set_root_ctm(page_transform);
    if !matches!(page.resources(), PDFObject::Null) {
        ctx.set_xobject_resources(&mut resolver, page.resources());
    }

    // C.3 render budget: hostile content streams must not loop unbounded.
    // Real pages are orders of magnitude below this (tracemonkey p0 is ~50k
    // ops); the cap exists so a pathological stream errors visibly instead of
    // hanging the shell.
    const MAX_OPERATIONS: u64 = 20_000_000;
    let mut operations: u64 = 0;
    while let Some(op) = evaluator.read_operation()? {
        operations += 1;
        if operations > MAX_OPERATIONS {
            return Err(crate::core::error::PDFError::Generic(format!(
                "Operation budget exceeded: {} content-stream operations (max {})",
                operations, MAX_OPERATIONS
            )));
        }
        ctx.process_operation(&op)?;
    }

    // PDF 32000-1 §12.5.5: paint annotation Normal appearance streams after the
    // page content. Beamer/animated PDFs store slide bodies in Widget `/AP /N`
    // Form XObjects (`animated-distributions.pdf`). Matches hayro's default
    // `render_annotations: true` path.
    // Source: hayro/hayro-interpret/src/interpret/mod.rs `interpret_page`.
    ctx.paint_annotation_appearances(page.annotations())?;
    Ok(())
}
