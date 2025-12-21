use super::error::{PDFResult, PDFError};
use super::parser::PDFObject;
use std::collections::HashMap;

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

        let contents = match self.contents() {
            Some(contents) => contents,
            None => return Ok(Vec::new()), // No content streams
        };

        let mut all_text_items = Vec::new();

        // Handle single content stream
        let content_streams = match contents {
            PDFObject::Stream { dict, data } => {
                vec![(dict.clone(), data.clone())]
            }
            PDFObject::Array(arr) => {
                // Multiple content streams - fetch each one
                let mut streams = Vec::new();
                for content_obj in arr {
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
        for (_dict, data) in content_streams {
            // Create a stream from the content data
            let stream = Box::new(Stream::from_bytes(data)) as Box<dyn super::BaseStream>;
            let lexer = Lexer::new(stream)?;
            let parser = super::Parser::new(lexer)?;
            let mut evaluator = ContentStreamEvaluator::new(parser);

            // Extract text from this stream
            let text_items = evaluator.extract_text()?;
            all_text_items.extend(text_items);
        }

        Ok(all_text_items)
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
    pages: HashMap<usize, Page>,
}

impl PageTreeCache {
    /// Creates a new empty page tree cache.
    pub fn new() -> Self {
        PageTreeCache {
            pages: HashMap::new(),
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

impl crate::core::image::ImageExtraction for Page {
    /// Extract image metadata from the page without full decoding.
    fn get_image_metadata(&self) -> PDFResult<Vec<crate::core::image::ImageMetadata>> {
        let mut images = Vec::new();

        // Get the Resources dictionary from the page
        if let Some(resources) = self.resources() {
            // Look for XObject dictionary
            if let PDFObject::Dictionary(xobject_dict) = resources {
                // Check if there's an XObject entry
                if let Some(xobject_entry) = xobject_dict.get("XObject") {
                    // Follow reference if needed
                    let xobject_dict = match xobject_entry {
                        PDFObject::Ref { .. } => {
                            // For now, we can't resolve references without document context
                            return Err(PDFError::Generic(
                                "Resolving XObject references requires document context".to_string()
                            ));
                        }
                        PDFObject::Dictionary(dict) => dict,
                        _ => return Ok(images), // No XObject dictionary found
                    };

                    for (name_key, _xobject_ref) in xobject_dict {
                        // name_key is a String key, not a PDFObject
                        println!("  🖼️  Found XObject: {} (reference resolution needs document context)", name_key);

                        // Create placeholder metadata - in real implementation would resolve the reference
                        let metadata = crate::core::image::ImageMetadata::new(
                            name_key.clone(),
                            crate::core::image::ImageFormat::Unknown
                        );
                        images.push(metadata);
                    }
                }
            }
        }

        Ok(images)
    }

    /// Extract complete images with full decoding.
    fn extract_images(&self) -> PDFResult<Vec<crate::core::image::DecodedImage>> {
        // For now, return empty list since we need xref access to resolve references
        // In a full implementation, this would require document context
        println!("  ⚠️  Full image extraction requires document context to resolve XObject references");
        Ok(Vec::new())
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
}
