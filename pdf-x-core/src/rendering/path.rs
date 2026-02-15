//! Path construction and management for PDF rendering.
//!
//! This module provides path construction utilities for PDF graphics.
//! Paths are built incrementally using move, line, curve, and rectangle operations.

use std::fmt;

/// A path element.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathElement {
    /// Move to a new point (starts a new subpath)
    MoveTo(f64, f64),
    /// Line to a point
    LineTo(f64, f64),
    /// Cubic Bézier curve (cp1x, cp1y, cp2x, cp2y, x, y)
    CurveTo(f64, f64, f64, f64, f64, f64),
    /// Close the current subpath
    ClosePath,
}

impl fmt::Display for PathElement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathElement::MoveTo(x, y) => write!(f, "M {} {}", x, y),
            PathElement::LineTo(x, y) => write!(f, "L {} {}", x, y),
            PathElement::CurveTo(cp1x, cp1y, cp2x, cp2y, x, y) => {
                write!(f, "C {} {} {} {} {} {}", cp1x, cp1y, cp2x, cp2y, x, y)
            }
            PathElement::ClosePath => write!(f, "Z"),
        }
    }
}

/// A path for rendering.
///
/// Paths are composed of a sequence of path elements (move, line, curve, close).
/// This is similar to how PDF content streams build paths incrementally.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// The path elements
    elements: Vec<PathElement>,

    /// Current point (if any)
    current_point: Option<(f64, f64)>,

    /// Start of the current subpath (for close operations)
    subpath_start: Option<(f64, f64)>,

    /// Whether we have an open subpath
    has_open_subpath: bool,
}

impl Default for Path {
    fn default() -> Self {
        Self::new()
    }
}

impl Path {
    /// Create a new empty path.
    pub fn new() -> Self {
        Path {
            elements: Vec::new(),
            current_point: None,
            subpath_start: None,
            has_open_subpath: false,
        }
    }

    /// Begin a new path, clearing any existing elements.
    pub fn begin(&mut self) {
        self.elements.clear();
        self.current_point = None;
        self.subpath_start = None;
        self.has_open_subpath = false;
    }

    /// Move to a new point, starting a new subpath.
    pub fn move_to(&mut self, x: f64, y: f64) {
        self.elements.push(PathElement::MoveTo(x, y));
        self.current_point = Some((x, y));
        self.subpath_start = Some((x, y));
        self.has_open_subpath = false;
    }

    /// Add a line segment from the current point to (x, y).
    pub fn line_to(&mut self, x: f64, y: f64) {
        // If we don't have a current point, implicit move
        if self.current_point.is_none() {
            self.move_to(x, y);
            return;
        }

        self.elements.push(PathElement::LineTo(x, y));
        self.current_point = Some((x, y));
        self.has_open_subpath = true;
    }

    /// Add a cubic Bézier curve.
    ///
    /// # Arguments
    /// * `cp1x, cp1y` - First control point
    /// * `cp2x, cp2y` - Second control point
    /// * `x, y` - End point
    pub fn curve_to(&mut self, cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64) {
        // If we don't have a current point, implicit move
        if self.current_point.is_none() {
            self.move_to(cp1x, cp1y);
        }

        self.elements
            .push(PathElement::CurveTo(cp1x, cp1y, cp2x, cp2y, x, y));
        self.current_point = Some((x, y));
        self.has_open_subpath = true;
    }

    /// Add a rectangle to the path.
    ///
    /// This is equivalent to:
    /// ```text
    /// move_to(x, y)
    /// line_to(x + width, y)
    /// line_to(x + width, y + height)
    /// line_to(x, y + height)
    /// close_path()
    /// ```
    pub fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        self.move_to(x, y);
        self.line_to(x + width, y);
        self.line_to(x + width, y + height);
        self.line_to(x, y + height);
        self.close_path();
    }

    /// Close the current subpath.
    ///
    /// This adds a line from the current point back to the start of the subpath.
    pub fn close_path(&mut self) {
        if self.has_open_subpath {
            self.elements.push(PathElement::ClosePath);
            // Return to subpath start
            if let Some(start) = self.subpath_start {
                self.current_point = Some(start);
            }
            self.has_open_subpath = false;
        }
    }

    /// Get the current point.
    pub fn current_point(&self) -> Option<(f64, f64)> {
        self.current_point
    }

    /// Get the path elements.
    pub fn elements(&self) -> &[PathElement] {
        &self.elements
    }

    /// Check if the path is empty.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Get the number of elements in the path.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Get the bounding box of the path.
    ///
    /// This returns a rough bounding box by finding the min/max x and y
    /// coordinates from all path elements.
    pub fn bounding_box(&self) -> Option<(f64, f64, f64, f64)> {
        if self.elements.is_empty() {
            return None;
        }

        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;

        for el in &self.elements {
            match el {
                PathElement::MoveTo(x, y) | PathElement::LineTo(x, y) => {
                    min_x = min_x.min(*x);
                    min_y = min_y.min(*y);
                    max_x = max_x.max(*x);
                    max_y = max_y.max(*y);
                }
                PathElement::CurveTo(cp1x, cp1y, cp2x, cp2y, x, y) => {
                    min_x = min_x.min(*cp1x).min(*cp2x).min(*x);
                    min_y = min_y.min(*cp1y).min(*cp2y).min(*y);
                    max_x = max_x.max(*cp1x).max(*cp2x).max(*x);
                    max_y = max_y.max(*cp1y).max(*cp2y).max(*y);
                }
                PathElement::ClosePath => {}
            }
        }

        Some((min_x, min_y, max_x, max_y))
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for el in &self.elements {
            write!(f, "{} ", el)?;
        }
        Ok(())
    }
}

