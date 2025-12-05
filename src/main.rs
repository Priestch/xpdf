use pdf_x::{PDFDocument, PDFObject, XRefEntry};
use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("PDF Structure Inspector");
        eprintln!("Usage: {} <pdf-file> [options]", args[0]);
        eprintln!("\nOptions:");
        eprintln!("  --catalog        Show document catalog");
        eprintln!("  --xref           Show cross-reference table");
        eprintln!("  --trailer        Show trailer dictionary");
        eprintln!("  --pages          Show pages dictionary");
        eprintln!("  --object <num>   Show specific object by number");
        eprintln!("  --all            Show all information (default)");
        process::exit(1);
    }

    let pdf_path = &args[1];

    // Parse options
    let show_all = args.len() == 2 || args.contains(&"--all".to_string());
    let show_catalog = show_all || args.contains(&"--catalog".to_string());
    let show_xref = show_all || args.contains(&"--xref".to_string());
    let show_trailer = show_all || args.contains(&"--trailer".to_string());
    let show_pages = show_all || args.contains(&"--pages".to_string());

    // Check for --object option
    let object_num = if let Some(pos) = args.iter().position(|arg| arg == "--object") {
        if pos + 1 < args.len() {
            args[pos + 1].parse::<u32>().ok()
        } else {
            eprintln!("Error: --object requires an object number");
            process::exit(1);
        }
    } else {
        None
    };

    // Read PDF file
    let pdf_data = match fs::read(pdf_path) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Error reading PDF file '{}': {}", pdf_path, e);
            process::exit(1);
        }
    };

    // Open PDF document
    let mut doc = match PDFDocument::open(pdf_data) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("Error parsing PDF: {:?}", e);
            process::exit(1);
        }
    };

    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║           PDF Structure Inspector                         ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!("\nFile: {}\n", pdf_path);

    // Show basic information
    println!("═══════════════ BASIC INFORMATION ═══════════════");
    if let Ok(page_count) = doc.page_count() {
        println!("Page Count: {}", page_count);
    }
    println!("XRef Entries: {}", doc.xref().len());
    println!();

    // Show catalog
    if show_catalog {
        println!("═══════════════ DOCUMENT CATALOG ═══════════════");
        if let Some(catalog) = doc.catalog() {
            print_object(catalog, 0);
        } else {
            println!("No catalog found");
        }
        println!();
    }

    // Show trailer
    if show_trailer {
        println!("═══════════════ TRAILER DICTIONARY ═══════════════");
        if let Some(trailer) = doc.xref().trailer() {
            print_object(trailer, 0);
        } else {
            println!("No trailer found");
        }
        println!();
    }

    // Show xref table
    if show_xref {
        println!("═══════════════ CROSS-REFERENCE TABLE ═══════════════");
        print_xref_table(doc.xref());
        println!();
    }

    // Show pages dictionary
    if show_pages {
        println!("═══════════════ PAGES DICTIONARY ═══════════════");
        match doc.pages_dict() {
            Ok(pages) => print_object(&pages, 0),
            Err(e) => println!("Error retrieving pages dictionary: {:?}", e),
        }
        println!();
    }

    // Show specific object
    if let Some(num) = object_num {
        println!("═══════════════ OBJECT {} 0 ═══════════════", num);
        match doc.xref_mut().fetch(num, 0) {
            Ok(obj) => print_object(&obj, 0),
            Err(e) => println!("Error fetching object {}: {:?}", num, e),
        }
        println!();
    }
}

