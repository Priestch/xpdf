use super::content_stream::TextItem;
use super::error::{PDFError, PDFResult};
use super::parser::PDFObject;
use super::{ContentStreamEvaluator, Lexer, Stream};
use rustc_hash::FxHashMap;
use std::sync::Arc;

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

/// Object resolver used by the render/extract paths, abstracting over the legacy base-only
/// path (`&mut XRef`) and the delta-aware path (`&mut DocStore`).
///
/// `resolve_if_ref` / `resolve_ref` are delta-aware for `DocStore` (`delta → base`), so all
/// object fetches — page contents AND nested resources (fonts/xobjects/ext-gstate) — honor
/// delta overrides. `as_xref_mut` exposes the base `XRef` only for operations not expressed
/// through the resolver (e.g. stream positioning).
pub trait PageResolver {
    /// Resolve `obj` if it is an indirect Reference (delta-aware where supported).
    fn resolve_if_ref(&mut self, obj: &PDFObject) -> PDFResult<Arc<PDFObject>>;
    /// Resolve object `(num, generation)` (delta-aware where supported).
    fn resolve_ref(&mut self, num: u32, generation: u32) -> PDFResult<Arc<PDFObject>>;
    /// Base `XRef` for legacy operations not yet migrated to the resolver trait.
    /// Panics for snapshot-backed resolvers (no live XRef).
    fn as_xref_mut(&mut self) -> &mut super::xref::XRef {
        unimplemented!("this resolver has no live XRef")
    }
}

impl PageResolver for super::xref::XRef {
    fn resolve_if_ref(&mut self, obj: &PDFObject) -> PDFResult<Arc<PDFObject>> {
        self.fetch_if_ref(obj)
    }
    fn resolve_ref(&mut self, num: u32, generation: u32) -> PDFResult<Arc<PDFObject>> {
        self.fetch(num, generation)
    }
    fn as_xref_mut(&mut self) -> &mut super::xref::XRef {
        self
    }
}

impl PageResolver for super::doc_store::DocStore {
    fn resolve_if_ref(&mut self, obj: &PDFObject) -> PDFResult<Arc<PDFObject>> {
        super::doc_store::DocStore::resolve_if_ref(self, obj)
    }
    fn resolve_ref(&mut self, num: u32, generation: u32) -> PDFResult<Arc<PDFObject>> {
        super::doc_store::DocStore::resolve_ref(self, num, generation)
    }
    fn as_xref_mut(&mut self) -> &mut super::xref::XRef {
        super::doc_store::DocStore::xref_mut(self)
    }
}

