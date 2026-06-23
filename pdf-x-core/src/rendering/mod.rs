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

// Re-export key types
pub use context::{is_text_trace_enabled, set_text_trace_enabled, RenderingContext};
pub use device::{Device, ImageData, Paint, PathDrawMode, TestDevice};
pub use graphics_state::{
    BlendMode, Color, FillRule, GraphicsState, LineCap, LineJoin, StrokeProps, TextRenderingMode,
};
pub use path::{Path, PathBuilder, PathElement};

#[cfg(feature = "rendering")]
pub mod skia_device;

#[cfg(feature = "rendering")]
pub mod font;

#[cfg(feature = "rendering")]
pub mod type1_font;

#[cfg(feature = "rendering")]
pub use skia_device::SkiaDevice;

#[cfg(feature = "rendering")]
pub use font::Font;

/// Pure interpretation of a [`ResolvedPage`] onto a [`Device`] (decision C).
///
/// This is the interpret half of resolve-then-interpret: it takes only an immutable,
/// fully-resolved page and a device — **no `DocStore`, no `Need`, no I/O** — so it cannot
/// block or hang a `drive` loop, and a native shell can run it on another thread.
///
/// *Step 4b skeleton*: this signature is in place to lock the purity contract, but the
/// body is a stub. The full content-stream engine moves off `RenderingContext`'s lazy
/// fetching and onto `ResolvedPage` in the Step 4a rewire.
pub fn interpret(_page: &crate::core::resolved_page::ResolvedPage, _device: &mut dyn Device) {}
