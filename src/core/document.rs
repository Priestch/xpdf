use super::base_stream::BaseStream;
use super::error::{PDFError, PDFResult};
use super::page::{Page, PageTreeCache};
use super::parser::PDFObject;
use super::stream::Stream;
use super::xref::XRef;
use std::collections::HashSet;

/// Information about a linearized PDF.
#[derive(Debug, Clone)]
pub struct LinearizedInfo {
    /// File size as specified in the linearization dictionary
    pub file_size: u64,

    /// The primary hint table offset (points to hint table location)
    pub primary_hint_offset: u64,

    /// The primary hint table length
    pub primary_hint_length: u64,

    /// The offset of the first page's object
    pub first_page_offset: u64,

    /// The number of pages
    pub page_count: u32,

    /// The object number of the first page
    pub first_page_obj_num: u32,
}

/// PDF Document reader.
///
/// This is the main entry point for reading and parsing PDF documents.
/// It handles opening a PDF file, parsing its structure, and providing
/// access to document-level information like the catalog and pages.
///
/// Based on PDF.js's PDFDocument class.
pub struct PDFDocument {
    /// The cross-reference table
    xref: XRef,

    /// The document catalog (root dictionary)
    catalog: Option<PDFObject>,

    /// Page tree cache for efficient page lookups
    page_cache: PageTreeCache,

    /// Linearized PDF information (if applicable)
    linearized: Option<LinearizedInfo>,
}

impl PDFDocument {
    /// Opens a PDF document from a byte array.
    ///
    /// This parses the PDF structure including the xref table and trailer,
    /// and loads the document catalog.
    ///
    /// # Arguments
    /// * `data` - The complete PDF file as bytes
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// let pdf_data = std::fs::read("document.pdf").unwrap();
    /// let doc = PDFDocument::open(pdf_data).unwrap();
    /// ```
    pub fn open(data: Vec<u8>) -> PDFResult<Self> {
        // Find the startxref offset
        let startxref = Self::find_startxref(&data)?;

        // Create stream and xref
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut xref = XRef::new(stream);

        // Position at xref table and parse
        xref.set_stream_pos(startxref)?;
        xref.parse()?;

        // Load the catalog
        let catalog = Some(xref.catalog()?);

        // Check if this is a linearized PDF
        let linearized = Self::check_linearized(&mut xref)?;

        Ok(PDFDocument {
            xref,
            catalog,
            page_cache: PageTreeCache::new(),
            linearized,
        })
    }

    /// Finds the byte offset of the cross-reference table.
    ///
    /// This searches for "startxref" near the end of the file and reads
    /// the offset that follows it.
    ///
    /// Format:
    /// ```text
    /// ...
    /// startxref
    /// 12345
    /// %%EOF
    /// ```
    fn find_startxref(data: &[u8]) -> PDFResult<usize> {
        // Search from the end of the file (last 1024 bytes)
        let search_start = if data.len() > 1024 {
            data.len() - 1024
        } else {
            0
        };

        let search_data = &data[search_start..];

        // Find "startxref"
        let keyword = b"startxref";
        let pos = search_data
            .windows(keyword.len())
            .rposition(|window| window == keyword)
            .ok_or_else(|| PDFError::Generic("startxref not found in PDF".to_string()))?;

        // Skip past "startxref" and any whitespace
        let mut offset_start = search_start + pos + keyword.len();

        // Skip whitespace
        while offset_start < data.len() && data[offset_start].is_ascii_whitespace() {
            offset_start += 1;
        }

        // Read the offset number (as ASCII digits)
        let mut offset_end = offset_start;
        while offset_end < data.len() && data[offset_end].is_ascii_digit() {
            offset_end += 1;
        }

        if offset_start == offset_end {
            return Err(PDFError::Generic(
                "No offset found after startxref".to_string(),
            ));
        }

        // Parse the offset
        let offset_str = std::str::from_utf8(&data[offset_start..offset_end])
            .map_err(|_| PDFError::Generic("Invalid UTF-8 in startxref offset".to_string()))?;

        let offset: usize = offset_str
            .parse()
            .map_err(|_| PDFError::Generic("Invalid startxref offset".to_string()))?;

        Ok(offset)
    }

