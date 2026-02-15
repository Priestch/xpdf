use super::error::{PDFError, PDFResult};
use super::parser::PDFObject;
use rustc_hash::FxHashMap;

/// A single page in a PDF document.
///
/// Pages are loaded lazily - the page dictionary is fetched from the xref table
/// only when needed. This mirrors PDF.js's Page class architecture.
///
/// A page dictionary contains properties like:
/// - MediaBox: The visible area of the page
/// - Resources: Fonts, images, and other resources used by the page
/// - Contents: The content stream(s) that draw the page
/// - Parent: Reference to the parent Pages node
#[derive(Debug, Clone)]
pub struct Page {
    /// The page index (0-based)
    page_index: usize,

    /// The page dictionary
    page_dict: PDFObject,

    /// The indirect object reference for this page (if it has one)
    page_ref: Option<(u32, u32)>, // (obj_num, generation)
}

impl Page {
    /// Creates a new Page from a page dictionary.
    ///
    /// # Arguments
    /// * `page_index` - The 0-based index of this page in the document
    /// * `page_dict` - The page dictionary object
    /// * `page_ref` - Optional indirect reference (obj_num, generation) for this page
    pub fn new(page_index: usize, page_dict: PDFObject, page_ref: Option<(u32, u32)>) -> Self {
        Page {
            page_index,
            page_dict,
            page_ref,
        }
    }

    /// Returns the page index (0-based).
    pub fn index(&self) -> usize {
        self.page_index
    }

    /// Returns a reference to the page dictionary.
    pub fn dict(&self) -> &PDFObject {
        &self.page_dict
    }

    /// Returns the page's indirect object reference if it has one.
    pub fn reference(&self) -> Option<(u32, u32)> {
        self.page_ref
    }

    /// Gets a property from the page dictionary.
    ///
    /// This is a convenience method for accessing page dict entries.
    pub fn get(&self, key: &str) -> Option<&PDFObject> {
        match &self.page_dict {
            PDFObject::Dictionary(dict) => dict.get(key),
            _ => None,
        }
    }

    /// Gets the MediaBox for this page.
    ///
    /// MediaBox defines the boundaries of the physical medium on which
    /// the page is to be printed. Returns an array of 4 numbers: [llx, lly, urx, ury]
    /// representing the lower-left and upper-right corners.
    ///
    /// Note: MediaBox is inheritable, so it may be defined in a parent Pages node.
    /// For now, this just returns the MediaBox from the page dict itself.
    pub fn media_box(&self) -> Option<&PDFObject> {
        self.get("MediaBox")
    }

    /// Gets the Resources dictionary for this page.
    ///
    /// Resources is inheritable and may be defined in a parent Pages node.
    /// For now, this just returns the Resources from the page dict itself.
    pub fn resources(&self) -> Option<&PDFObject> {
        self.get("Resources")
    }

    /// Gets the Annotations for this page.
    ///
    /// Annotations can be either a single annotation or an array of annotations.
    pub fn annotations(&self) -> Option<&PDFObject> {
        self.get("Annots")
    }

    /// Gets the Contents for this page.
    ///
    /// Contents can be either a single stream or an array of streams.
    pub fn contents(&self) -> Option<&PDFObject> {
        self.get("Contents")
    }

    /// Resolve an inheritable page property, merging dictionaries across the page tree.
    /// Reference: pdf.js/src/core/core_utils.js - getInheritableProperty
    fn get_inheritable_property(
        &self,
        xref: &mut super::xref::XRef,
        key: &str,
        stop_when_found: bool,
    ) -> PDFResult<Option<PDFObject>> {
        let mut current = self.page_dict.clone();
        let mut visited_refs: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        let mut values: Vec<PDFObject> = Vec::new();

        loop {
            let dict = match &current {
                PDFObject::Dictionary(d) => d,
                _ => break,
            };

            if let Some(value) = dict.get(key) {
                let resolved = xref.fetch_if_ref(value)?;
                if stop_when_found {
                    return Ok(Some(resolved));
                }
                values.push(resolved);
            }

            let parent = match dict.get("Parent") {
                Some(parent) => parent,
                None => break,
            };

            match parent {
                PDFObject::Ref(ref_obj) => {
                    let ref_key = (ref_obj.num, ref_obj.generation);
                    if visited_refs.contains(&ref_key) {
                        return Err(PDFError::Generic(
                            "Circular reference in page tree".to_string(),
                        ));
                    }
                    visited_refs.insert(ref_key);
                    let parent_obj = xref.fetch(ref_obj.num, ref_obj.generation)?;
                    current = (*parent_obj).clone();
                }
                _ => {
                    current = parent.clone();
                }
            }
        }

        if values.is_empty() {
            return Ok(None);
        }

        if values.len() == 1 {
            return Ok(Some(values.remove(0)));
        }

        match values.first() {
            Some(PDFObject::Dictionary(_)) => Ok(Some(Self::merge_dict_values(values))),
            Some(first) => Ok(Some(first.clone())),
            None => Ok(None),
        }
    }

    fn merge_dict_values(values: Vec<PDFObject>) -> PDFObject {
        let mut merged = std::collections::HashMap::new();
        for value in values {
            if let PDFObject::Dictionary(dict) = value {
                for (key, val) in dict {
                    merged.entry(key).or_insert(val);
                }
            }
        }
        PDFObject::Dictionary(merged)
    }

    fn resolve_rect(value: &PDFObject) -> Option<[f64; 4]> {
        let PDFObject::Array(arr) = value else {
            return None;
        };
        if arr.len() < 4 {
            return None;
        }
        let mut vals = [0.0; 4];
        for i in 0..4 {
            vals[i] = match &**&arr[i] {
                PDFObject::Number(n) => *n,
                _ => return None,
            };
        }
        let (min_x, max_x) = if vals[0] <= vals[2] {
            (vals[0], vals[2])
        } else {
            (vals[2], vals[0])
        };
        let (min_y, max_y) = if vals[1] <= vals[3] {
            (vals[1], vals[3])
        } else {
            (vals[3], vals[1])
        };
        Some([min_x, min_y, max_x, max_y])
    }

