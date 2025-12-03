// Example: Reading a PDF document and extracting basic information

use pdf_x::{PDFDocument, PDFObject};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a minimal test PDF
    let pdf_data = create_test_pdf();

    // Open the PDF document
    let mut doc = PDFDocument::open(pdf_data)?;

    println!("PDF Document opened successfully!");

    // Get the catalog (root dictionary)
    if let Some(catalog) = doc.catalog() {
        println!("\nCatalog:");
        print_object(catalog, 1);
    }

    // Get page count
    let page_count = doc.page_count()?;
    println!("\nTotal pages: {}", page_count);

    // Get the Pages dictionary
    let pages_dict = doc.pages_dict()?;
    println!("\nPages dictionary:");
    print_object(&pages_dict, 1);

    Ok(())
}

fn print_object(obj: &PDFObject, indent: usize) {
    let indent_str = "  ".repeat(indent);

    match obj {
        PDFObject::Null => println!("{}null", indent_str),
        PDFObject::Boolean(b) => println!("{}{}", indent_str, b),
        PDFObject::Number(n) => println!("{}{}", indent_str, n),
        PDFObject::String(s) => {
            println!("{}({})", indent_str, String::from_utf8_lossy(s))
        }
        PDFObject::HexString(s) => {
            let hex_str: String = s.iter().map(|b| format!("{:02x}", b)).collect();
            println!("{}<{}>", indent_str, hex_str)
        }
        PDFObject::Name(n) => println!("{}/{}", indent_str, n),
        PDFObject::Array(arr) => {
            println!("{}[", indent_str);
            for item in arr {
                print_object(item, indent + 1);
            }
            println!("{}]", indent_str);
        }
        PDFObject::Dictionary(dict) => {
            println!("{}<<", indent_str);
            for (key, value) in dict {
                print!("{}/{}:", "  ".repeat(indent + 1), key);
                match value {
                    PDFObject::Dictionary(_) | PDFObject::Array(_) => {
                        println!();
                        print_object(value, indent + 1);
                    }
                    _ => {
                        print!(" ");
                        print_object(value, 0);
                    }
                }
            }
            println!("{}>>", indent_str);
        }
        PDFObject::Ref { num, generation } => {
            println!("{}{} {} R", indent_str, num, generation)
        }
        PDFObject::EOF => println!("{}EOF", indent_str),
    }
}

fn create_test_pdf() -> Vec<u8> {
    // A minimal but valid PDF with multiple pages
    let pdf = b"%PDF-1.4\n\
        1 0 obj\n\
        << /Type /Catalog /Pages 2 0 R >>\n\
        endobj\n\
        2 0 obj\n\
        << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\n\
        endobj\n\
        3 0 obj\n\
        << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n\
        endobj\n\
        4 0 obj\n\
        << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n\
        endobj\n\
        xref\n\
        0 5\n\
        0000000000 65535 f\n\
        0000000009 00000 n\n\
        0000000058 00000 n\n\
        0000000127 00000 n\n\
        0000000209 00000 n\n\
        trailer\n\
        << /Size 5 /Root 1 0 R >>\n\
        startxref\n\
        263\n\
        %%EOF\n";

    pdf.to_vec()
}