    /// Returns the document catalog (root dictionary).
    pub fn catalog(&self) -> Option<&PDFObject> {
        self.catalog.as_ref()
    }

    /// Returns a mutable reference to the xref table for fetching objects.
    pub fn xref_mut(&mut self) -> &mut XRef {
        &mut self.xref
    }

    /// Returns an immutable reference to the xref table.
    pub fn xref(&self) -> &XRef {
        &self.xref
    }

    /// Gets the /Pages dictionary from the catalog.
    pub fn pages_dict(&mut self) -> PDFResult<PDFObject> {
        let catalog = self
            .catalog
            .as_ref()
            .ok_or_else(|| PDFError::Generic("No catalog".to_string()))?;

        let catalog_dict = match catalog {
            PDFObject::Dictionary(dict) => dict,
            _ => {
                return Err(PDFError::Generic(
                    "Catalog is not a dictionary".to_string(),
                ))
            }
        };

        let pages_ref = catalog_dict
            .get("Pages")
            .ok_or_else(|| PDFError::Generic("No /Pages in catalog".to_string()))?;

        self.xref.fetch_if_ref(pages_ref)
    }

    /// Gets the page count from the /Pages dictionary.
    pub fn page_count(&mut self) -> PDFResult<u32> {
        let pages_dict = self.pages_dict()?;

        let pages_dict = match pages_dict {
            PDFObject::Dictionary(dict) => dict,
            _ => {
                return Err(PDFError::Generic(
                    "/Pages is not a dictionary".to_string(),
                ))
            }
        };

        let count = pages_dict
            .get("Count")
            .ok_or_else(|| PDFError::Generic("No /Count in /Pages".to_string()))?;

        match count {
            PDFObject::Number(n) => Ok(*n as u32),
            _ => Err(PDFError::Generic("/Count is not a number".to_string())),
        }
    }

    /// Traverses the page tree to find a specific page by index.
    ///
    /// PDF page trees can be hierarchical with intermediate "Pages" nodes
    /// containing "Kids" arrays. This method implements depth-first traversal
    /// similar to PDF.js's getPageDict method.
    ///
    /// # Arguments
    /// * `page_index` - The 0-based page index to retrieve
    ///
    /// # Returns
    /// Returns (page_dict, page_ref) where page_ref is the indirect reference if available
    fn get_page_dict(&mut self, page_index: usize) -> PDFResult<(PDFObject, Option<(u32, u32)>)> {
        // Get the root Pages dictionary
        let root_pages = self.pages_dict()?;

        // Stack for depth-first traversal: (node, is_reference)
        let mut nodes_to_visit: Vec<(PDFObject, Option<(u32, u32)>)> = vec![(root_pages, None)];
        let mut visited_refs: HashSet<(u32, u32)> = HashSet::new();
        let mut current_page_index = 0;

        while let Some((current_node, node_ref)) = nodes_to_visit.pop() {
            // Handle references
            let (node_obj, obj_ref) = match &current_node {
                PDFObject::Ref { num, generation } => {
                    let ref_key = (*num, *generation);

                    // Prevent circular references
                    if visited_refs.contains(&ref_key) {
                        return Err(PDFError::Generic(
                            "Circular reference in page tree".to_string(),
                        ));
                    }
                    visited_refs.insert(ref_key);

                    // Fetch the object
                    let fetched = self.xref.fetch(*num, *generation)?;
                    let obj = (*fetched).clone();
                    (obj, Some(ref_key))
                }
                _ => (current_node.clone(), node_ref),
            };

            // Check if this is a dictionary
            let dict = match &node_obj {
                PDFObject::Dictionary(d) => d,
                _ => continue,
            };

            // Check the Type field
            let type_obj = dict.get("Type");
            let is_page = match type_obj {
                Some(PDFObject::Name(name)) => name == "Page",
                _ => !dict.contains_key("Kids"), // If no Type, check for Kids (leaf node)
            };

            if is_page {
                // This is a page node
                if current_page_index == page_index {
                    return Ok((node_obj, obj_ref));
                }
                current_page_index += 1;
                continue;
            }

            // This is an intermediate Pages node - traverse its Kids
            let kids = dict.get("Kids").ok_or_else(|| {
                PDFError::Generic("Pages node missing Kids array".to_string())
            })?;

            // Get the kids array (either directly or by resolving a reference)
            let kids_array = match kids {
                PDFObject::Array(arr) => arr.clone(),
                PDFObject::Ref { num, generation } => {
                    // Kids is a reference, fetch it
                    let fetched = self.xref.fetch(*num, *generation)?;
                    match &*fetched {
                        PDFObject::Array(arr) => arr.clone(),
                        _ => {
                            return Err(PDFError::Generic(
                                "Kids reference doesn't point to array".to_string(),
                            ))
                        }
                    }
                }
                _ => {
                    return Err(PDFError::Generic(
                        "Kids is not an array or reference".to_string(),
                    ))
                }
            };

            // Add kids to stack in reverse order (to maintain order during DFS)
            for kid in kids_array.iter().rev() {
                nodes_to_visit.push((kid.clone(), None));
            }
        }

        Err(PDFError::Generic(format!(
            "Page index {} not found in page tree",
            page_index
        )))
    }