    pub(crate) fn resolve_view_box_for_rendering(
        &self,
        xref: &mut super::xref::XRef,
    ) -> [f64; 4] {
        // Reference: pdf.js/src/core/document.js - Page.view
        let media_box = self
            .get_inheritable_property(xref, "MediaBox", false)
            .ok()
            .and_then(|value| value.as_ref().and_then(Self::resolve_rect))
            .unwrap_or([0.0, 0.0, 612.0, 792.0]);

        let crop_box = self
            .get_inheritable_property(xref, "CropBox", false)
            .ok()
            .and_then(|value| value.as_ref().and_then(Self::resolve_rect))
            .unwrap_or(media_box);

        let intersect = [
            media_box[0].max(crop_box[0]),
            media_box[1].max(crop_box[1]),
            media_box[2].min(crop_box[2]),
            media_box[3].min(crop_box[3]),
        ];

        if intersect[2] - intersect[0] > 0.0 && intersect[3] - intersect[1] > 0.0 {
            intersect
        } else {
            media_box
        }
    }

    pub(crate) fn resolve_rotate_for_rendering(&self, xref: &mut super::xref::XRef) -> i32 {
        // Reference: pdf.js/src/core/document.js - Page.rotate
        let mut rotate = self
            .get_inheritable_property(xref, "Rotate", false)
            .ok()
            .and_then(|value| value.as_ref().and_then(|obj| match obj {
                PDFObject::Number(n) => Some(*n as i32),
                _ => None,
            }))
            .unwrap_or(0);

        if rotate % 90 != 0 {
            rotate = 0;
        } else if rotate >= 360 {
            rotate %= 360;
        } else if rotate < 0 {
            rotate = ((rotate % 360) + 360) % 360;
        }

        rotate
    }

    pub(crate) fn get_inheritable_resources(
        &self,
        xref: &mut super::xref::XRef,
    ) -> PDFResult<Option<PDFObject>> {
        // Reference: pdf.js/src/core/document.js - Page.resources
        self.get_inheritable_property(xref, "Resources", false)
    }

    fn merge_stream_resources(
        &self,
        xref: &mut super::xref::XRef,
        stream_resources: Option<&PDFObject>,
        page_resources: &Option<PDFObject>,
    ) -> PDFResult<Option<PDFObject>> {
        // Reference: pdf.js/src/core/document.js - #getMergedResources
        let local_resources = match stream_resources {
            Some(value) => match xref.fetch_if_ref(value)? {
                PDFObject::Dictionary(dict) if !dict.is_empty() => {
                    Some(PDFObject::Dictionary(dict))
                }
                _ => None,
            },
            None => None,
        };

        match (local_resources, page_resources) {
            (Some(local), Some(page)) => {
                let merged = Self::merge_dict_values(vec![local, page.clone()]);
                Ok(Some(merged))
            }
            (Some(local), None) => Ok(Some(local)),
            (None, Some(page)) => Ok(Some(page.clone())),
            (None, None) => Ok(None),
        }
    }

