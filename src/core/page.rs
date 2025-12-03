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