    /// Gets a specific page by index (0-based).
    ///
    /// Pages are loaded lazily and cached. The first call for a page will
    /// traverse the page tree; subsequent calls return the cached page.
    ///
    /// # Arguments
    /// * `page_index` - The 0-based page index (0 = first page)
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// let pdf_data = std::fs::read("document.pdf").unwrap();
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    ///
    /// // Get the first page
    /// let page = doc.get_page(0).unwrap();
    /// println!("Page index: {}", page.index());
    /// ```
    pub fn get_page(&mut self, page_index: usize) -> PDFResult<Page> {
        // Check cache first
        if let Some(cached_page) = self.page_cache.get(page_index) {
            return Ok(cached_page.clone());
        }

        // Traverse the page tree to find the page
        let (page_dict, page_ref) = self.get_page_dict(page_index)?;

        // Create the Page object
        let page = Page::new(page_index, page_dict, page_ref);

        // Cache it
        self.page_cache.put(page_index, page.clone());

        Ok(page)
    }

    /// Gets an inheritable property from a page dictionary.
    ///
    /// PDF pages can inherit certain properties from parent Pages nodes in the
    /// page tree. This method walks up the tree following "Parent" references
    /// until it finds the property.
    ///
    /// Inheritable properties include:
    /// - Resources: Fonts, images, and other resources
    /// - MediaBox: Page dimensions
    /// - CropBox: Visible page area
    /// - Rotate: Page rotation
    ///
    /// # Arguments
    /// * `page` - The page to start searching from
    /// * `key` - The property name (e.g., "MediaBox", "Resources")
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// # let pdf_data = vec![];  // Placeholder for example
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    ///
    /// // Get MediaBox (might be inherited from parent)
    /// let media_box = doc.get_inheritable_property(&page, "MediaBox").ok();
    /// ```
    pub fn get_inheritable_property(&mut self, page: &Page, key: &str) -> PDFResult<PDFObject> {
        let mut current_dict = page.dict().clone();
        let mut visited_refs: HashSet<(u32, u32)> = HashSet::new();

        loop {
            // Get the dictionary
            let dict = match &current_dict {
                PDFObject::Dictionary(d) => d,
                _ => return Err(PDFError::Generic("Not a dictionary".to_string())),
            };

            // Check if this dictionary has the property
            if let Some(value) = dict.get(key) {
                // Found it! Resolve if it's a reference
                return self.xref.fetch_if_ref(value);
            }

            // Not found, try parent
            let parent = dict.get("Parent").ok_or_else(|| {
                PDFError::Generic(format!("Property '{}' not found in page tree", key))
            })?;

            // Resolve parent if it's a reference
            match parent {
                PDFObject::Ref { num, generation } => {
                    let ref_key = (*num, *generation);

                    // Prevent circular references
                    if visited_refs.contains(&ref_key) {
                        return Err(PDFError::Generic(
                            "Circular reference in page tree".to_string(),
                        ));
                    }
                    visited_refs.insert(ref_key);

                    // Fetch the parent dictionary
                    let parent_obj = self.xref.fetch(*num, *generation)?;
                    current_dict = (*parent_obj).clone();
                }
                _ => {
                    // Parent is not a reference, use it directly
                    current_dict = parent.clone();
                }
            }
        }
    }