    #[cfg(feature = "rendering")]
    fn load_fonts_for_rendering_with_resources<D: crate::rendering::Device>(
        &self,
        xref: &mut super::xref::XRef,
        device: &mut D,
        resources: Option<&PDFObject>,
    ) -> PDFResult<()> {
        let resources = match resources {
            Some(r) => r,
            None => return Ok(()),
        };

        let resources_dict = match resources {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(()),
        };

        let font_entry = match resources_dict.get("Font") {
            Some(f) => f,
            None => return Ok(()),
        };

        let font_dict = match xref.fetch_if_ref(font_entry)? {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(()),
        };

        for (font_name, font_ref) in font_dict {
            if let Ok(font_obj) = xref.fetch_if_ref(&font_ref) {
                if let Ok(pdf_font) = super::font::Font::new(font_obj, xref) {
                    if let Some(embedded_data) = pdf_font.embedded_font {
                        if let Err(e) = device.load_font_data(&font_name, embedded_data, None) {
                            if !e.to_string().contains("UnknownMagic") {
                                eprintln!(
                                    "Warning: Failed to load embedded font '{}': {}",
                                    font_name, e
                                );
                            }
                        }
                    } else if let Some(fallback_data) = Self::get_fallback_font_data(pdf_font.base_font()) {
                        if let Err(e) = device.load_font_data(&font_name, fallback_data, None) {
                            eprintln!(
                                "Warning: Failed to load fallback font for '{}': {}",
                                font_name, e
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Extracts text from this page's content streams.
    ///
    /// This method processes all content streams for the page and extracts
    /// text content with position and font information.
    ///
    /// # Returns
    /// A vector of TextItem objects containing the extracted text
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// let pdf_data = std::fs::read("document.pdf").unwrap();
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    ///
    /// let text_items = page.extract_text(&mut doc).unwrap();
    /// for item in text_items {
    ///     println!("Text: '{}' at {:?}", item.text, item.position);
    /// }
    /// ```
    pub fn extract_text(
        &self,
        xref: &mut super::xref::XRef,
    ) -> PDFResult<Vec<super::content_stream::TextItem>> {
        use super::decode::decode_flate;
        use super::{ContentStreamEvaluator, Lexer, Stream};

        let contents = match self.contents() {
            Some(contents) => contents,
            None => return Ok(Vec::new()), // No content streams
        };

        // Dereference if it's a reference
        let contents = xref.fetch_if_ref(contents)?;

        let mut all_text_items = Vec::new();

        // Handle single content stream
        let content_streams = match contents {
            PDFObject::Stream { dict, data } => {
                vec![(dict.clone(), data.clone())]
            }
            PDFObject::Array(arr) => {
                // Multiple content streams - fetch each one
                let mut streams = Vec::new();
                for content_obj in &arr {
                    match xref.fetch_if_ref(content_obj)? {
                        PDFObject::Stream { dict, data } => {
                            streams.push((dict, data));
                        }
                        _ => {
                            return Err(super::PDFError::Generic(
                                "Contents array contains non-stream object".to_string(),
                            ));
                        }
                    }
                }
                streams
            }
            _ => {
                // Handle unexpected Contents types gracefully
                // Some PDFs may have null Contents, references to null, or other formats
                // This commonly occurs with image-only pages or empty pages
                return Ok(Vec::new());
            }
        };

        // Process each content stream
        for (dict, data) in content_streams {
            // Decode the stream if it's compressed
            let decoded_data = if let Some(filter) = dict.get("Filter") {
                match filter {
                    PDFObject::Name(filter_name) if filter_name == "FlateDecode" => {
                        // Decompress FlateDecode stream
                        match decode_flate(&data) {
                            Ok(decompressed) => decompressed,
                            Err(_) => continue, // Skip this stream if decompression fails
                        }
                    }
                    _ => data, // Other filters not yet supported, use raw data
                }
            } else {
                data // No filter, use raw data
            };

            // Create a stream from the (decoded) content data
            let stream = Box::new(Stream::from_bytes(decoded_data)) as Box<dyn super::BaseStream>;
            let lexer = Lexer::new(stream)?;
            let parser = super::Parser::new(lexer)?;
            let mut evaluator = ContentStreamEvaluator::new(parser);

            // Load fonts from page resources (for proper character encoding)
            if let Some(resources) = self.resources() {
                // Ignore font loading errors - text extraction will still work with fallback encoding
                let _ = evaluator.load_fonts(resources, xref);
            }

            // Extract text from this stream
            let text_items = evaluator.extract_text()?;
            all_text_items.extend(text_items);
        }

        Ok(all_text_items)
    }

    /// Extracts all text from the page as a single string.
    ///
    /// This is a convenience method that extracts text items and joins them
    /// into a single string, sorted by position (top to bottom, left to right).
    ///
    /// # Returns
    /// A single string containing all the text from the page
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// let mut doc = PDFDocument::open_file("document.pdf", None, None).unwrap();
    /// let page = doc.get_page(0).unwrap();
    ///
    /// let text = page.extract_text_as_string(&mut doc.xref_mut()).unwrap();
    /// println!("Page text:\n{}", text);
    /// ```
    pub fn extract_text_as_string(&self, xref: &mut super::xref::XRef) -> PDFResult<String> {
        let mut text_items = self.extract_text(xref)?;

        // Sort text items by position (top to bottom, left to right)
        // Y-axis in PDF goes bottom to top, so we sort by descending Y, then ascending X
        text_items.sort_by(|a, b| {
            match (a.position, b.position) {
                (Some((x1, y1)), Some((x2, y2))) => {
                    // First sort by Y (descending - top to bottom)
                    let y_cmp = y2.partial_cmp(&y1).unwrap_or(std::cmp::Ordering::Equal);
                    if y_cmp != std::cmp::Ordering::Equal {
                        y_cmp
                    } else {
                        // Then sort by X (ascending - left to right)
                        x1.partial_cmp(&x2).unwrap_or(std::cmp::Ordering::Equal)
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });

        // Group text items into lines based on Y position
        let mut result = String::new();
        let mut last_y: Option<f64> = None;
        let line_threshold = 2.0; // Y-distance threshold to consider same line

        for item in text_items {
            if let Some((_, y)) = item.position {
                if let Some(prev_y) = last_y {
                    // If Y position changed significantly, start a new line
                    if (y - prev_y).abs() > line_threshold {
                        result.push('\n');
                    } else {
                        // Same line, add a space between items
                        if !result.is_empty() && !result.ends_with(' ') && !result.ends_with('\n') {
                            result.push(' ');
                        }
                    }
                }
                last_y = Some(y);
            }

            result.push_str(&item.text);
        }

        Ok(result)
    }

    /// Renders this page to a rendering device.
    ///
    /// This method processes all content streams for the page and renders
    /// them using the provided rendering device. The device handles the actual
    /// drawing operations (canvas, image export, etc.).
    ///
    /// # Arguments
    /// * `xref` - The cross-reference table for fetching objects
    /// * `device` - A mutable reference to a rendering device
    ///
    /// # Returns
    /// Ok(()) if rendering succeeded, or an error if something went wrong
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x_core::PDFDocument;
    /// use pdf_x_core::rendering::{SkiaDevice, Device};
    /// use tiny_skia::Pixmap;
    ///
    /// let pdf_data = std::fs::read("document.pdf").unwrap();
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    ///
    /// // Create a pixmap for rendering
    /// let mut pixmap = Pixmap::new(800, 600).unwrap();
    /// let mut device = SkiaDevice::new(pixmap.as_mut());
    ///
    /// // Render the page
    /// page.render(&mut doc.xref_mut(), &mut device).unwrap();
    ///
    /// // Save the result
    /// pixmap.save_png("page1.png").unwrap();
    /// ```
    pub fn render<D: crate::rendering::Device>(
        &self,
        xref: &mut super::xref::XRef,
        device: &mut D,
    ) -> PDFResult<()> {
        use super::{Lexer, Parser, Stream};
        use crate::rendering::RenderingContext;

        // Reference: pdf.js/src/core/document.js - Page.view (MediaBox/CropBox handling)
        let view_box = self.resolve_view_box_for_rendering(xref);

        let contents = match self.contents() {
            Some(contents) => contents,
            None => return Ok(()), // No content streams to render
        };

        // Dereference if it's a reference
        let contents = xref.fetch_if_ref(contents)?;

        // Handle single content stream or array of streams
        let content_streams = match contents {
            PDFObject::Stream { dict, data } => {
                vec![(dict.clone(), data.clone())]
            }
            PDFObject::Array(arr) => {
                // Multiple content streams - fetch each one
                let mut streams = Vec::new();
                let mut seen_streams = std::collections::HashSet::new();
                for content_obj in &arr {
                    // Track which object references we've already processed
                    // to avoid duplicate rendering (common in some PDFs)
                    let obj_key = match content_obj.as_ref() {
                        PDFObject::Ref(r) => format!("{}+{}", r.num, r.generation),
                        _ => format!("indirect_{}", streams.len()),
                    };

                    if seen_streams.contains(&obj_key) {
                        eprintln!(
                            "Warning: Skipping duplicate content stream reference: {}",
                            obj_key
                        );
                        continue;
                    }
                    seen_streams.insert(obj_key);

                    match xref.fetch_if_ref(content_obj)? {
                        PDFObject::Stream { dict, data } => {
                            streams.push((dict, data));
                        }
                        _ => {
                            return Err(super::PDFError::Generic(
                                "Contents array contains non-stream object".to_string(),
                            ));
                        }
                    }
                }
                streams
            }
            _ => {
                // Handle unexpected Contents types gracefully
                return Ok(());
            }
        };

        // Set initial clip to the view box to prevent rendering outside page bounds
        let [x0, y0, x1, y1] = view_box;
        device.begin_path();
        device.move_to(x0, y0);
        device.line_to(x1, y0);
        device.line_to(x1, y1);
        device.line_to(x0, y1);
        device.close_path();
        device.clip_path(crate::rendering::graphics_state::FillRule::NonZero)?;

        // Resolve inheritable page resources once (fonts, XObjects, etc.)
        // Reference: pdf.js/src/core/document.js - #getInheritableProperty("Resources")
        let page_resources = self.get_inheritable_resources(xref)?;

        // Process each content stream
        let mut total_operations = 0;
        for (stream_idx, (dict, data)) in content_streams.into_iter().enumerate() {
            // Save device state before processing this stream
            // This ensures each stream starts with the same CTM
            device.save_state();

            // Decode the stream if it has filters
            let decoded_data = if let Some(filter) = dict.get("Filter") {
                match super::decode::apply_filters(&data, filter) {
                    Ok(decoded) => decoded,
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to decode content stream {}: {}",
                            stream_idx, e
                        );
                        device.restore_state();
                        continue; // Skip this stream if decoding fails
                    }
                }
            } else {
                data.to_vec() // No filter, use raw data
            };

            eprintln!(
                "Info: Processing content stream {} ({} bytes)",
                stream_idx,
                decoded_data.len()
            );

            // Create a stream from the (decoded) content data
            let stream = Box::new(Stream::from_bytes(decoded_data)) as Box<dyn super::BaseStream>;
            let lexer = Lexer::new(stream)?;
            let parser = Parser::new(lexer)?;
            let mut evaluator = super::content_stream::ContentStreamEvaluator::new(parser);

            // Merge any stream-level Resources with page-level Resources.
            // Reference: pdf.js/src/core/document.js - #getMergedResources
            let resources = self.merge_stream_resources(xref, dict.get("Resources"), &page_resources)?;

            // Load fonts from merged resources (for proper text rendering)
            #[cfg(feature = "rendering")]
            self.load_fonts_for_rendering_with_resources(xref, device, resources.as_ref())?;

            // Create a rendering context to process operations
            let mut ctx = RenderingContext::new(device);

            // Set xref and resources for XObject (image) rendering
            // Note: We need to extend the lifetime of the fetched resources
            // by storing it and passing a reference to the context
            if let Some(ref resources_obj) = resources {
                ctx.set_xobject_resources(xref, resources_obj);
            }

            // Parse and process each operation in the content stream
            let mut stream_operations = 0;
            loop {
                match evaluator.read_operation() {
                    Ok(Some(op)) => {
                        stream_operations += 1;
                        if let Err(e) = ctx.process_operation(&op) {
                            // Log but continue processing - one bad operator shouldn't stop entire rendering
                            eprintln!("Warning: Failed to process operator {:?}: {}", op.op, e);
                        }
                    }
                    Ok(None) => break, // End of stream
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to read operation in stream {}, stopping: {}",
                            stream_idx, e
                        );
                        break; // Can't continue after a read error
                    }
                }
            }
            eprintln!(
                "Info: Processed {} operations in stream {}",
                stream_operations, stream_idx
            );
            total_operations += stream_operations;

            // Restore device state after processing this stream
            // This resets the CTM to the state before this stream
            device.restore_state();
        }
        eprintln!(
            "Info: Total {} operations processed for page {}",
            total_operations, self.page_index
        );

        Ok(())
    }

    // ========== Font Loading Methods ==========

    /// Gets all fonts from the page's Resources dictionary.
    ///
    /// Returns a mapping of font names (like "F1", "F2") to font dictionaries.
    pub fn get_fonts(
        &self,
        xref: &mut super::xref::XRef,
    ) -> PDFResult<Vec<(String, super::PDFObject)>> {
        use super::PDFObject;

        let resources = match self.resources() {
            Some(r) => xref.fetch_if_ref(r)?,
            None => return Ok(Vec::new()),
        };

        let resources_dict = match resources {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(Vec::new()),
        };

        let font_dict = match resources_dict.get("Font") {
            Some(f) => xref.fetch_if_ref(f)?,
            None => return Ok(Vec::new()),
        };

        match font_dict {
            PDFObject::Dictionary(d) => {
                let mut fonts = Vec::new();
                for (name, font_ref) in d.iter() {
                    fonts.push((name.clone(), font_ref.clone()));
                }
                Ok(fonts)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Loads all fonts from the page into the device for rendering.
    ///
    /// This method:
    /// 1. Extracts all fonts from the page's Resources
    /// 2. Loads embedded font data (if available)
    /// 3. For standard fonts (Helvetica, Times, Courier), attempts to use fallback fonts
    ///
    /// # Arguments
    /// * `xref` - Cross-reference table for fetching objects
    /// * `device` - The device to load fonts into
    ///
    /// # Returns
    /// Ok(()) if successful, even if some fonts fail to load
    #[cfg(feature = "rendering")]
    pub fn load_fonts_for_rendering<D: crate::rendering::Device>(
        &self,
        xref: &mut super::xref::XRef,
        device: &mut D,
    ) -> PDFResult<()> {
        let fonts = self.get_fonts(xref)?;

        for (font_name, font_ref) in fonts {
            // Try to fetch and load the font
            match xref.fetch_if_ref(&font_ref) {
                Ok(font_obj) => {
                    // Try to create a Font and extract embedded data
                    match super::font::Font::new(font_obj, xref) {
                        Ok(pdf_font) => {
                            // If we have embedded font data, load it
                            if let Some(embedded_data) = pdf_font.embedded_font {
                                // Note: page.rs doesn't have access to the encoding dictionary here
                                // The rendering context will handle encoding properly
                                if let Err(e) =
                                    device.load_font_data(&font_name, embedded_data, None)
                                {
                                    // Only log for non-Type1 fonts (Type1 "UnknownMagic" is expected)
                                    if !e.to_string().contains("UnknownMagic") {
                                        eprintln!(
                                            "Warning: Failed to load embedded font '{}': {}",
                                            font_name, e
                                        );
                                    }
                                }
                            } else {
                                // No embedded font - try to use a fallback for standard fonts
                                let base_font = pdf_font.base_font();
                                if let Some(fallback_data) = Self::get_fallback_font_data(base_font)
                                {
                                    if let Err(e) =
                                        device.load_font_data(&font_name, fallback_data, None)
                                    {
                                        eprintln!(
                                            "Warning: Failed to load fallback font for '{}': {}",
                                            font_name, e
                                        );
                                    }
                                }
                                // Silently skip fonts without embedded data or fallback
                            }
                        }
                        Err(_) => {
                            // Silently skip fonts that fail to parse (Type1 fonts are expected to fail)
                        }
                    }
                }
                Err(_) => {
                    // Silently skip fonts that fail to fetch
                }
            }
        }

        Ok(())
    }

    /// Gets fallback font data for standard PDF fonts.
    ///
    /// Returns font data for common PDF base fonts (Helvetica, Times, Courier, etc.)
    /// by loading bundled Liberation fonts (which are metric-compatible with PDF standard fonts).
    #[cfg(feature = "rendering")]
    fn get_fallback_font_data(base_font: &str) -> Option<Vec<u8>> {
        // Normalize font name (remove suffixes like -Bold, -Italic, etc.)
        let font_name = base_font.split('-').next().unwrap_or(base_font);

        // Map standard PDF fonts to bundled Liberation fonts
        // Liberation fonts are metric-compatible with standard PDF fonts and are OFL licensed
        let fallback_font = match font_name {
            // Helvetica family -> Liberation Sans
            "Helvetica" | "Arial" | "sans-serif" => "LiberationSans-Regular.ttf",
            "Helvetica-Bold" | "Arial-Bold" => "LiberationSans-Bold.ttf",
            "Helvetica-Oblique" | "Helvetica-Italic" | "Arial-Italic" => {
                "LiberationSans-Italic.ttf"
            }
            "Helvetica-BoldOblique" | "Helvetica-BoldItalic" | "Arial-BoldItalic" => {
                "LiberationSans-BoldItalic.ttf"
            }

            // Times family -> Liberation Serif
            "Times" | "Times-Roman" | "serif" => "LiberationSerif-Regular.ttf",
            "Times-Bold" => "LiberationSerif-Bold.ttf",
            "Times-Italic" | "Times-Roman-Italic" => "LiberationSerif-Italic.ttf",
            "Times-BoldItalic" | "Times-Roman-BoldItalic" => "LiberationSerif-BoldItalic.ttf",

            // Courier family -> Liberation Mono
            "Courier" | "CourierNew" | "monospace" => "LiberationMono-Regular.ttf",
            "Courier-Bold" | "CourierNew-Bold" => "LiberationMono-Bold.ttf",
            "Courier-Oblique" | "Courier-Italic" | "CourierNew-Italic" => {
                "LiberationMono-Italic.ttf"
            }
            "Courier-BoldOblique" | "Courier-BoldItalic" | "CourierNew-BoldItalic" => {
                "LiberationMono-BoldItalic.ttf"
            }

            // Symbol and ZapfDingbats don't have Liberation equivalents, return None
            "Symbol" | "ZapfDingbats" => return None,

            _ => return None,
        };

        // Load the font from bundled assets
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("fonts")
            .join(fallback_font);

        match std::fs::read(&font_path) {
            Ok(data) => {
                #[cfg(feature = "debug-logging")]
                eprintln!(
                    "DEBUG: Loaded fallback font '{}' for '{}' ({} bytes)",
                    fallback_font,
                    base_font,
                    data.len()
                );
                Some(data)
            }
            Err(e) => {
                eprintln!(
                    "Warning: Failed to load fallback font '{}': {}",
                    font_path.display(),
                    e
                );
                None
            }
        }
    }
}

/// Page tree cache for efficient page lookups.
///
/// The page tree in a PDF can be deeply nested. To avoid re-traversing
/// the tree on every page access, we cache:
/// - Page dictionaries by index
/// - Kid counts for intermediate nodes (for skipping branches)
///
/// This mirrors PDF.js's caching strategy in the Catalog class.
#[derive(Debug)]
pub struct PageTreeCache {
    /// Cache of page dictionaries by page index
    pages: FxHashMap<usize, Page>,
}

impl PageTreeCache {
    /// Creates a new empty page tree cache.
    pub fn new() -> Self {
        PageTreeCache {
            pages: FxHashMap::default(),
        }
    }

    /// Gets a cached page by index.
    pub fn get(&self, page_index: usize) -> Option<&Page> {
        self.pages.get(&page_index)
    }

    /// Caches a page.
    pub fn put(&mut self, page_index: usize, page: Page) {
        self.pages.insert(page_index, page);
    }

    /// Checks if a page is cached.
    pub fn has(&self, page_index: usize) -> bool {
        self.pages.contains_key(&page_index)
    }

    /// Clears the cache.
    pub fn clear(&mut self) {
        self.pages.clear();
    }
}

impl Default for PageTreeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl Page {
    /// Extract image metadata from the page without full decoding.
    ///
    /// This method scans the page's Resources/XObject dictionary and extracts
    /// metadata for all images (XObjects with Subtype /Image).
    ///
    /// # Arguments
    /// * `xref` - Mutable reference to the XRef table for resolving object references
    ///
    /// # Returns
    /// Vector of ImageMetadata structures containing image information
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let images = page.get_image_metadata(&mut doc.xref_mut()).unwrap();
    ///
    /// for image in images {
    ///     println!("Image: {} ({}x{}, {})",
    ///              image.name, image.width, image.height, image.format);
    /// }
    /// ```
    pub fn get_image_metadata(
        &self,
        xref: &mut super::xref::XRef,
    ) -> PDFResult<Vec<super::image::ImageMetadata>> {
        use super::image::{ImageDecoder, ImageFormat, ImageMetadata};

        let mut images = Vec::new();

        // Get the Resources dictionary from the page
        let resources = match self.resources() {
            Some(res) => self.fetch_if_ref(res, xref)?,
            None => return Ok(images), // No resources
        };

        let resources_dict = match resources {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(images), // Resources is not a dictionary
        };

        // Get XObject dictionary from Resources
        let xobject_entry = match resources_dict.get("XObject") {
            Some(entry) => entry,
            None => return Ok(images), // No XObjects
        };

        let xobject_dict = match self.fetch_if_ref(xobject_entry, xref)? {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(images), // XObject is not a dictionary
        };

        // Iterate through all XObjects
        for (name, xobject_ref) in &xobject_dict {
            // Resolve the XObject reference
            let xobject = self.fetch_if_ref(xobject_ref, xref)?;

            // Check if it's an image (Subtype == /Image)
            if let PDFObject::Stream { dict, data } = xobject {
                // Check Subtype
                let is_image = dict
                    .get("Subtype")
                    .and_then(|subtype| match subtype {
                        PDFObject::Name(n) => Some(n == "Image"),
                        _ => None,
                    })
                    .unwrap_or(false);

                if !is_image {
                    continue; // Not an image XObject (could be Form)
                }

                // Extract metadata
                let width = dict
                    .get("Width")
                    .or_else(|| dict.get("W"))
                    .and_then(|w| match w {
                        PDFObject::Number(n) => Some(*n as u32),
                        _ => None,
                    })
                    .unwrap_or(0);

                let height = dict
                    .get("Height")
                    .or_else(|| dict.get("H"))
                    .and_then(|h| match h {
                        PDFObject::Number(n) => Some(*n as u32),
                        _ => None,
                    })
                    .unwrap_or(0);

                let bits_per_component = dict
                    .get("BitsPerComponent")
                    .or_else(|| dict.get("BPC"))
                    .and_then(|bpc| match bpc {
                        PDFObject::Number(n) => Some(*n as u8),
                        _ => None,
                    })
                    .unwrap_or(8);

                // Get color space
                let color_space = dict
                    .get("ColorSpace")
                    .or_else(|| dict.get("CS"))
                    .map(|cs| match cs {
                        PDFObject::Name(name) => name.clone(),
                        PDFObject::Array(arr) => arr
                            .get(0)
                            .and_then(|obj| match &**obj {
                                PDFObject::Name(n) => Some(n.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| "Unknown".to_string()),
                        _ => "Unknown".to_string(),
                    })
                    .unwrap_or_else(|| "Unknown".to_string());

                // Detect image format from filter
                let format = dict
                    .get("Filter")
                    .or_else(|| dict.get("F"))
                    .map(|filter| match filter {
                        PDFObject::Name(name) => match name.as_str() {
                            "DCTDecode" => ImageFormat::JPEG,
                            "JPXDecode" => ImageFormat::JPEG2000,
                            "JBIG2Decode" => ImageFormat::JBIG2,
                            "FlateDecode" => ImageFormat::Raw,
                            _ => ImageFormat::Unknown,
                        },
                        PDFObject::Array(arr) => {
                            // Multiple filters - check the first one
                            arr.get(0)
                                .and_then(|obj| match &**obj {
                                    PDFObject::Name(n) => match n.as_str() {
                                        "DCTDecode" => Some(ImageFormat::JPEG),
                                        "JPXDecode" => Some(ImageFormat::JPEG2000),
                                        "JBIG2Decode" => Some(ImageFormat::JBIG2),
                                        "FlateDecode" => Some(ImageFormat::Raw),
                                        _ => Some(ImageFormat::Unknown),
                                    },
                                    _ => None,
                                })
                                .unwrap_or(ImageFormat::Unknown)
                        }
                        _ => ImageFormat::Unknown,
                    })
                    .unwrap_or_else(|| {
                        // No filter - try to detect from data
                        ImageDecoder::detect_format(&data)
                    });

                // Check for SMask (soft mask / alpha channel)
                let has_alpha = dict.get("SMask").is_some();

                let metadata = ImageMetadata {
                    name: name.clone(),
                    format,
                    width,
                    height,
                    bits_per_component,
                    color_space,
                    has_alpha,
                    data_length: Some(data.len()),
                };

                images.push(metadata);
            }
        }

        Ok(images)
    }

    /// Extract complete images with full decoding.
    ///
    /// This method extracts and decodes all images from the page, returning
    /// complete pixel data ready for use.
    ///
    /// # Arguments
    /// * `xref` - Mutable reference to the XRef table for resolving object references
    ///
    /// # Returns
    /// Vector of DecodedImage structures containing pixel data
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let images = page.extract_images(&mut doc.xref_mut()).unwrap();
    ///
    /// for (i, image) in images.iter().enumerate() {
    ///     println!("Image {}: {}x{} pixels, {} bytes",
    ///              i, image.width, image.height, image.data.len());
    ///     // Save or process image.data...
    /// }
    /// ```
    pub fn extract_images(
        &self,
        xref: &mut super::xref::XRef,
    ) -> PDFResult<Vec<super::image::DecodedImage>> {
        use super::decode;
        use super::image::{ImageColorSpace, ImageDecoder};

        let mut decoded_images = Vec::new();

        // Get the Resources dictionary from the page
        let resources = match self.resources() {
            Some(res) => self.fetch_if_ref(res, xref)?,
            None => return Ok(decoded_images), // No resources
        };

        let resources_dict = match resources {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(decoded_images), // Resources is not a dictionary
        };

        // Get XObject dictionary from Resources
        let xobject_entry = match resources_dict.get("XObject") {
            Some(entry) => entry,
            None => return Ok(decoded_images), // No XObjects
        };

        let xobject_dict = match self.fetch_if_ref(xobject_entry, xref)? {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(decoded_images), // XObject is not a dictionary
        };

        // Iterate through all XObjects
        for (name, xobject_ref) in &xobject_dict {
            // Resolve the XObject reference
            let xobject = self.fetch_if_ref(xobject_ref, xref)?;

            // Check if it's an image (Subtype == /Image)
            if let PDFObject::Stream { dict, data } = xobject {
                // Check Subtype
                let is_image = dict
                    .get("Subtype")
                    .and_then(|subtype| match subtype {
                        PDFObject::Name(n) => Some(n == "Image"),
                        _ => None,
                    })
                    .unwrap_or(false);

                if !is_image {
                    continue; // Not an image XObject
                }

                // Get filter to determine decoding strategy
                let filter = dict.get("Filter").or_else(|| dict.get("F"));

                // Decode the image stream based on filter
                match filter {
                    Some(PDFObject::Name(filter_name)) => {
                        match filter_name.as_str() {
                            "DCTDecode" => {
                                // JPEG - decode directly
                                match ImageDecoder::decode_image(
                                    &data,
                                    super::image::ImageFormat::JPEG,
                                ) {
                                    Ok(img) => decoded_images.push(img),
                                    Err(e) => {
                                        eprintln!(
                                            "Warning: Failed to decode JPEG image '{}': {}",
                                            name, e
                                        );
                                    }
                                }
                            }
                            "JPXDecode" => {
                                // JPEG2000 - decode directly
                                match ImageDecoder::decode_image(
                                    &data,
                                    super::image::ImageFormat::JPEG2000,
                                ) {
                                    Ok(img) => decoded_images.push(img),
                                    Err(e) => {
                                        eprintln!(
                                            "Warning: Failed to decode JPEG2000 image '{}': {}",
                                            name, e
                                        );
                                    }
                                }
                            }
                            "JBIG2Decode" => {
                                // JBIG2 - decode directly
                                match ImageDecoder::decode_image(
                                    &data,
                                    super::image::ImageFormat::JBIG2,
                                ) {
                                    Ok(img) => decoded_images.push(img),
                                    Err(e) => {
                                        eprintln!(
                                            "Warning: Failed to decode JBIG2 image '{}': {}",
                                            name, e
                                        );
                                    }
                                }
                            }
                            "FlateDecode" => {
                                // Decompress first, then decode as raw image
                                match decode::decode_flate(&data) {
                                    Ok(decompressed) => {
                                        // Get image parameters for raw decoding
                                        let width = dict
                                            .get("Width")
                                            .or_else(|| dict.get("W"))
                                            .and_then(|w| match w {
                                                PDFObject::Number(n) => Some(*n as u32),
                                                _ => None,
                                            })
                                            .unwrap_or(0);

                                        let height = dict
                                            .get("Height")
                                            .or_else(|| dict.get("H"))
                                            .and_then(|h| match h {
                                                PDFObject::Number(n) => Some(*n as u32),
                                                _ => None,
                                            })
                                            .unwrap_or(0);

                                        let bpc = dict
                                            .get("BitsPerComponent")
                                            .or_else(|| dict.get("BPC"))
                                            .and_then(|bpc| match bpc {
                                                PDFObject::Number(n) => Some(*n as u8),
                                                _ => None,
                                            })
                                            .unwrap_or(8);

                                        let color_space = dict
                                            .get("ColorSpace")
                                            .or_else(|| dict.get("CS"))
                                            .map(|cs| ImageDecoder::parse_color_space(cs))
                                            .unwrap_or(ImageColorSpace::RGB);

                                        // Decode as raw image
                                        match ImageDecoder::decode_raw_image(
                                            &decompressed,
                                            width,
                                            height,
                                            bpc,
                                            color_space,
                                        ) {
                                            Ok(img) => decoded_images.push(img),
                                            Err(e) => {
                                                eprintln!(
                                                    "Warning: Failed to decode raw image '{}': {}",
                                                    name, e
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "Warning: Failed to decompress FlateDecode image '{}': {}",
                                            name, e
                                        );
                                    }
                                }
                            }
                            other => {
                                eprintln!(
                                    "Warning: Unsupported image filter '{}' for image '{}'",
                                    other, name
                                );
                            }
                        }
                    }
                    Some(PDFObject::Array(_arr)) => {
                        eprintln!(
                            "Warning: Multiple filters not yet supported for image '{}'",
                            name
                        );
                    }
                    None => {
                        // No filter - raw uncompressed image data
                        let width = dict
                            .get("Width")
                            .or_else(|| dict.get("W"))
                            .and_then(|w| match w {
                                PDFObject::Number(n) => Some(*n as u32),
                                _ => None,
                            })
                            .unwrap_or(0);

                        let height = dict
                            .get("Height")
                            .or_else(|| dict.get("H"))
                            .and_then(|h| match h {
                                PDFObject::Number(n) => Some(*n as u32),
                                _ => None,
                            })
                            .unwrap_or(0);

                        let bpc = dict
                            .get("BitsPerComponent")
                            .or_else(|| dict.get("BPC"))
                            .and_then(|bpc| match bpc {
                                PDFObject::Number(n) => Some(*n as u8),
                                _ => None,
                            })
                            .unwrap_or(8);

                        let color_space = dict
                            .get("ColorSpace")
                            .or_else(|| dict.get("CS"))
                            .map(|cs| ImageDecoder::parse_color_space(cs))
                            .unwrap_or(ImageColorSpace::RGB);

                        // Decode as raw image
                        match ImageDecoder::decode_raw_image(&data, width, height, bpc, color_space)
                        {
                            Ok(img) => decoded_images.push(img),
                            Err(e) => {
                                eprintln!("Warning: Failed to decode raw image '{}': {}", name, e);
                            }
                        }
                    }
                    Some(_) => {
                        eprintln!("Warning: Unexpected filter type for image '{}'", name);
                    }
                }
            }
        }

        Ok(decoded_images)
    }
}

impl Page {
    /// Helper method to fetch an object, following references if needed.
    /// This provides access to the document's xref table for resolving object references.
    /// Note: This is a simplified implementation that requires document context.
    pub fn fetch_if_ref(
        &self,
        obj: &PDFObject,
        xref: &mut super::xref::XRef,
    ) -> PDFResult<PDFObject> {
        match obj {
            PDFObject::Ref(ref_obj) => match xref.fetch(ref_obj.num, ref_obj.generation)? {
                rc_obj => Ok((*rc_obj).clone()),
            },
            _ => Ok(obj.clone()),
        }
    }

    /// Helper method to get XObject data by name.
    /// This retrieves the actual image data from the page's XObject resources.
    pub fn get_xobject_data(&self, name: &str, xref: &mut super::xref::XRef) -> PDFResult<Vec<u8>> {
        // Get the Resources dictionary from the page
        if let Some(resources) = self.resources() {
            // Look for XObject dictionary
            if let PDFObject::Dictionary(xobject_dict) = resources {
                // Check if there's an XObject entry
                if let Some(xobject_entry) = xobject_dict.get("XObject") {
                    // Follow reference if needed
                    let xobject_dict = match self.fetch_if_ref(xobject_entry, xref)? {
                        PDFObject::Dictionary(dict) => dict,
                        _ => {
                            return Err(PDFError::Generic(
                                "XObject is not a dictionary".to_string(),
                            ));
                        }
                    };

                    // Find the XObject with the given name
                    for (name_key, xobject_ref) in xobject_dict {
                        if name_key == name {
                            // Follow the reference to get the actual XObject
                            let xobject = self.fetch_if_ref(&xobject_ref, xref)?;

                            // Check if it's an image stream and return the data
                            if let PDFObject::Stream { dict: _, data } = xobject {
                                return Ok(data);
                            } else {
                                return Err(PDFError::Generic(format!(
                                    "XObject '{}' is not a stream",
                                    name
                                )));
                            }
                        }
                    }
                    return Err(PDFError::Generic(format!(
                        "XObject '{}' not found in page resources",
                        name
                    )));
                } else {
                    return Err(PDFError::Generic(
                        "No XObject found in page resources".to_string(),
                    ));
                }
            } else {
                return Err(PDFError::Generic(
                    "Resources is not a dictionary".to_string(),
                ));
            }
        } else {
            Err(PDFError::Generic(
                "No resources dictionary found in page".to_string(),
            ))
        }
    }

    /// Extracts annotations from this page.
    ///
    /// This method parses all annotations associated with the page, including
    /// links, text notes, highlights, form fields, etc.
    ///
    /// # Arguments
    /// * `xref` - Mutable reference to the XRef table for resolving object references
    ///
    /// # Returns
    /// Vector of Annotation structures containing annotation information
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let annotations = page.extract_annotations(&mut doc.xref_mut()).unwrap();
    ///
    /// for annot in annotations {
    ///     println!("Annotation type: {:?}", annot.annotation_type);
    ///     if let Some(ref contents) = annot.contents {
    ///         println!("  Contents: {}", contents);
    ///     }
    /// }
    /// ```
    pub fn extract_annotations(
        &self,
        xref: &mut super::xref::XRef,
    ) -> PDFResult<Vec<super::annotation::Annotation>> {
        use super::annotation::parse_annotations;

        let annots = match self.annotations() {
            Some(a) => a,
            None => return Ok(Vec::new()),
        };

        parse_annotations(annots, xref)
    }
}
