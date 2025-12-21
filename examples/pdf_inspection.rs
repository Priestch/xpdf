//! PDF Structure Inspection Example
//!
//! This example demonstrates how to programmatically inspect PDF files:
//! - Reading PDF structure and metadata
//! - Accessing specific objects and dictionaries
//! - Analyzing the cross-reference table
//! - Extracting document information
//!
//! Run with: cargo run --example pdf_inspection <pdf_file>

use pdf_x::core::{PDFDocument, PDFObject, XRefEntry};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: cargo run --example pdf_inspection <pdf_file>");
        eprintln!("\nThis example demonstrates programmatic PDF inspection.");
        return Ok(());
    }

    let pdf_path = &args[1];
    println!("🔍 PDF Inspection Example: {}", pdf_path);

    // Open PDF document
    let pdf_data = std::fs::read(pdf_path)?;
    let mut doc = PDFDocument::open(pdf_data)?;

    // Basic document information
    println!("\n📋 Document Information:");
    println!("  File: {}", pdf_path);
    println!("  Size: {} bytes", std::fs::metadata(pdf_path)?.len());

    if let Ok(page_count) = doc.page_count() {
        println!("  Pages: {}", page_count);
    }

    println!("  Linearized: {}", doc.is_linearized());
    println!("  XRef entries: {}", doc.xref().len());

    // Inspect document catalog
    println!("\n🗂️  Document Catalog:");
    if let Some(catalog) = doc.catalog() {
        inspect_dictionary("Catalog", catalog, 1);
    } else {
        println!("  No catalog found");
    }

    // Inspect trailer dictionary
    println!("\n🚚 Trailer Dictionary:");
    if let Some(trailer) = doc.xref().trailer() {
        inspect_dictionary("Trailer", trailer, 1);
    } else {
        println!("  No trailer found");
    }

    // Inspect pages dictionary
    println!("\n📄 Pages Dictionary:");
    match doc.pages_dict() {
        Ok(pages) => inspect_dictionary("Pages", &pages, 1),
        Err(e) => println!("  Error: {:?}", e),
    }

    // Analyze XRef table
    println!("\n📊 Cross-Reference Table Analysis:");
    analyze_xref_table(doc.xref());

    // Show interesting objects
    println!("\n🔎 Key Objects:");
    inspect_key_objects(&mut doc)?;

    // Object statistics
    println!("\n📈 Object Statistics:");
    show_object_statistics(&mut doc)?;

    Ok(())
}

fn inspect_dictionary(name: &str, obj: &PDFObject, indent: usize) {
    let indent_str = "  ".repeat(indent);

    match obj {
        PDFObject::Dictionary(dict) => {
            println!("{}{}: {} entries", indent_str, name, dict.len());

            for (key, value) in dict {
                match value {
                    PDFObject::Dictionary(_) => {
                        println!("{}  /{}: [Dictionary]", indent_str, key);
                        if key == "Info" || key == "Root" {
                            inspect_dictionary(&format!("{}->{}", name, key), value, indent + 1);
                        }
                    }
                    PDFObject::Array(arr) => {
                        println!("{}  /{}: [Array] ({} items)", indent_str, key, arr.len());
                        if key == "Kids" && arr.len() <= 3 {
                            for (i, item) in arr.iter().enumerate() {
                                println!("{}    [{}]: {:?}", indent_str, i, item);
                            }
                        }
                    }
                    PDFObject::Ref { num, generation } => {
                        println!("{}  /{}: {} {} R", indent_str, key, num, generation);
                    }
                    PDFObject::String(s) => {
                        let text = String::from_utf8_lossy(s);
                        if text.len() > 50 {
                            println!("{}  /{}: \"{}...\"", indent_str, key, &text[..50]);
                        } else {
                            println!("{}  /{}: \"{}\"", indent_str, key, text);
                        }
                    }
                    PDFObject::Name(n) => {
                        println!("{}  /{}: /{}", indent_str, key, n);
                    }
                    PDFObject::Number(n) => {
                        if n.fract() == 0.0 {
                            println!("{}  /{}: {}", indent_str, key, *n as i64);
                        } else {
                            println!("{}  /{}: {}", indent_str, key, n);
                        }
                    }
                    _ => {
                        println!("{}  /{}: {:?}", indent_str, key, value);
                    }
                }
            }
        }
        _ => {
            println!("{}{}: {:?}", indent_str, name, obj);
        }
    }
}

fn analyze_xref_table(xref: &pdf_x::XRef) {
    let mut free_count = 0;
    let mut uncompressed_count = 0;
    let mut compressed_count = 0;

    for i in 0..xref.len() {
        if let Some(entry) = xref.get_entry(i as u32) {
            match entry {
                XRefEntry::Free { .. } => free_count += 1,
                XRefEntry::Uncompressed { .. } => uncompressed_count += 1,
                XRefEntry::Compressed { .. } => compressed_count += 1,
            }
        }
    }

    println!("  Total objects: {}", xref.len());
    println!("  Free entries: {}", free_count);
    println!("  Uncompressed objects: {}", uncompressed_count);
    println!("  Compressed objects: {}", compressed_count);

    if compressed_count > 0 {
        println!("  ✨ Object streams are being used (compression)");
    }

    if uncompressed_count > 0 {
        println!("  📄 Traditional object format");
    }
}