    /// Gets the MediaBox for a page, using inheritance if needed.
    ///
    /// MediaBox defines the boundaries of the physical medium on which the page
    /// is to be printed. It's an array of 4 numbers: [llx, lly, urx, ury]
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// # let pdf_data = vec![];  // Placeholder for example
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let media_box = doc.get_media_box(&page).unwrap();
    /// ```
    pub fn get_media_box(&mut self, page: &Page) -> PDFResult<PDFObject> {
        self.get_inheritable_property(page, "MediaBox")
    }

    /// Gets the Resources dictionary for a page, using inheritance if needed.
    ///
    /// Resources contains fonts, images, and other resources used by the page.
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x::core::PDFDocument;
    ///
    /// # let pdf_data = vec![];  // Placeholder for example
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let resources = doc.get_resources(&page).ok();
    /// ```
    pub fn get_resources(&mut self, page: &Page) -> PDFResult<PDFObject> {
        self.get_inheritable_property(page, "Resources")
    }

    /// Gets the CropBox for a page, using inheritance if needed.
    ///
    /// CropBox defines the region to which the contents of the page should be
    /// clipped when displayed or printed.
    pub fn get_crop_box(&mut self, page: &Page) -> PDFResult<PDFObject> {
        self.get_inheritable_property(page, "CropBox")
    }

    /// Gets the Rotate value for a page, using inheritance if needed.
    ///
    /// Rotate specifies the number of degrees by which the page should be
    /// rotated clockwise when displayed or printed. Must be a multiple of 90.
    pub fn get_rotate(&mut self, page: &Page) -> PDFResult<PDFObject> {
        self.get_inheritable_property(page, "Rotate")
    }

    /// Checks if this PDF is linearized (optimized for web view).
    ///
    /// Linearized PDFs (also known as "optimized for web" or "fast web view")
    /// allow the first page to be displayed before the entire file is downloaded.
    ///
    /// # Returns
    /// `Some(LinearizedInfo)` if the PDF is linearized, `None` otherwise
    ///
    /// Based on PDF.js's checkLinearization method
    fn check_linearized(xref: &mut XRef) -> PDFResult<Option<LinearizedInfo>> {
        // Linearized PDFs have an object at the beginning of the file
        // with /Linearized in the dictionary. Usually object 1.

        // Try to get object 1 (common position for linearization dict)
        let obj1_result = xref.fetch(1, 0);

        let obj1 = match obj1_result {
            Ok(obj) => obj,
            Err(_) => {
                // Object 1 doesn't exist or can't be fetched - not linearized
                return Ok(None);
            }
        };

        // Check if it's a dictionary with /Linearized entry
        let dict = match &*obj1 {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(None),
        };

        // Check for /Linearized key
        let _linearized_version = match dict.get("Linearized") {
            Some(PDFObject::Number(n)) => *n,
            _ => return Ok(None),
        };

        // Extract linearized information
        let file_size = dict.get("L")
            .and_then(|obj| match obj {
                PDFObject::Number(n) => Some(*n as u64),
                _ => None,
            })
            .ok_or_else(|| PDFError::Generic("Linearized PDF missing /L (file size)".to_string()))?;

        let primary_hint_offset = dict.get("H")
            .and_then(|obj| match obj {
                PDFObject::Array(arr) if arr.len() >= 2 => {
                    match &arr[0] {
                        PDFObject::Number(n) => Some(*n as u64),
                        _ => None,
                    }
                }
                _ => None,
            })
            .unwrap_or(0);

        let primary_hint_length = dict.get("H")
            .and_then(|obj| match obj {
                PDFObject::Array(arr) if arr.len() >= 2 => {
                    match &arr[1] {
                        PDFObject::Number(n) => Some(*n as u64),
                        _ => None,
                    }
                }
                _ => None,
            })
            .unwrap_or(0);

        let first_page_offset = dict.get("O")
            .and_then(|obj| match obj {
                PDFObject::Number(n) => Some(*n as u64),
                _ => None,
            })
            .ok_or_else(|| PDFError::Generic("Linearized PDF missing /O (first page offset)".to_string()))?;

        let first_page_obj_num = dict.get("P")
            .and_then(|obj| match obj {
                PDFObject::Number(n) => Some(*n as u32),
                _ => None,
            })
            .ok_or_else(|| PDFError::Generic("Linearized PDF missing /P (first page object number)".to_string()))?;

        let page_count = dict.get("N")
            .and_then(|obj| match obj {
                PDFObject::Number(n) => Some(*n as u32),
                _ => None,
            })
            .ok_or_else(|| PDFError::Generic("Linearized PDF missing /N (page count)".to_string()))?;

        Ok(Some(LinearizedInfo {
            file_size,
            primary_hint_offset,
            primary_hint_length,
            first_page_offset,
            page_count,
            first_page_obj_num,
        }))
    }

