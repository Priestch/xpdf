use super::error::{PDFResult, PDFError};
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
    pub fn extract_text(&self, xref: &mut super::xref::XRef) -> PDFResult<Vec<super::content_stream::TextItem>> {
        use super::{ContentStreamEvaluator, Lexer, Stream};
        use super::decode::decode_flate;

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
                    _ => data // Other filters not yet supported, use raw data
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
    pub fn get_image_metadata(&self, xref: &mut super::xref::XRef) -> PDFResult<Vec<super::image::ImageMetadata>> {
        use super::image::{ImageMetadata, ImageFormat, ImageDecoder};

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
                let is_image = dict.get("Subtype")
                    .and_then(|subtype| match subtype {
                        PDFObject::Name(n) => Some(n == "Image"),
                        _ => None,
                    })
                    .unwrap_or(false);

                if !is_image {
                    continue; // Not an image XObject (could be Form)
                }

                // Extract metadata
                let width = dict.get("Width")
                    .or_else(|| dict.get("W"))
                    .and_then(|w| match w {
                        PDFObject::Number(n) => Some(*n as u32),
                        _ => None,
                    })
                    .unwrap_or(0);

                let height = dict.get("Height")
                    .or_else(|| dict.get("H"))
                    .and_then(|h| match h {
                        PDFObject::Number(n) => Some(*n as u32),
                        _ => None,
                    })
                    .unwrap_or(0);

                let bits_per_component = dict.get("BitsPerComponent")
                    .or_else(|| dict.get("BPC"))
                    .and_then(|bpc| match bpc {
                        PDFObject::Number(n) => Some(*n as u8),
                        _ => None,
                    })
                    .unwrap_or(8);

                // Get color space
                let color_space = dict.get("ColorSpace")
                    .or_else(|| dict.get("CS"))
                    .map(|cs| match cs {
                        PDFObject::Name(name) => name.clone(),
                        PDFObject::Array(arr) => {
                            arr.get(0)
                                .and_then(|obj| match &**obj {
                                    PDFObject::Name(n) => Some(n.clone()),
                                    _ => None,
                                })
                                .unwrap_or_else(|| "Unknown".to_string())
                        }
                        _ => "Unknown".to_string(),
                    })
                    .unwrap_or_else(|| "Unknown".to_string());

                // Detect image format from filter
                let format = dict.get("Filter")
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
    pub fn extract_images(&self, xref: &mut super::xref::XRef) -> PDFResult<Vec<super::image::DecodedImage>> {
        use super::image::{ImageDecoder, ImageColorSpace};
        use super::decode;

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
                let is_image = dict.get("Subtype")
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
                                match ImageDecoder::decode_image(&data, super::image::ImageFormat::JPEG) {
                                    Ok(img) => decoded_images.push(img),
                                    Err(e) => {
                                        eprintln!("Warning: Failed to decode JPEG image '{}': {}", name, e);
                                    }
                                }
                            }
                            "JPXDecode" => {
                                // JPEG2000 - decode directly
                                match ImageDecoder::decode_image(&data, super::image::ImageFormat::JPEG2000) {
                                    Ok(img) => decoded_images.push(img),
                                    Err(e) => {
                                        eprintln!("Warning: Failed to decode JPEG2000 image '{}': {}", name, e);
                                    }
                                }
                            }
                            "JBIG2Decode" => {
                                // JBIG2 - decode directly
                                match ImageDecoder::decode_image(&data, super::image::ImageFormat::JBIG2) {
                                    Ok(img) => decoded_images.push(img),
                                    Err(e) => {
                                        eprintln!("Warning: Failed to decode JBIG2 image '{}': {}", name, e);
                                    }
                                }
                            }
                            "FlateDecode" => {
                                // Decompress first, then decode as raw image
                                match decode::decode_flate(&data) {
                                    Ok(decompressed) => {
                                        // Get image parameters for raw decoding
                                        let width = dict.get("Width").or_else(|| dict.get("W"))
                                            .and_then(|w| match w {
                                                PDFObject::Number(n) => Some(*n as u32),
                                                _ => None,
                                            })
                                            .unwrap_or(0);

                                        let height = dict.get("Height").or_else(|| dict.get("H"))
                                            .and_then(|h| match h {
                                                PDFObject::Number(n) => Some(*n as u32),
                                                _ => None,
                                            })
                                            .unwrap_or(0);

                                        let bpc = dict.get("BitsPerComponent").or_else(|| dict.get("BPC"))
                                            .and_then(|bpc| match bpc {
                                                PDFObject::Number(n) => Some(*n as u8),
                                                _ => None,
                                            })
                                            .unwrap_or(8);

                                        let color_space = dict.get("ColorSpace").or_else(|| dict.get("CS"))
                                            .map(|cs| ImageDecoder::parse_color_space(cs))
                                            .unwrap_or(ImageColorSpace::RGB);

                                        // Decode as raw image
                                        match ImageDecoder::decode_raw_image(&decompressed, width, height, bpc, color_space) {
                                            Ok(img) => decoded_images.push(img),
                                            Err(e) => {
                                                eprintln!("Warning: Failed to decode raw image '{}': {}", name, e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Warning: Failed to decompress FlateDecode image '{}': {}", name, e);
                                    }
                                }
                            }
                            other => {
                                eprintln!("Warning: Unsupported image filter '{}' for image '{}'", other, name);
                            }
                        }
                    }
                    Some(PDFObject::Array(_arr)) => {
                        eprintln!("Warning: Multiple filters not yet supported for image '{}'", name);
                    }
                    None => {
                        // No filter - raw uncompressed image data
                        let width = dict.get("Width").or_else(|| dict.get("W"))
                            .and_then(|w| match w {
                                PDFObject::Number(n) => Some(*n as u32),
                                _ => None,
                            })
                            .unwrap_or(0);

                        let height = dict.get("Height").or_else(|| dict.get("H"))
                            .and_then(|h| match h {
                                PDFObject::Number(n) => Some(*n as u32),
                                _ => None,
                            })
                            .unwrap_or(0);

                        let bpc = dict.get("BitsPerComponent").or_else(|| dict.get("BPC"))
                            .and_then(|bpc| match bpc {
                                PDFObject::Number(n) => Some(*n as u8),
                                _ => None,
                            })
                            .unwrap_or(8);

                        let color_space = dict.get("ColorSpace").or_else(|| dict.get("CS"))
                            .map(|cs| ImageDecoder::parse_color_space(cs))
                            .unwrap_or(ImageColorSpace::RGB);

                        // Decode as raw image
                        match ImageDecoder::decode_raw_image(&data, width, height, bpc, color_space) {
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
    pub fn fetch_if_ref(&self, obj: &PDFObject, xref: &mut super::xref::XRef) -> PDFResult<PDFObject> {
        match obj {
            PDFObject::Ref { num, generation } => {
                match xref.fetch(*num, *generation)? {
                    rc_obj => Ok((*rc_obj).clone()),
                }
            }
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
                        _ => return Err(PDFError::Generic(
                            "XObject is not a dictionary".to_string()
                        )),
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
                                return Err(PDFError::Generic(
                                    format!("XObject '{}' is not a stream", name)
                                ));
                            }
                        }
                    }
                    return Err(PDFError::Generic(
                        format!("XObject '{}' not found in page resources", name)
                    ));
                } else {
                    return Err(PDFError::Generic(
                        "No XObject found in page resources".to_string()
                    ));
                }
            } else {
                return Err(PDFError::Generic(
                    "Resources is not a dictionary".to_string()
                ));
            }
        } else {
            Err(PDFError::Generic(
                "No resources dictionary found in page".to_string()
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
    pub fn extract_annotations(&self, xref: &mut super::xref::XRef) -> PDFResult<Vec<super::annotation::Annotation>> {
        use super::annotation::parse_annotations;

        let annots = match self.annotations() {
            Some(a) => a,
            None => return Ok(Vec::new()),
        };

        parse_annotations(annots, xref)
    }
}
