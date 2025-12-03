use super::base_stream::BaseStream;
use super::error::{PDFError, PDFResult};
use super::parser::PDFObject;
use super::stream::Stream;
use super::xref::XRef;

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

        Ok(PDFDocument { xref, catalog })
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
            0000000117 00000 n\n\
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
}