    /// Returns information about linearized PDF optimization, if available.
    ///
    /// # Returns
    /// `Some(&LinearizedInfo)` if the PDF is linearized, `None` otherwise
    pub fn linearized_info(&self) -> Option<&LinearizedInfo> {
        self.linearized.as_ref()
    }

    /// Returns true if this PDF is linearized (optimized for web view).
    pub fn is_linearized(&self) -> bool {
        self.linearized.is_some()
    }

    /// Gets the first page of a linearized PDF with progressive loading optimization.
    ///
    /// For linearized PDFs, this method can load the first page without needing
    /// to load the entire file. This is useful for web viewers that want to display
    /// the first page quickly.
    ///
    /// # Returns
    /// `Some(Page)` if the PDF is linearized and first page can be loaded, `None` otherwise
    ///
    /// # Note
    /// This is a simplified implementation. A full implementation would use the
    /// hint table to load shared resources incrementally.
    pub fn get_first_page_linearized(&mut self) -> PDFResult<Option<Page>> {
        let linearized_info = match &self.linearized {
            Some(info) => info,
            None => return Ok(None),
        };

        // For linearized PDFs, try to fetch the first page object directly
        // without parsing the entire page tree
        match self.xref.fetch(linearized_info.first_page_obj_num, 0) {
            Ok(page_obj) => {
                // Create a page object with the first page index (0)
                // We don't have the reference, so use None for page_ref
                let page = Page::new(0, (*page_obj).clone(), None);
                Ok(Some(page))
            }
            Err(_) => {
                // Fall back to regular page loading
                self.get_page(0).map(Some)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a minimal valid PDF document for testing.
    fn create_minimal_pdf() -> Vec<u8> {
        // A minimal PDF with:
        // - Catalog (object 1)
        // - Pages dict (object 2)
        // - Single page (object 3)
        // - xref table
        // - trailer
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 1 >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R >>\n\
            endobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000115 00000 n\n\
            trailer\n\
            << /Size 4 /Root 1 0 R >>\n\
            startxref\n\
            162\n\
            %%EOF\n";

        pdf.to_vec()
    }

    #[test]
    fn test_find_startxref() {
        let pdf = create_minimal_pdf();
        let offset = PDFDocument::find_startxref(&pdf).unwrap();
        assert_eq!(offset, 162);
    }

    #[test]
    fn test_open_minimal_pdf() {
        let pdf = create_minimal_pdf();
        let doc = PDFDocument::open(pdf).unwrap();

        assert!(doc.catalog().is_some());

        let catalog = doc.catalog().unwrap();
        match catalog {
            PDFObject::Dictionary(dict) => {
                assert!(dict.contains_key("Type"));
                assert!(dict.contains_key("Pages"));
            }
            _ => panic!("Expected catalog to be a dictionary"),
        }
    }

    #[test]
    fn test_get_pages_dict() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        let pages = doc.pages_dict().unwrap();

        match pages {
            PDFObject::Dictionary(dict) => {
                assert_eq!(
                    dict.get("Type"),
                    Some(&PDFObject::Name("Pages".to_string()))
                );
                assert!(dict.contains_key("Kids"));
                assert!(dict.contains_key("Count"));
            }
            _ => panic!("Expected pages to be a dictionary"),
        }
    }

    #[test]
    fn test_page_count() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        let count = doc.page_count().unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_get_page_simple() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        // Get the first (and only) page
        let page = doc.get_page(0).unwrap();
        assert_eq!(page.index(), 0);

        // Verify it's a page dictionary
        match page.dict() {
            PDFObject::Dictionary(dict) => {
                assert_eq!(
                    dict.get("Type"),
                    Some(&PDFObject::Name("Page".to_string()))
                );
                assert!(dict.contains_key("Parent"));
            }
            _ => panic!("Expected page dict to be a dictionary"),
        }
    }

    #[test]
    fn test_get_page_multiple_pages() {
        // Create a PDF with 3 pages (flat structure)
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            4 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            5 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            xref\n\
            0 6\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000127 00000 n\n\
            0000000198 00000 n\n\
            0000000269 00000 n\n\
            trailer\n\
            << /Size 6 /Root 1 0 R >>\n\
            startxref\n\
            340\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();

        // Verify page count
        assert_eq!(doc.page_count().unwrap(), 3);

        // Get each page and verify index
        let page0 = doc.get_page(0).unwrap();
        assert_eq!(page0.index(), 0);

        let page1 = doc.get_page(1).unwrap();
        assert_eq!(page1.index(), 1);

        let page2 = doc.get_page(2).unwrap();
        assert_eq!(page2.index(), 2);
    }

    #[test]
    fn test_page_caching() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        // Get the page twice
        let page1 = doc.get_page(0).unwrap();
        let page2 = doc.get_page(0).unwrap();

        // Both should have the same index (verifies caching works)
        assert_eq!(page1.index(), page2.index());
        assert_eq!(page1.index(), 0);
    }