/// Builder for constructing paths.
///
/// This provides a convenient interface for building paths incrementally.
pub struct PathBuilder {
    path: Path,
}

impl Default for PathBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PathBuilder {
    /// Create a new path builder.
    pub fn new() -> Self {
        PathBuilder { path: Path::new() }
    }

    /// Begin a new path.
    pub fn begin(&mut self) -> &mut Self {
        self.path.begin();
        self
    }

    /// Move to a point.
    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.path.move_to(x, y);
        self
    }

    /// Add a line segment.
    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.path.line_to(x, y);
        self
    }

    /// Add a cubic Bézier curve.
    pub fn curve_to(
        &mut self,
        cp1x: f64,
        cp1y: f64,
        cp2x: f64,
        cp2y: f64,
        x: f64,
        y: f64,
    ) -> &mut Self {
        self.path.curve_to(cp1x, cp1y, cp2x, cp2y, x, y);
        self
    }

    /// Add a rectangle.
    pub fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) -> &mut Self {
        self.path.rect(x, y, width, height);
        self
    }

    /// Close the current subpath.
    pub fn close(&mut self) -> &mut Self {
        self.path.close_path();
        self
    }

    /// Build and return the path.
    pub fn build(&self) -> Path {
        self.path.clone()
    }

    /// Get a reference to the path being built.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get a mutable reference to the path being built.
    pub fn path_mut(&mut self) -> &mut Path {
        &mut self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_path() {
        let path = Path::new();
        assert!(path.is_empty());
        assert_eq!(path.len(), 0);
    }

    #[test]
    fn test_move_to() {
        let mut path = Path::new();
        path.move_to(10.0, 20.0);
        assert_eq!(path.current_point(), Some((10.0, 20.0)));
        assert_eq!(path.len(), 1);
    }

    #[test]
    fn test_line_to() {
        let mut path = Path::new();
        path.move_to(10.0, 20.0);
        path.line_to(30.0, 40.0);
        assert_eq!(path.current_point(), Some((30.0, 40.0)));
        assert_eq!(path.len(), 2);
    }

    #[test]
    fn test_curve_to() {
        let mut path = Path::new();
        path.move_to(10.0, 20.0);
        path.curve_to(15.0, 25.0, 20.0, 30.0, 30.0, 40.0);
        assert_eq!(path.current_point(), Some((30.0, 40.0)));
        assert_eq!(path.len(), 2);
    }

    #[test]
    fn test_close_path() {
        let mut path = Path::new();
        path.move_to(10.0, 20.0);
        path.line_to(30.0, 40.0);
        path.close_path();
        assert_eq!(path.current_point(), Some((10.0, 20.0)));
        assert_eq!(path.len(), 3);
    }

    #[test]
    fn test_rect() {
        let mut path = Path::new();
        path.rect(10.0, 20.0, 100.0, 50.0);
        assert_eq!(path.len(), 5); // move + 3 lines + close

        let bbox = path.bounding_box();
        assert_eq!(bbox, Some((10.0, 20.0, 110.0, 70.0)));
    }

    #[test]
    fn test_bounding_box() {
        let mut path = Path::new();
        path.move_to(10.0, 20.0);
        path.line_to(30.0, 40.0);
        path.line_to(50.0, 10.0);

        let bbox = path.bounding_box();
        assert_eq!(bbox, Some((10.0, 10.0, 50.0, 40.0)));
    }

    #[test]
    fn test_path_builder() {
        let mut builder = PathBuilder::new();
        builder
            .move_to(10.0, 20.0)
            .line_to(30.0, 40.0)
            .line_to(50.0, 60.0)
            .close();

        let path = builder.build();
        assert_eq!(path.len(), 4);
    }

    #[test]
    fn test_implicit_move_to() {
        let mut path = Path::new();
        // Line without move_to should do implicit move
        path.line_to(30.0, 40.0);
        assert_eq!(path.current_point(), Some((30.0, 40.0)));
    }
}