impl Page {
    /// Creates a new Page from a page dictionary.
    ///
    /// # Arguments
    /// * `page_index` - The 0-based index of this page in the document
    /// * `page_dict` - The page dictionary object
    /// * `page_ref` - Optional indirect reference (obj_num, generation) for this page
    pub(crate) fn new(
        page_index: usize,
        page_dict: Arc<PDFObject>,
        page_ref: Option<(u32, u32)>,
    ) -> Self {
        Page {
            page_index,
            page_dict: (*page_dict).clone(),
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
    fn get_inheritable_property<R: PageResolver>(
        &self,
        resolver: &mut R,
        key: &str,
        stop_when_found: bool,
    ) -> PDFResult<Option<Arc<PDFObject>>> {
        let mut current = self.page_dict.clone();
        let mut visited_refs: std::collections::HashSet<(u32, u32)> =
            std::collections::HashSet::new();
        let mut values: Vec<Arc<PDFObject>> = Vec::new();

        loop {
            let dict = match &current {
                PDFObject::Dictionary(d) => d,
                _ => break,
            };

            if let Some(value) = dict.get(key) {
                let resolved = resolver.resolve_if_ref(value)?;
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
                    let parent_obj = resolver.resolve_ref(ref_obj.num, ref_obj.generation)?;
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
            return Ok(Some(Arc::clone(&values.remove(0))));
        }

        // For Dictionary values, merge keys across the inheritance chain. For all
        // other types (Array, Number, etc.) the closest value wins — mirrors pdf.js
        // `inherit` behavior for non-dict properties like MediaBox.
        match &*values[0] {
            PDFObject::Dictionary(_) => Ok(Some(Arc::new(Self::merge_dict_values(
                values.into_iter().map(|a| (*a).clone()).collect(),
            )))),
            _ => Ok(Some(Arc::clone(&values[0]))),
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

    /// Resolve the render view box (MediaBox ∩ CropBox) for this page.
    ///
    /// Public so a `Device` backend adapter (e.g. `pdf-x-device-tinyskia`) can size
    /// its raster target without core knowing about any specific backend.
    pub fn resolve_view_box_for_rendering<R: PageResolver>(&self, resolver: &mut R) -> [f64; 4] {
        let media_box = self
            .get_inheritable_property(resolver, "MediaBox", false)
            .ok()
            .and_then(|value| value.as_deref().and_then(Self::resolve_rect))
            .unwrap_or([0.0, 0.0, 612.0, 792.0]);

        let crop_box = self
            .get_inheritable_property(resolver, "CropBox", false)
            .ok()
            .and_then(|value| value.as_deref().and_then(Self::resolve_rect))
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

    /// Resolve the normalized `/Rotate` for this page. Public for backend adapters.
    pub fn resolve_rotate_for_rendering<R: PageResolver>(&self, resolver: &mut R) -> i32 {
        let mut rotate = self
            .get_inheritable_property(resolver, "Rotate", false)
            .ok()
            .and_then(|value| {
                value.as_deref().and_then(|obj| match obj {
                    PDFObject::Number(n) => Some(*n as i32),
                    _ => None,
                })
            })
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

    pub(crate) fn get_inheritable_resources<R: PageResolver>(
        &self,
        resolver: &mut R,
    ) -> PDFResult<Option<Arc<PDFObject>>> {
        self.get_inheritable_property(resolver, "Resources", false)
    }

    fn merge_stream_resources<R: PageResolver>(
        &self,
        resolver: &mut R,
        stream_resources: Option<&PDFObject>,
        page_resources: &Option<Arc<PDFObject>>,
    ) -> PDFResult<Option<PDFObject>> {
        let local_resources = match stream_resources {
            Some(value) => match &*resolver.resolve_if_ref(value)? {
                PDFObject::Dictionary(dict) if !dict.is_empty() => {
                    Some(PDFObject::Dictionary(dict.clone()))
                }
                _ => None,
            },
            None => None,
        };

        match (local_resources, page_resources) {
            (Some(local), Some(page)) => {
                let merged = Self::merge_dict_values(vec![local, (**page).clone()]);
                Ok(Some(merged))
            }
            (Some(local), None) => Ok(Some(local)),
            (None, Some(page)) => Ok(Some((**page).clone())),
            (None, None) => Ok(None),
        }
    }

    #[cfg(feature = "rendering")]
    fn load_fonts_for_rendering_with_resources<D: crate::rendering::Device, R: PageResolver>(
        &self,
        resolver: &mut R,
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

        let font_dict = match &*resolver.resolve_if_ref(font_entry)? {
            PDFObject::Dictionary(d) => d.clone(),
            _ => return Ok(()),
        };

        for (font_name, font_ref) in font_dict {
            if let Ok(font_obj) = resolver.resolve_if_ref(&font_ref) {
                if let Ok(pdf_font) = super::font::Font::new((*font_obj).clone(), resolver) {
                    if let Some(embedded_data) = pdf_font.embedded_font {
                        if let Err(e) = device.load_font_data(&font_name, embedded_data, None) {
                            if !e.to_string().contains("UnknownMagic") {
                                eprintln!(
                                    "Warning: Failed to load embedded font '{}': {}",
                                    font_name, e
                                );
                            }
                        }
                    } else if let Some(fallback_data) =
                        Self::get_fallback_font_data(pdf_font.base_font())
                    {
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
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// let pdf_data = std::fs::read("document.pdf").unwrap();
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    ///
    /// let text_items = page.extract_text(doc.doc_store_mut()).unwrap();
    /// for item in text_items {
    ///     println!("Text: '{}' at {:?}", item.text, item.position);
    /// }
    /// ```
    pub fn extract_text<R: PageResolver>(
        &self,
        resolver: &mut R,
    ) -> PDFResult<Vec<super::content_stream::TextItem>> {
        use super::{ContentStreamEvaluator, Lexer, Stream};

        let contents = match self.contents() {
            Some(contents) => contents,
            None => return Ok(Vec::new()), // No content streams
        };

        // Dereference if it's a reference — delta-aware (delta → base) via the resolver.
        let contents = resolver.resolve_if_ref(contents)?;

        let mut all_text_items = Vec::new();

        // Handle single content stream
        let content_streams = match &*contents {
            PDFObject::Stream { dict, data } => {
                vec![(dict.clone(), data.clone())]
            }
            PDFObject::Array(arr) => {
                // Multiple content streams - fetch each one
                let mut streams = Vec::new();
                for content_obj in arr.iter() {
                    match &*resolver.resolve_if_ref(content_obj)? {
                        PDFObject::Stream { dict, data } => {
                            streams.push((dict.clone(), data.clone()));
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

        // Concatenate decoded content streams BEFORE parsing. Real PDFs (e.g.
        // font_truetype_slow_post_lookup.pdf) split /Contents mid-operator/mid-BT
        // so each fragment is not a self-contained program. pdf.js / ResolvedPage
        // concatenate first; parsing streams independently aborts with
        // "ended with operands but no operator".
        // Source: ResolvedPage::resolve_contents, pdf.js Page.getOperatorList.
        let mut concatenated = Vec::new();
        for (dict, data) in content_streams {
            let decoded_data = if let Some(filter) = dict.get("Filter") {
                match super::decode::apply_filters(&data, filter) {
                    Ok(decoded) => decoded,
                    Err(_) => continue, // Skip this stream if decoding fails
                }
            } else {
                data
            };
            if !concatenated.is_empty() && !concatenated.ends_with(b"\n") {
                concatenated.push(b'\n');
            }
            concatenated.extend(decoded_data);
        }

        if !concatenated.is_empty() {
            // Create a stream from the (decoded) content data
            let stream = Box::new(Stream::from_bytes(concatenated)) as Box<dyn super::BaseStream>;
            let lexer = Lexer::new(stream)?;
            let parser = super::Parser::new(lexer)?;
            let mut evaluator = ContentStreamEvaluator::new(parser);

            // Load fonts from page resources (for proper character encoding).
            // Use get_inheritable_resources so /Resources as a Ref is fetched and
            // inherited resources from parent Pages nodes are honored — the render
            // path does the same. The previous self.resources() returned a raw Ref
            // which load_fonts skipped (not a Dictionary), so NO fonts were loaded
            // for any page with /Resources as an indirect object.
            let page_resources = self.get_inheritable_resources(resolver)?;
            if let Some(resources) = page_resources {
                // Ignore font loading errors - text extraction will still work with fallback encoding
                let _ = evaluator.load_fonts(&resources, resolver.as_xref_mut());
            }

            // Extract text from the concatenated page content
            let text_items = evaluator.extract_text()?;
            all_text_items.extend(text_items);

            // Recurse into Form XObjects referenced by `Do` — text inside Form
            // XObjects must be extracted too (many PDFs put body text in Form
            // XObjects, e.g. TAMReview.pdf).
            let form_names = evaluator.take_form_xobject_names();
            if !form_names.is_empty() {
                let resources = self.get_inheritable_resources(resolver)?;
                for name in form_names {
                    if let Some(ref res) = resources {
                        if let Err(e) = self.extract_text_from_form_xobject(
                            &name,
                            res,
                            resolver,
                            &mut all_text_items,
                            0,
                        ) {
                            eprintln!(
                                "Warning: Form XObject '{}' text extraction failed: {:?}",
                                name, e
                            );
                        }
                    }
                }
            }
        }

        Ok(all_text_items)
    }

    /// Maximum recursion depth for nested Form XObjects (prevent infinite loops).
    const FORM_XOBJECT_MAX_DEPTH: u8 = 16;

    /// Recursively extract text from a Form XObject referenced by name in the
    /// given resources. Mirrors `RenderingContext::paint_form_xobject` but for
    /// text extraction: decodes the form stream, loads its fonts (from the form's
    /// own /Resources or the inherited page resources), and runs the evaluator.
    fn extract_text_from_form_xobject<R: PageResolver>(
        &self,
        xobject_name: &str,
        resources: &PDFObject,
        resolver: &mut R,
        out: &mut Vec<super::content_stream::TextItem>,
        depth: u8,
    ) -> PDFResult<()> {
        if depth >= Self::FORM_XOBJECT_MAX_DEPTH {
            return Ok(());
        }

        let resources_dict = match resources {
            PDFObject::Dictionary(d) => d,
            _ => return Ok(()),
        };

        // Find /XObject in resources and resolve if Ref.
        let xobject_obj = match resources_dict.get("XObject") {
            Some(o) => o,
            None => return Ok(()),
        };
        let xobject_dict = match &*resolver.resolve_if_ref(xobject_obj)? {
            PDFObject::Dictionary(d) => d.clone(),
            _ => return Ok(()),
        };

        // Find the named XObject.
        let xobject_ref = match xobject_dict.get(xobject_name) {
            Some(o) => o,
            None => return Ok(()),
        };
        let xobject = resolver.resolve_if_ref(xobject_ref)?;

        let (form_dict, form_data) = match &*xobject {
            PDFObject::Stream { dict, data } => (dict.clone(), data.clone()),
            _ => return Ok(()),
        };

        // Only Form XObjects contain text; Image XObjects don't.
        let is_form = form_dict
            .get("Subtype")
            .and_then(|s| match s {
                PDFObject::Name(n) => Some(n.as_str()),
                _ => None,
            })
            .map(|n| n == "Form")
            .unwrap_or(false);
        if !is_form {
            return Ok(());
        }

        // Decode the form stream.
        let decoded_data = if let Some(filter) = form_dict.get("Filter") {
            match super::decode::apply_filters(&form_data, filter) {
                Ok(d) => d,
                Err(_) => return Ok(()),
            }
        } else {
            form_data.clone()
        };

        let stream = Box::new(Stream::from_bytes(decoded_data)) as Box<dyn super::BaseStream>;
        let lexer = Lexer::new(stream)?;
        let parser = super::Parser::new(lexer)?;
        let mut evaluator = ContentStreamEvaluator::new(parser);

        // Load fonts: Form XObjects may define their own /Resources; otherwise
        // inherit the page's. Fetch if Ref.
        let form_resources = if let Some(res_obj) = form_dict.get("Resources") {
            resolver.resolve_if_ref(res_obj).ok()
        } else {
            Some(Arc::new(resources.clone()))
        };
        if let Some(ref form_res) = form_resources {
            let _ = evaluator.load_fonts(form_res, resolver.as_xref_mut());
        }

        // Extract text from the form's content stream.
        let text_items = evaluator.extract_text()?;
        out.extend(text_items);

        // Recurse into nested Form XObjects.
        let nested_names = evaluator.take_form_xobject_names();
        for name in nested_names {
            if let Some(ref form_res) = form_resources {
                let _ =
                    self.extract_text_from_form_xobject(&name, form_res, resolver, out, depth + 1);
            }
        }

        Ok(())
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
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// let mut doc = pdf_x_source_file::open_file("document.pdf", None, None).unwrap();
    /// let page = doc.get_page(0).unwrap();
    ///
    /// let text = page.extract_text_as_string(doc.doc_store_mut()).unwrap();
    /// println!("Page text:\n{}", text);
    /// ```
    /// Same as `extract_text` but returns all text as a single string.
    ///
    /// This is the method called from the CLI.
    pub fn extract_text_as_string<R: PageResolver>(&self, resolver: &mut R) -> PDFResult<String> {
        let text_items = self.extract_text(resolver)?;

        // Determine line threshold: use the largest font size * 0.5, min 2.0.
        let line_threshold = text_items
            .iter()
            .filter_map(|t| t.font_size)
            .fold(2.0_f64, |m, fs| m.max(fs * 0.5));

        Ok(Self::decompose_ligatures(&Self::cluster_and_join(
            text_items,
            line_threshold,
        )))
    }

    /// Replace Unicode ligature codepoints with their ASCII equivalents.
    /// PDF fonts often encode ﬁ, ﬂ, ﬀ, etc. as single codepoints; pdftotext
    /// and pdf.js decompose them for plain-text output.
    fn decompose_ligatures(s: &str) -> String {
        s.replace('\u{FB00}', "ff")
            .replace('\u{FB01}', "fi")
            .replace('\u{FB02}', "fl")
            .replace('\u{FB03}', "ffi")
            .replace('\u{FB04}', "ffl")
            .replace('\u{FB05}', "st")
            .replace('\u{FB06}', "st")
    }

    /// Merge hyphenated line breaks: "word-\nword" → "wordword".
    /// Only when `-` is preceded by a letter and `\n` is followed by a
    /// lowercase letter (the common hyphenation pattern in English text).
    fn dehyphenate(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        let mut result = String::with_capacity(s.len());
        let mut i = 0;
        while i < chars.len() {
            if i + 2 < chars.len()
                && chars[i] == '-'
                && chars[i + 1] == '\n'
                && chars[i + 2].is_ascii_lowercase()
                && i > 0
                && chars[i - 1].is_alphabetic()
            {
                i += 2;
                continue;
            }
            result.push(chars[i]);
            i += 1;
        }
        result
    }

    /// Join pre-clustered lines in per-char mode (running average gap detection).
    fn join_per_char(lines: &[Vec<&TextItem>], line_threshold: f64) -> String {
        let mut result = String::new();
        let mut last_y: Option<f64> = None;
        let mut last_x: Option<f64> = None;
        let mut avg_gap: f64 = 0.0;
        let mut has_avg: bool = false;

        for line in lines {
            if !result.is_empty() {
                result.push('\n');
            }
            for item in line {
                if let Some(pos) = item.position {
                    if let Some(prev_y) = last_y {
                        if (pos.1 - prev_y).abs() > line_threshold {
                            result.push('\n');
                            avg_gap = 0.0;
                            has_avg = false;
                        } else if let Some(prev_x) = last_x {
                            let gap = pos.0 - prev_x;
                            if has_avg {
                                if gap > avg_gap * 1.35
                                    && !result.is_empty()
                                    && !result.ends_with(' ')
                                {
                                    result.push(' ');
                                    has_avg = false;
                                } else {
                                    avg_gap = avg_gap * 0.6 + gap * 0.4;
                                }
                            } else if gap > 0.0 {
                                avg_gap = gap;
                                has_avg = true;
                            }
                        }
                    }
                    last_y = Some(pos.1);
                    last_x = Some(pos.0);
                }
                result.push_str(&item.text);
            }
        }
        result
    }

    /// Cluster text items by Y proximity, sort each cluster by X, then join.
    ///
    /// In multi-column layouts, items from different columns at the same Y end up
    /// in the same line. We detect large X-gaps within each line and insert newlines
    /// to separate columns, producing row-major output.
    fn cluster_and_join(items: Vec<TextItem>, line_threshold: f64) -> String {
        if items.is_empty() {
            return String::new();
        }

        // Y-cluster items into lines by Y proximity, then sort each line by X.
        let mut sorted: Vec<&TextItem> = items.iter().collect();
        sorted.sort_by(|a, b| match (a.position, b.position) {
            (Some((_, y1)), Some((_, y2))) => {
                y2.partial_cmp(&y1).unwrap_or(std::cmp::Ordering::Equal)
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });

        let mut lines: Vec<Vec<&TextItem>> = Vec::new();
        for item in sorted.into_iter() {
            let y = item.position.map(|p| p.1);
            let placed = lines.last_mut().and_then(|line| {
                line.first()
                    .and_then(|f| f.position.map(|p| p.1))
                    .and_then(|first_y| y.map(|iy| (iy - first_y).abs() <= line_threshold))
            });
            if placed == Some(true) {
                lines.last_mut().unwrap().push(item);
            } else {
                lines.push(vec![item]);
            }
        }
        for line in &mut lines {
            line.sort_by(|a, b| match (a.position, b.position) {
                (Some((x1, _)), Some((x2, _))) => {
                    x1.partial_cmp(&x2).unwrap_or(std::cmp::Ordering::Equal)
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });
        }

        // Compute a dynamic column-gap threshold from the median X-gap.
        let column_threshold = {
            let mut gaps: Vec<f64> = Vec::new();
            for line in &lines {
                for pair in line.windows(2) {
                    if let (Some((x1, _)), Some((x2, _))) = (pair[0].position, pair[1].position) {
                        let gap = x2 - x1;
                        if gap > 0.0 {
                            gaps.push(gap);
                        }
                    }
                }
            }
            if gaps.is_empty() {
                50.0
            } else {
                gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let mid = gaps.len() / 2;
                let med = if gaps.len() % 2 == 0 {
                    (gaps[mid - 1] + gaps[mid]) / 2.0
                } else {
                    gaps[mid]
                };
                (med * 3.0).max(80.0).min(150.0)
            }
        };

        // Multi-column detection: find the MODAL gap midpoint across all Y-lines.
        // For each line, only the LARGEST gap participates — the column gap is
        // always the widest intra-line gap.  This avoids false positives from
        // word-spacing gaps while not requiring an arbitrary X threshold that
        // would exclude left-column items starting near x=54.
        let mut midpoints: Vec<f64> = Vec::new();
        for line in &lines {
            let mut best_gap: Option<(f64, f64)> = None; // (gap, midpoint)
            for pair in line.windows(2) {
                if let (Some((x1, _)), Some((x2, _))) = (pair[0].position, pair[1].position) {
                    let gap = x2 - x1;
                    if gap >= 60.0 {
                        let mid = (x1 + x2) / 2.0;
                        if best_gap.is_none_or(|(g, _)| gap > g) {
                            best_gap = Some((gap, mid));
                        }
                    }
                }
            }
            if let Some((_, mid)) = best_gap {
                midpoints.push(mid);
            }
        }
        let col_boundary = if midpoints.len() >= 6 {
            midpoints.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            // Sliding 25‑pt window: find the densest region.
            let window = 25.0;
            let mut best_count = 0usize;
            let mut best_mid = 0.0;
            for (i, &start) in midpoints.iter().enumerate() {
                let limit = start + window;
                let count = midpoints[i..].iter().take_while(|&&m| m <= limit).count();
                if count > best_count {
                    best_count = count;
                    best_mid = start + window / 2.0;
                }
            }
            if best_count >= 3 { best_mid } else { f64::MAX }
        } else {
            f64::MAX
        };

        if col_boundary == f64::MAX {
            // Single column: standard join.
            let all_single_char = lines.iter().flatten().all(|t| t.text.chars().count() == 1);
            if all_single_char {
                return Self::dehyphenate(&Self::join_per_char(&lines, line_threshold));
            } else {
                let mut result = String::new();
                for line in &lines {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    for (i, item) in line.iter().enumerate() {
                        if i > 0 && !item.text.starts_with(' ') && !result.ends_with(' ') {
                            result.push(' ');
                        }
                        result.push_str(&item.text);
                    }
                }
                return Self::dehyphenate(&result);
            }
        }

        // Multi-column: partition each line's items by the global column boundary.
        // Items with x < col_boundary go left, items with x >= col_boundary go right.
        // This handles lines with any number of items on either side (e.g. multi-item
        // reference entries with hanging-indent continuation lines).
        let mut col_lines: [Vec<Vec<&TextItem>>; 2] = [Vec::new(), Vec::new()];

        for line in &lines {
            let mut left: Vec<&TextItem> = Vec::new();
            let mut right: Vec<&TextItem> = Vec::new();
            for item in line {
                let is_right = item.position.is_some_and(|p| p.0 >= col_boundary);
                if is_right {
                    right.push(item);
                } else {
                    left.push(item);
                }
            }
            if !left.is_empty() {
                col_lines[0].push(left);
            }
            if !right.is_empty() {
                col_lines[1].push(right);
            }
        }

        let mut col_groups: Vec<Vec<String>> = vec![Vec::new(), Vec::new()];
        for (ci, lines_for_col) in col_lines.iter().enumerate() {
            for items in lines_for_col {
                let mut s = String::new();
                for (i, t) in items.iter().enumerate() {
                    if i > 0 && !t.text.starts_with(' ') && !s.ends_with(' ') {
                        s.push(' ');
                    }
                    s.push_str(&t.text);
                }
                col_groups[ci].push(s);
            }
        }

        col_groups
            .into_iter()
            .map(|g| Self::dehyphenate(&g.join("\n")))
            .collect::<Vec<_>>()
            .join("\n")
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
    /// use pdf_x_core::rendering::Device;
    /// use pdf_x_device_tinyskia::SkiaDevice;
    /// use tiny_skia::Pixmap;
    ///
    /// let pdf_data = std::fs::read("document.pdf").unwrap();
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    ///
    /// // Create a pixmap for rendering
    /// let mut pixmap = Pixmap::new(800, 600).unwrap();
    /// let mut font_cache = Default::default();
    /// let mut device = SkiaDevice::new(pixmap.as_mut(), &mut font_cache);
    ///
    /// // Render the page
    /// page.render(doc.doc_store_mut(), &mut device).unwrap();
    ///
    /// // Save the result
    /// pixmap.save_png("page1.png").unwrap();
    /// ```
    pub fn render<D: crate::rendering::Device, R: PageResolver>(
        &self,
        resolver: &mut R,
        device: &mut D,
    ) -> PDFResult<()> {
        use super::{Lexer, Parser, Stream};
        use crate::rendering::RenderingContext;

        // Reference: pdf.js/src/core/document.js - Page.view (MediaBox/CropBox handling)
        let view_box = self.resolve_view_box_for_rendering(resolver);

        let contents = match self.contents() {
            Some(contents) => contents,
            None => return Ok(()), // No content streams to render
        };

        // Dereference if it's a reference — delta-aware (delta → base) via the resolver.
        let contents = resolver.resolve_if_ref(contents)?;

        // Handle single content stream or array of streams
        let content_streams = match &*contents {
            PDFObject::Stream { dict, data } => {
                vec![(dict.clone(), data.clone())]
            }
            PDFObject::Array(arr) => {
                // Multiple content streams - fetch each one
                let mut streams = Vec::new();
                let mut seen_streams = std::collections::HashSet::new();
                for content_obj in arr.iter() {
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

                    match &*resolver.resolve_if_ref(content_obj)? {
                        PDFObject::Stream { dict, data } => {
                            streams.push((dict.clone(), data.clone()));
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

        // Set initial CTM to map view_box [x0,y0,x1,y1] to device pixels [0,0,w,h].
        // This handles non-zero MediaBox origins (e.g. [20,20,180,180]).
        let [x0, y0, x1, y1] = view_box;
        let rotate = self.resolve_rotate_for_rendering(resolver);
        let page_transform = page_initial_transform(view_box, rotate);
        device.set_matrix(&page_transform);

        // Set initial clip to the view box to prevent rendering outside page bounds
        device.begin_path();
        device.move_to(x0, y0);
        device.line_to(x1, y0);
        device.line_to(x1, y1);
        device.line_to(x0, y1);
        device.close_path();
        device.clip_path(crate::rendering::graphics_state::FillRule::NonZero)?;

        // Resolve inheritable page resources once (fonts, XObjects, etc.)
        // Reference: pdf.js/src/core/document.js - #getInheritableProperty("Resources")
        let page_resources = self.get_inheritable_resources(resolver)?;

        // Concatenate decoded content streams, then parse once.
        // Some PDFs split /Contents mid-operator (font_truetype_slow_post_lookup);
        // parsing fragments independently loses trailing ops and can abort early.
        // Matches ResolvedPage::resolve_contents / pdf.js Page.getOperatorList.
        // Also preserves graphics state across stream boundaries (no per-stream
        // save/restore of the page CTM).
        let mut concatenated = Vec::new();
        for (stream_idx, (dict, data)) in content_streams.into_iter().enumerate() {
            let decoded_data = if let Some(filter) = dict.get("Filter") {
                match super::decode::apply_filters(&data, filter) {
                    Ok(decoded) => decoded,
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to decode content stream {}: {}",
                            stream_idx, e
                        );
                        continue;
                    }
                }
            } else {
                data.to_vec()
            };
            if !concatenated.is_empty() && !concatenated.ends_with(b"\n") {
                concatenated.push(b'\n');
            }
            concatenated.extend(decoded_data);
        }

        // Resolve inheritable page resources once (fonts, XObjects, etc.)
        // Reference: pdf.js/src/core/document.js - #getInheritableProperty("Resources")
        // Note: page_resources was already resolved above the old loop; keep using it.

        if !concatenated.is_empty() {
            eprintln!(
                "Info: Processing concatenated content ({} bytes)",
                concatenated.len()
            );

            let stream = Box::new(Stream::from_bytes(concatenated)) as Box<dyn super::BaseStream>;
            let lexer = Lexer::new(stream)?;
            let parser = Parser::new(lexer)?;
            let mut evaluator = super::content_stream::ContentStreamEvaluator::new(parser);

            // Page-level resources (stream-level Resources on individual content
            // streams are rare; ResolvedPage also uses page resources for the
            // concatenated program).
            // Arc<PDFObject> → owned PDFObject for the font-loader / XObject resource APIs.
            let resources_owned: Option<PDFObject> = page_resources.as_ref().map(|r| (**r).clone());

            #[cfg(feature = "rendering")]
            self.load_fonts_for_rendering_with_resources(
                resolver.as_xref_mut(),
                device,
                resources_owned.as_ref(),
            )?;

            let mut ctx = RenderingContext::new(device);
            ctx.current_state_mut().set_matrix(&page_transform);
            ctx.set_root_ctm(page_transform);

            if let Some(ref resources_obj) = resources_owned {
                ctx.set_xobject_resources(&mut *resolver, resources_obj);
            }

            let mut total_operations = 0;
            loop {
                match evaluator.read_operation() {
                    Ok(Some(op)) => {
                        total_operations += 1;
                        if let Err(e) = ctx.process_operation(&op) {
                            eprintln!("Warning: Failed to process operator {:?}: {}", op.op, e);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("Warning: Failed to read operation, stopping: {}", e);
                        break;
                    }
                }
            }
            eprintln!(
                "Info: Total {} operations processed for page {}",
                total_operations, self.page_index
            );
        }

        Ok(())
    }

    // ========== Font Loading Methods ==========

    /// Gets all fonts from the page's Resources dictionary.
    ///
    /// Returns a mapping of font names (like "F1", "F2") to font dictionaries.
    pub fn get_fonts<R: PageResolver>(
        &self,
        resolver: &mut R,
    ) -> PDFResult<Vec<(String, super::PDFObject)>> {
        use super::PDFObject;

        let resources = match self.resources() {
            Some(r) => resolver.resolve_if_ref(r)?,
            None => return Ok(Vec::new()),
        };

        let resources_dict = match &*resources {
            PDFObject::Dictionary(d) => d.clone(),
            _ => return Ok(Vec::new()),
        };

        let font_dict = match resources_dict.get("Font") {
            Some(f) => resolver.resolve_if_ref(f)?,
            None => return Ok(Vec::new()),
        };

        match &*font_dict {
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
    pub fn load_fonts_for_rendering<D: crate::rendering::Device, R: PageResolver>(
        &self,
        resolver: &mut R,
        device: &mut D,
    ) -> PDFResult<()> {
        let fonts = self.get_fonts(resolver)?;

        for (font_name, font_ref) in fonts {
            // Try to fetch and load the font
            match resolver.resolve_if_ref(&font_ref) {
                Ok(font_obj) => {
                    // Try to create a Font and extract embedded data
                    match super::font::Font::new((*font_obj).clone(), resolver) {
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
    /// Returns Foxit Base14 CFF programs (same as pdf.js / hayro) so unembedded
    /// standard fonts share outlines with those engines.
    #[cfg(feature = "rendering")]
    fn get_fallback_font_data(base_font: &str) -> Option<Vec<u8>> {
        let data = match base_font {
            // Helvetica family -> Foxit Sans
            "Helvetica" | "Arial" | "sans-serif" => {
                include_bytes!("../../assets/fonts/FoxitSans.pfb").as_slice()
            }
            "Helvetica-Bold" | "Arial-Bold" => {
                include_bytes!("../../assets/fonts/FoxitSansBold.pfb").as_slice()
            }
            "Helvetica-Oblique" | "Helvetica-Italic" | "Arial-Italic" => {
                include_bytes!("../../assets/fonts/FoxitSansItalic.pfb").as_slice()
            }
            "Helvetica-BoldOblique" | "Helvetica-BoldItalic" | "Arial-BoldItalic" => {
                include_bytes!("../../assets/fonts/FoxitSansBoldItalic.pfb").as_slice()
            }

            // Times family -> Foxit Serif
            "Times" | "Times-Roman" | "serif" => {
                include_bytes!("../../assets/fonts/FoxitSerif.pfb").as_slice()
            }
            "Times-Bold" => include_bytes!("../../assets/fonts/FoxitSerifBold.pfb").as_slice(),
            "Times-Italic" | "Times-Roman-Italic" => {
                include_bytes!("../../assets/fonts/FoxitSerifItalic.pfb").as_slice()
            }
            "Times-BoldItalic" | "Times-Roman-BoldItalic" => {
                include_bytes!("../../assets/fonts/FoxitSerifBoldItalic.pfb").as_slice()
            }

            // Courier family -> Foxit Fixed
            "Courier" | "CourierNew" | "monospace" => {
                include_bytes!("../../assets/fonts/FoxitFixed.pfb").as_slice()
            }
            "Courier-Bold" | "CourierNew-Bold" => {
                include_bytes!("../../assets/fonts/FoxitFixedBold.pfb").as_slice()
            }
            "Courier-Oblique" | "Courier-Italic" | "CourierNew-Italic" => {
                include_bytes!("../../assets/fonts/FoxitFixedItalic.pfb").as_slice()
            }
            "Courier-BoldOblique" | "Courier-BoldItalic" | "CourierNew-BoldItalic" => {
                include_bytes!("../../assets/fonts/FoxitFixedBoldItalic.pfb").as_slice()
            }

            "Symbol" => include_bytes!("../../assets/fonts/FoxitSymbol.pfb").as_slice(),
            "ZapfDingbats" => include_bytes!("../../assets/fonts/FoxitDingbats.pfb").as_slice(),

            _ => return None,
        };

        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: Loaded bundled fallback font for '{}' ({} bytes)",
            base_font,
            data.len()
        );
        Some(data.to_vec())
    }
}

pub(crate) fn page_initial_transform(view_box: [f64; 4], rotate: i32) -> [f64; 6] {
    let [x0, y0, x1, y1] = view_box;
    let center_x = (x0 + x1) * 0.5;
    let center_y = (y0 + y1) * 0.5;
    let (a, b, c, d) = match rotate.rem_euclid(360) {
        90 => (0.0, 1.0, 1.0, 0.0),
        180 => (-1.0, 0.0, 0.0, 1.0),
        270 => (0.0, -1.0, -1.0, 0.0),
        _ => (1.0, 0.0, 0.0, -1.0),
    };

    let (offset_x, offset_y) = if a == 0.0 {
        ((center_y - y0).abs(), (center_x - x0).abs())
    } else {
        ((center_x - x0).abs(), (center_y - y0).abs())
    };

    [
        a,
        b,
        c,
        d,
        offset_x - a * center_x - c * center_y,
        offset_y - b * center_x - d * center_y,
    ]
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

    /// Forward cache: page index → page ref (object number, generation).
    /// Populated lazily; once `page_refs_fully_built` the entire tree has
    /// been enumerated so no further traversal is needed.
    page_refs: Vec<Option<(u32, u32)>>,
    page_refs_fully_built: bool,
}

impl PageTreeCache {
    /// Creates a new empty page tree cache.
    pub fn new() -> Self {
        PageTreeCache {
            pages: FxHashMap::default(),
            page_refs: Vec::new(),
            page_refs_fully_built: false,
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

    /// Returns the cached page ref for an index, if known.
    pub fn get_page_ref(&self, page_index: usize) -> Option<(u32, u32)> {
        self.page_refs.get(page_index).copied().flatten()
    }

    /// Stores a page ref for a given index, ensuring the vec is large enough.
    pub fn put_page_ref(&mut self, page_index: usize, page_ref: Option<(u32, u32)>) {
        if page_index >= self.page_refs.len() {
            self.page_refs.resize(page_index + 1, None);
        }
        self.page_refs[page_index] = page_ref;
    }

    /// Whether the full page tree has been walked at least once.
    pub fn is_fully_built(&self) -> bool {
        self.page_refs_fully_built
    }

    /// Marks the full page tree as enumerated.
    pub fn set_fully_built(&mut self) {
        self.page_refs_fully_built = true;
    }

    /// Clears the cache.
    pub fn clear(&mut self) {
        self.pages.clear();
        self.page_refs.clear();
        self.page_refs_fully_built = false;
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
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let images = page.get_image_metadata(doc.doc_store_mut()).unwrap();
    ///
    /// for image in images {
    ///     println!("Image: {} ({}x{}, {})",
    ///              image.name, image.width, image.height, image.format);
    /// }
    /// ```
    pub fn get_image_metadata<R: PageResolver>(
        &self,
        resolver: &mut R,
    ) -> PDFResult<Vec<super::image::ImageMetadata>> {
        use super::image::{ImageDecoder, ImageFormat, ImageMetadata};

        let mut images = Vec::new();

        let resources = match self.resources() {
            Some(res) => resolver.resolve_if_ref(res)?,
            None => return Ok(images),
        };

        let resources_dict = match &*resources {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(images),
        };

        let xobject_entry = match resources_dict.get("XObject") {
            Some(entry) => entry,
            None => return Ok(images),
        };

        let xobject_obj = resolver.resolve_if_ref(xobject_entry)?;
        let xobject_dict = match &*xobject_obj {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(images),
        };

        for (name, xobject_ref) in xobject_dict {
            let xobject = resolver.resolve_if_ref(xobject_ref)?;

            // Check if it's an image (Subtype == /Image)
            if let PDFObject::Stream { dict, data } = &*xobject {
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
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let images = page.extract_images(doc.doc_store_mut()).unwrap();
    ///
    /// for (i, image) in images.iter().enumerate() {
    ///     println!("Image {}: {}x{} pixels, {} bytes",
    ///              i, image.width, image.height, image.data.len());
    ///     // Save or process image.data...
    /// }
    /// ```
    pub fn extract_images<R: PageResolver>(
        &self,
        resolver: &mut R,
    ) -> PDFResult<Vec<super::image::DecodedImage>> {
        use super::decode;
        use super::image::{ImageColorSpace, ImageDecoder};

        let mut decoded_images = Vec::new();

        let resources = match self.resources() {
            Some(res) => resolver.resolve_if_ref(res)?,
            None => return Ok(decoded_images),
        };

        let resources_dict = match &*resources {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(decoded_images),
        };

        let xobject_entry = match resources_dict.get("XObject") {
            Some(entry) => entry,
            None => return Ok(decoded_images),
        };

        let xobject_obj = resolver.resolve_if_ref(xobject_entry)?;
        let xobject_dict = match &*xobject_obj {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(decoded_images),
        };

        for (name, xobject_ref) in xobject_dict {
            let xobject = resolver.resolve_if_ref(xobject_ref)?;

            // Check if it's an image (Subtype == /Image)
            if let PDFObject::Stream { dict, data } = &*xobject {
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

    /// Helper method to get XObject data by name.
    /// This retrieves the actual image data from the page's XObject resources.
    pub fn get_xobject_data<R: PageResolver>(
        &self,
        name: &str,
        resolver: &mut R,
    ) -> PDFResult<Vec<u8>> {
        if let Some(resources) = self.resources() {
            if let PDFObject::Dictionary(xobject_dict) = resources {
                if let Some(xobject_entry) = xobject_dict.get("XObject") {
                    let xobject_obj = resolver.resolve_if_ref(xobject_entry)?;
                    let xobject_dict = match &*xobject_obj {
                        PDFObject::Dictionary(dict) => dict,
                        _ => {
                            return Err(PDFError::Generic(
                                "XObject is not a dictionary".to_string(),
                            ));
                        }
                    };

                    for (name_key, xobject_ref) in xobject_dict {
                        if name_key == name {
                            let xobject = resolver.resolve_if_ref(xobject_ref)?;

                            if let PDFObject::Stream { dict: _, data } = &*xobject {
                                return Ok(data.clone());
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
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let annotations = page.extract_annotations(doc.doc_store_mut()).unwrap();
    ///
    /// for annot in annotations {
    ///     println!("Annotation type: {:?}", annot.annotation_type);
    ///     if let Some(ref contents) = annot.contents {
    ///         println!("  Contents: {}", contents);
    ///     }
    /// }
    /// ```
    pub fn extract_annotations<R: PageResolver>(
        &self,
        resolver: &mut R,
    ) -> PDFResult<Vec<super::annotation::Annotation>> {
        use super::annotation::parse_annotations;

        let annots = match self.annotations() {
            Some(a) => a,
            None => return Ok(Vec::new()),
        };

        parse_annotations(annots, resolver)
    }
}