fn print_object(obj: &PDFObject, indent: usize) {
    let indent_str = "  ".repeat(indent);

    match obj {
        PDFObject::Null => println!("{}null", indent_str),
        PDFObject::Boolean(b) => println!("{}{}", indent_str, b),
        PDFObject::Number(n) => {
            // Format numbers nicely (show integers without decimals)
            if n.fract() == 0.0 {
                println!("{}{}", indent_str, *n as i64);
            } else {
                println!("{}{}", indent_str, n);
            }
        }
        PDFObject::String(s) => {
            let display = String::from_utf8_lossy(s);
            if display.len() > 100 {
                println!("{}({}...)", indent_str, &display[..100]);
            } else {
                println!("{}({})", indent_str, display);
            }
        }
        PDFObject::HexString(s) => {
            let hex_str: String = s.iter().map(|b| format!("{:02x}", b)).collect();
            if hex_str.len() > 100 {
                println!("{}<{}...>", indent_str, &hex_str[..100]);
            } else {
                println!("{}<{}>", indent_str, hex_str);
            }
        }
        PDFObject::Name(n) => println!("{}/{}", indent_str, n),
        PDFObject::Array(arr) => {
            if arr.is_empty() {
                println!("{}[]", indent_str);
            } else {
                println!("{}[", indent_str);
                for item in arr {
                    print_object(item, indent + 1);
                }
                println!("{}]", indent_str);
            }
        }
        PDFObject::Dictionary(dict) => {
            if dict.is_empty() {
                println!("{}<< >>", indent_str);
            } else {
                println!("{}<<", indent_str);
                // Sort keys for consistent output
                let mut keys: Vec<_> = dict.keys().collect();
                keys.sort();

                for key in keys {
                    let value = &dict[key];
                    print!("{}/{}:", "  ".repeat(indent + 1), key);
                    match value {
                        PDFObject::Dictionary(_) | PDFObject::Array(_) => {
                            println!();
                            print_object(value, indent + 2);
                        }
                        _ => {
                            print!(" ");
                            print_object_inline(value);
                        }
                    }
                }
                println!("{}>>", indent_str);
            }
        }
        PDFObject::Stream { dict, data } => {
            println!("{}stream ({} bytes)", indent_str, data.len());
            println!("{}<<", indent_str);
            // Sort keys for consistent output
            let mut keys: Vec<_> = dict.keys().collect();
            keys.sort();

            for key in keys {
                let value = &dict[key];
                print!("{}/{}:", "  ".repeat(indent + 1), key);
                match value {
                    PDFObject::Dictionary(_) | PDFObject::Array(_) => {
                        println!();
                        print_object(value, indent + 2);
                    }
                    _ => {
                        print!(" ");
                        print_object_inline(value);
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

fn print_object_inline(obj: &PDFObject) {
    match obj {
        PDFObject::Null => println!("null"),
        PDFObject::Boolean(b) => println!("{}", b),
        PDFObject::Number(n) => {
            if n.fract() == 0.0 {
                println!("{}", *n as i64);
            } else {
                println!("{}", n);
            }
        }
        PDFObject::String(s) => {
            let display = String::from_utf8_lossy(s);
            if display.len() > 50 {
                println!("({}...)", &display[..50]);
            } else {
                println!("({})", display);
            }
        }
        PDFObject::HexString(s) => {
            let hex_str: String = s.iter().map(|b| format!("{:02x}", b)).collect();
            if hex_str.len() > 50 {
                println!("<{}...>", &hex_str[..50]);
            } else {
                println!("<{}>", hex_str);
            }
        }
        PDFObject::Name(n) => println!("/{}", n),
        PDFObject::Array(_) => println!("[...]"),
        PDFObject::Dictionary(_) => println!("<< ... >>"),
        PDFObject::Stream { dict: _, data } => println!("stream ({} bytes)", data.len()),
        PDFObject::Ref { num, generation } => println!("{} {} R", num, generation),
        PDFObject::EOF => println!("EOF"),
    }
}

fn print_xref_table(xref: &pdf_x::XRef) {
    println!("Total entries: {}\n", xref.len());
    println!("{:<8} {:<12} {:<12} {:<8}", "Object", "Type", "Offset/Ref", "Gen");
    println!("{}", "─".repeat(50));

    for i in 0..xref.len() {
        if let Some(entry) = xref.get_entry(i as u32) {
            match entry {
                XRefEntry::Free { next_free, generation } => {
                    println!(
                        "{:<8} {:<12} {:<12} {:<8}",
                        i, "free", next_free, generation
                    );
                }
                XRefEntry::Uncompressed { offset, generation } => {
                    println!(
                        "{:<8} {:<12} {:<12} {:<8}",
                        i, "uncompressed", offset, generation
                    );
                }
                XRefEntry::Compressed {
                    obj_stream_num,
                    index,
                } => {
                    println!(
                        "{:<8} {:<12} {:<12} {:<8}",
                        i,
                        "compressed",
                        format!("{}[{}]", obj_stream_num, index),
                        "0"
                    );
                }
            }
        }
    }
}