    #[test]
    fn test_get_page_out_of_bounds() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        // Try to get a page that doesn't exist
        let result = doc.get_page(99);
        assert!(result.is_err());
    }

    #[test]
    #[ignore] // TODO: Fix test PDF structure
    fn test_linearized_pdf_detection() {
        // Create a minimal linearized PDF
        let pdf = b"%PDF-1.4
1 0 obj
<< /Linearized 1.0 /L 1000 /H [ 10 5 ] /O 25 /P 3 /N 3 /T 500 >>
endobj

25 0 obj
<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R >> >> /MediaBox [0 0 612 792] /Contents 4 0 R >>
endobj

2 0 obj
<< /Type /Pages /Kids [25 0 R 26 0 R 27 0 R] /Count 3 >>
endobj

3 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj

4 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Hello World) Tj
ET
endstream
endobj

26 0 obj
<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R >> >> /MediaBox [0 0 612 792] /Contents 5 0 R >>
endobj

5 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Page 2) Tj
ET
endstream
endobj

27 0 obj
<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R >> >> /MediaBox [0 0 612 792] /Contents 6 0 R >>
endobj

6 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Page 3) Tj
ET
endstream
endobj

7 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj

xref
0 8
0000000000 65535 f
0000000010 00000 n
0000000480 00000 n
0000000460 00000 n
0000000360 00000 n
0000000560 00000 n
0000000540 00000 n
0000000640 00000 n
trailer
<< /Size 8 /Root 7 0 R >>
startxref
650
%%EOF";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();

        // Check that it's detected as linearized
        assert!(doc.is_linearized());

        // Check linearized info
        let info = doc.linearized_info().unwrap();
        assert_eq!(info.file_size, 1000);
        assert_eq!(info.first_page_obj_num, 3);
        assert_eq!(info.page_count, 3);
        assert_eq!(info.first_page_offset, 25);
        assert_eq!(info.primary_hint_offset, 10);
        assert_eq!(info.primary_hint_length, 5);
    }

    #[test]
    fn test_non_linearized_pdf() {
        // Use the existing minimal PDF test (which is not linearized)
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        // Should not be detected as linearized
        assert!(!doc.is_linearized());
        assert!(doc.linearized_info().is_none());
    }

    #[test]
    fn test_hierarchical_page_tree() {
        // Create a PDF with 2-level page tree:
        // Root Pages -> 2 intermediate Pages nodes -> 2 pages each
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R 4 0 R] /Count 4 >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Pages /Kids [5 0 R 6 0 R] /Count 2 /Parent 2 0 R >>\n\
            endobj\n\
            4 0 obj\n\
            << /Type /Pages /Kids [7 0 R 8 0 R] /Count 2 /Parent 2 0 R >>\n\
            endobj\n\
            5 0 obj\n\
            << /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            6 0 obj\n\
            << /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            7 0 obj\n\
            << /Type /Page /Parent 4 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            8 0 obj\n\
            << /Type /Page /Parent 4 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            xref\n\
            0 9\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000121 00000 n\n\
            0000000198 00000 n\n\
            0000000275 00000 n\n\
            0000000346 00000 n\n\
            0000000417 00000 n\n\
            0000000488 00000 n\n\
            trailer\n\
            << /Size 9 /Root 1 0 R >>\n\
            startxref\n\
            559\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();

        // Verify page count
        assert_eq!(doc.page_count().unwrap(), 4);

        // Get all 4 pages and verify they have correct indices
        for i in 0..4 {
            let page = doc.get_page(i).unwrap();
            assert_eq!(page.index(), i);

            // Verify it's a page
            match page.dict() {
                PDFObject::Dictionary(dict) => {
                    assert_eq!(
                        dict.get("Type"),
                        Some(&PDFObject::Name("Page".to_string()))
                    );
                }
                _ => panic!("Expected page dict to be a dictionary"),
            }
        }
    }

    #[test]
    fn test_inherited_media_box() {
        // Create a PDF where MediaBox is defined at the Pages level, not on individual pages
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R >>\n\
            endobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000139 00000 n\n\
            trailer\n\
            << /Size 4 /Root 1 0 R >>\n\
            startxref\n\
            186\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();
        let page = doc.get_page(0).unwrap();

        // Page itself doesn't have MediaBox
        assert!(page.get("MediaBox").is_none());

        // But we can get it via inheritance
        let media_box = doc.get_media_box(&page).unwrap();
        match media_box {
            PDFObject::Array(arr) => {
                assert_eq!(arr.len(), 4);
                assert_eq!(arr[0], PDFObject::Number(0.0));
                assert_eq!(arr[1], PDFObject::Number(0.0));
                assert_eq!(arr[2], PDFObject::Number(612.0));
                assert_eq!(arr[3], PDFObject::Number(792.0));
            }
            _ => panic!("Expected MediaBox to be an array"),
        }
    }

    #[test]
    fn test_inherited_resources() {
        // Create a PDF where Resources is defined at the Pages level
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /Font << >> >> >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R >>\n\
            endobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000144 00000 n\n\
            trailer\n\
            << /Size 4 /Root 1 0 R >>\n\
            startxref\n\
            191\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();
        let page = doc.get_page(0).unwrap();

        // Get Resources via inheritance
        let resources = doc.get_resources(&page).unwrap();
        match resources {
            PDFObject::Dictionary(dict) => {
                assert!(dict.contains_key("Font"));
            }
            _ => panic!("Expected Resources to be a dictionary"),
        }
    }

    #[test]
    fn test_property_override() {
        // Create a PDF where MediaBox is defined at both Pages and Page level
        // Page level should take precedence
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>\n\
            endobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000139 00000 n\n\
            trailer\n\
            << /Size 4 /Root 1 0 R >>\n\
            startxref\n\
            210\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();
        let page = doc.get_page(0).unwrap();

        // Should get the page-level MediaBox (300x400), not the inherited one (612x792)
        let media_box = doc.get_media_box(&page).unwrap();
        match media_box {
            PDFObject::Array(arr) => {
                assert_eq!(arr[2], PDFObject::Number(300.0));
                assert_eq!(arr[3], PDFObject::Number(400.0));
            }
            _ => panic!("Expected MediaBox to be an array"),
        }
    }

    #[test]
    fn test_multi_level_inheritance() {
        // Create a hierarchical page tree where MediaBox is at the root Pages level
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 2 /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /Parent 2 0 R >>\n\
            endobj\n\
            4 0 obj\n\
            << /Type /Page /Parent 3 0 R >>\n\
            endobj\n\
            5 0 obj\n\
            << /Type /Page /Parent 3 0 R >>\n\
            endobj\n\
            xref\n\
            0 6\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000139 00000 n\n\
            0000000216 00000 n\n\
            0000000263 00000 n\n\
            trailer\n\
            << /Size 6 /Root 1 0 R >>\n\
            startxref\n\
            310\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();

        // Both pages should inherit MediaBox from the root Pages level
        for i in 0..2 {
            let page = doc.get_page(i).unwrap();
            let media_box = doc.get_media_box(&page).unwrap();
            match media_box {
                PDFObject::Array(arr) => {
                    assert_eq!(arr[2], PDFObject::Number(612.0));
                    assert_eq!(arr[3], PDFObject::Number(792.0));
                }
                _ => panic!("Expected MediaBox to be an array"),
            }
        }
    }
}
