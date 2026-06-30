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
pub fn interpret<D: Device>(
    page: &crate::core::resolved_page::ResolvedPage,
    device: &mut D,
) -> crate::core::error::PDFResult<()> {
    use crate::core::base_stream::BaseStream;
    use crate::core::content_stream::ContentStreamEvaluator;
    use crate::core::lexer::Lexer;
    use crate::core::parser::{PDFObject, Parser};
    use crate::core::resolved_page::SnapshotPageResolver;
    use crate::core::stream::Stream;
    use crate::rendering::context::RenderingContext;
    use crate::rendering::graphics_state::FillRule;

    // Set initial CTM and clip to the view box (matches Page::render's pre-amble).
    let [x0, y0, x1, y1] = page.view_box();
    let page_transform = [1.0, 0.0, 0.0, -1.0, -x0, y1];
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
    if !matches!(page.resources(), PDFObject::Null) {
        ctx.set_xobject_resources(&mut resolver, page.resources());
    }

    loop {
        match evaluator.read_operation() {
            Ok(Some(op)) => {
                if let Err(e) = ctx.process_operation(&op) {
                    eprintln!("Warning: interpret: failed to process {:?}: {}", op.op, e);
                }
            }
            // End of stream, or a read error (e.g. a stray byte) — stop, like Page::render.
            Ok(None) | Err(_) => return Ok(()),
        }
    }
}