fn inspect_key_objects(doc: &mut PDFDocument) -> Result<(), Box<dyn std::error::Error>> {
    // Look for common object types
    let mut page_count = 0;
    let mut font_count = 0;
    let mut stream_count = 0;

    for i in 0..doc.xref().len().min(20) { // Limit to first 20 objects
        let obj_num = i as u32;
        if let Ok(obj) = doc.xref_mut().fetch(obj_num, 0) {
            match &*obj {
                PDFObject::Dictionary(dict) => {
                    if let Some(PDFObject::Name(name)) = dict.get("Type") {
                        match name.as_str() {
                            "Page" => {
                                page_count += 1;
                                if page_count <= 3 {
                                    println!("  Object {} - Page: {:?}", obj_num, dict.keys().collect::<Vec<_>>());
                                }
                            }
                            "Font" => {
                                font_count += 1;
                                if font_count <= 2 {
                                    println!("  Object {} - Font: {:?}", obj_num, dict.keys().collect::<Vec<_>>());
                                }
                            }
                            "Catalog" => {
                                println!("  Object {} - Catalog: {:?}", obj_num, dict.keys().collect::<Vec<_>>());
                            }
                            "Pages" => {
                                println!("  Object {} - Pages: {:?}", obj_num, dict.keys().collect::<Vec<_>>());
                            }
                            _ => {}
                        }
                    }
                }
                PDFObject::Stream { dict, .. } => {
                    stream_count += 1;
                    if stream_count <= 2 {
                        if let Some(PDFObject::Name(name)) = dict.get("Type") {
                            println!("  Object {} - Stream {}: {} keys", obj_num, name, dict.len());
                        } else {
                            println!("  Object {} - Stream: {} keys", obj_num, dict.len());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    println!("  Found: {} pages, {} fonts, {} streams (showing first few)",
             page_count, font_count, stream_count);

    Ok(())
}

fn show_object_statistics(doc: &mut PDFDocument) -> Result<(), Box<dyn std::error::Error>> {
    let mut stats = std::collections::HashMap::new();
    let mut total_size = 0;

    // Sample first 50 objects for statistics
    for i in 0..doc.xref().len().min(50) {
        let obj_num = i as u32;
        if let Ok(obj) = doc.xref_mut().fetch(obj_num, 0) {
            let size = estimate_object_size(&*obj);
            total_size += size;

            let obj_type = get_object_type(&*obj);
            *stats.entry(obj_type).or_insert(0) += 1;
        }
    }

    println!("  Sampled objects: {}", doc.xref().len().min(50));
    println!("  Estimated total size: {} bytes", total_size);

    println!("  Object types:");
    for (obj_type, count) in stats {
        println!("    {}: {}", obj_type, count);
    }

    Ok(())
}

fn estimate_object_size(obj: &PDFObject) -> usize {
    match obj {
        PDFObject::Null => 4,
        PDFObject::Boolean(_) => 5,
        PDFObject::Number(_) => 10,
        PDFObject::String(s) => s.len() + 2,
        PDFObject::HexString(s) => s.len() * 2 + 2,
        PDFObject::Name(n) => n.len() + 1,
        PDFObject::Command(c) => c.len(),
        PDFObject::Array(arr) => arr.iter().map(estimate_object_size).sum::<usize>() + 2,
        PDFObject::Dictionary(dict) => {
            dict.iter().map(|(k, v)| k.len() + estimate_object_size(v) + 3).sum::<usize>() + 4
        }
        PDFObject::Stream { dict, data } => {
            estimate_object_size(&PDFObject::Dictionary(dict.clone())) + data.len() + 10
        }
        PDFObject::Ref { .. } => 15,
        PDFObject::EOF => 3,
    }
}

fn get_object_type(obj: &PDFObject) -> &'static str {
    match obj {
        PDFObject::Null => "Null",
        PDFObject::Boolean(_) => "Boolean",
        PDFObject::Number(_) => "Number",
        PDFObject::String(_) => "String",
        PDFObject::HexString(_) => "HexString",
        PDFObject::Name(_) => "Name",
        PDFObject::Command(_) => "Command",
        PDFObject::Array(_) => "Array",
        PDFObject::Dictionary(_) => "Dictionary",
        PDFObject::Stream { .. } => "Stream",
        PDFObject::Ref { .. } => "Reference",
        PDFObject::EOF => "EOF",
    }
}

/// Helper function to extract document metadata if available
#[allow(dead_code)]
fn extract_document_metadata(doc: &mut PDFDocument) -> Result<(), Box<dyn std::error::Error>> {
    // Try to get Info dictionary from trailer - clone to avoid borrow issues
    let info_ref = if let Some(trailer) = doc.xref().trailer() {
        if let PDFObject::Dictionary(trailer_dict) = trailer {
            if let Some(PDFObject::Ref { num, generation: _ }) = trailer_dict.get("Info") {
                Some(*num)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    if let Some(info_num) = info_ref {
        if let Ok(info_obj) = doc.xref_mut().fetch(info_num, 0) {
            if let PDFObject::Dictionary(info_dict) = &*info_obj {
                println!("\n📝 Document Metadata:");
                for (key, value) in info_dict {
                    match value {
                        PDFObject::String(s) => {
                            let text = String::from_utf8_lossy(&s);
                            println!("  {}: {}", key, text);
                        }
                        _ => {
                            println!("  {}: {:?}", key, value);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}