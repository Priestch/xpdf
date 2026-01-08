//! PDF rendering layer.
//!
//! This module provides the rendering infrastructure for PDF content streams.
//! It follows a similar architecture to PDF.js and hayro, with:
//! - A Device trait for backend abstraction
//! - Graphics state management
//! - Path construction and rendering
//! - Text rendering support

pub mod device;
pub mod graphics_state;
pub mod context;
pub mod path;

// Re-export key types
pub use device::{Device, ImageData, Paint, PathDrawMode, TestDevice};
pub use graphics_state::{
    Color, FillRule, GraphicsState, LineCap, LineJoin, StrokeProps, TextRenderingMode
};
pub use context::RenderingContext;
pub use path::{Path, PathBuilder, PathElement};

#[cfg(feature = "rendering")]
pub mod skia_device;

#[cfg(feature = "rendering")]
pub mod font;

#[cfg(feature = "rendering")]
pub use skia_device::SkiaDevice;

#[cfg(feature = "rendering")]
pub use font::Font;
