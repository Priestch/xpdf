use pdf_x::{PDFDocument, PDFObject, XRefEntry};
use pdf_x::core::{ImageDecoder, ImageFormat, Page};
use pdf_x::core::decode::{decode_flate, decode_png_predictor};
use std::env;
use std::fs;
use std::path::Path;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("PDF Structure Inspector");
        eprintln!("Usage: {} <pdf-file> [options]", args[0]);
        eprintln!("\nOptions:");
        eprintln!("  --all            Show all information (default)");
        eprintln!("  --catalog        Show document catalog");
        eprintln!("  --xref           Show cross-reference table");
        eprintln!("  --trailer        Show trailer dictionary");
        eprintln!("  --pages          Show pages dictionary");
        eprintln!("  --images         Extract and show image information");
        eprintln!("  --object <num>   Show specific object by number");
        eprintln!("  --version        Show PDF version");
        eprintln!("  --info           Show document metadata (Title, Author, etc.)");
        eprintln!("  --fonts          List fonts used in the document");
        eprintln!("  --extract-text   Extract text from all pages");
        eprintln!("  --outline        Show document outline (bookmarks)");
        eprintln!("  --stats          Show summary statistics");
        eprintln!("  --page-sizes     Show page dimensions");
        process::exit(1);
    }

    let pdf_path = &args[1];

    // Check if file exists before trying to parse
    if !Path::new(pdf_path).exists() {
        eprintln!("Error: File not found: {}", pdf_path);
        process::exit(1);
    }

    // Parse options - use any() instead of contains() to avoid String allocations
    let show_all = args.len() == 2 || args.iter().any(|x| x == "--all");
    let show_catalog = show_all || args.iter().any(|x| x == "--catalog");
    let show_xref = show_all || args.iter().any(|x| x == "--xref");
    let show_trailer = show_all || args.iter().any(|x| x == "--trailer");
    let show_pages = show_all || args.iter().any(|x| x == "--pages");
    let show_images = args.iter().any(|x| x == "--images");
    let show_version = args.iter().any(|x| x == "--version");
    let show_info = args.iter().any(|x| x == "--info");
    let show_fonts = args.iter().any(|x| x == "--fonts");
    let extract_text = args.iter().any(|x| x == "--extract-text");
    let show_outline = args.iter().any(|x| x == "--outline");
    let show_stats = args.iter().any(|x| x == "--stats");
    let show_page_sizes = args.iter().any(|x| x == "--page-sizes");

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

    // Open PDF document using progressive/chunked loading
    // This loads the PDF in 64KB chunks rather than reading the entire file into memory
    let mut doc = match PDFDocument::open_file(pdf_path, None, None) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("Error parsing PDF: {:?}", e);
            eprintln!("\nNote: Some PDFs with compressed streams may not be fully supported yet.");
            eprintln!("This is a known limitation that will be addressed in future updates.");
            process::exit(1);
        }
    };

    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║           PDF Structure Inspector                         ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!("\nFile: {}\n", pdf_path);

    // Get file size
    let file_size = fs::metadata(pdf_path)
        .map(|m| m.len())
        .ok();

    // Show basic information
    println!("═══════════════ BASIC INFORMATION ═══════════════");
    if let Ok(page_count) = doc.page_count() {
        println!("Page Count: {}", page_count);
    }
    println!("XRef Entries: {}", doc.xref().len());

    // Show file size
    if let Some(size) = file_size {
        if size < 1024 {
            println!("File Size: {} B", size);
        } else if size < 1024 * 1024 {
            println!("File Size: {:.2} KB", size as f64 / 1024.0);
        } else if size < 1024 * 1024 * 1024 {
            println!("File Size: {:.2} MB", size as f64 / (1024.0 * 1024.0));
        } else {
            println!("File Size: {:.2} GB", size as f64 / (1024.0 * 1024.0 * 1024.0));
        }
    }

    // Show linearized status
    if doc.is_linearized() {
        println!("Linearized: Yes (optimized for web viewing)");
    } else {
        println!("Linearized: No");
    }

    // Show PDF version
    if show_version {
        match doc.pdf_version() {
            Ok(version) => println!("PDF Version: {}", version),
            Err(e) => println!("PDF Version: Unknown ({:?})", e),
        }
    }
    println!();

    // Show document info
    if show_info {
        println!("═══════════════ DOCUMENT INFO ═══════════════");
        match doc.document_info() {
            Ok(Some(info)) => print_document_info(&info),
            Ok(None) => println!("No document info dictionary found"),
            Err(e) => println!("Error retrieving document info: {:?}", e),
        }
        println!();
    }

    // Show fonts
    if show_fonts {
        println!("═══════════════ FONT LIST ═══════════════");
        extract_fonts(&mut doc);
        println!();
    }

    // Extract text
    if extract_text {
        println!("═══════════════ TEXT EXTRACTION ═══════════════");
        extract_all_text(&mut doc);
        println!();
    }

    // Show outline
    if show_outline {
        println!("═══════════════ DOCUMENT OUTLINE ═══════════════");
        extract_outline(&mut doc);
        println!();
    }

    // Show stats
    if show_stats {
        println!("═══════════════ STATISTICS ═══════════════");
        show_statistics(&mut doc, file_size);
        println!();
    }

    // Show page sizes
    if show_page_sizes {
        println!("═══════════════ PAGE SIZES ═══════════════");
        show_page_sizes_info(&mut doc);
        println!();
    }

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

    // Show images
    if show_images {
        println!("═══════════════ IMAGE EXTRACTION ═══════════════");
        extract_images(&mut doc);
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
        PDFObject::Command(c) => println!("{}{}", indent_str, c),
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
        PDFObject::Command(c) => println!("{}", c),
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

fn extract_images(doc: &mut PDFDocument) {
    // Get page count
    let page_count = match doc.page_count() {
        Ok(count) => count,
        Err(e) => {
            println!("Error getting page count: {:?}", e);
            return;
        }
    };

    if page_count == 0 {
        println!("No pages found in PDF");
        return;
    }

    println!("Analyzing {} page(s) for images...\n", page_count);

    let mut total_images = 0;
    let mut total_image_size = 0;

    for page_num in 0..page_count {
        println!("📄 Page {}:", page_num + 1);

        // Get page
        let page = match doc.get_page(page_num as usize) {
            Ok(page) => page,
            Err(e) => {
                println!("  ❌ Error getting page {}: {:?}", page_num + 1, e);
                continue;
            }
        };

        // Extract image metadata from page resources
        let images_found = extract_page_images(&page, doc, &mut total_image_size);
        total_images += images_found;

        if images_found == 0 {
            println!("  ℹ️  No images found on this page");
        } else {
            println!("  ✅ Found {} image(s) on this page", images_found);
        }
        println!();
    }

    // Summary
    println!("═══════════════ IMAGE SUMMARY ═══════════════");
    println!("Total images found: {}", total_images);
    println!("Total image data: {} bytes", total_image_size);

    if total_images > 0 {
        let avg_size = total_image_size as f64 / total_images as f64;
        println!("Average image size: {:.1} KB", avg_size / 1024.0);
    }

    // Show image format support status
    println!("\n═══════════════ DECODER STATUS ═══════════════");
    show_decoder_status();
}

fn extract_page_images(page: &Page, doc: &mut PDFDocument, total_size: &mut usize) -> usize {
    let mut images_found = 0;

    // Get page resources
    if let Some(resources) = page.resources() {
        // Look for XObject dictionary
        if let PDFObject::Dictionary(xobject_dict) = resources {
            // Check if there's an XObject entry
            if let Some(xobject_obj) = xobject_dict.get("XObject") {
                // Follow reference if needed
                let xobject_dict = match doc.xref_mut().fetch_if_ref(xobject_obj) {
                    Ok(PDFObject::Dictionary(dict)) => dict,
                    Ok(PDFObject::Stream { dict, .. }) => dict,
                    Ok(_) => {
                        println!("  ⚠️  XObject is not a dictionary or stream");
                        return 0;
                    }
                    Err(e) => {
                        println!("  ⚠️  Error fetching XObject: {:?}", e);
                        return 0;
                    }
                };

                // Iterate through XObject entries
            for (name, xobject_ref) in xobject_dict {
                println!("  🖼️  Image: {}", name);

                // Get the actual XObject
                let xobject = match doc.xref_mut().fetch_if_ref(&xobject_ref) {
                    Ok(obj) => obj,
                    Err(e) => {
                        println!("    ❌ Error fetching XObject '{}': {:?}", name, e);
                        continue;
                    }
                };

                // Check if it's an image stream
                if let PDFObject::Stream { dict, data } = xobject {
                    // Check if it's an image XObject
                    if let Some(subtype) = dict.get("Subtype") {
                        if let PDFObject::Name(subtype_name) = subtype {
                            if subtype_name == "Image" {
                                images_found += 1;
                                *total_size += data.len();

                                // Extract image information
                                extract_image_info(&name, &dict, &data);
                            } else {
                                println!("    ℹ️  XObject '{}' is not an image (subtype: {})", name, subtype_name);
                            }
                        }
                    }
                } else {
                    println!("    ⚠️  XObject '{}' is not a stream", name);
                }
            }
            } else {
                println!("  ℹ️  No XObject dictionary found in page resources");
            }
        } else {
            println!("  ℹ️  No XObject entry found in page resources");
        }
    } else {
        println!("  ℹ️  No resources dictionary found in page");
    }

    images_found
}

fn extract_image_info(name: &str, dict: &std::collections::HashMap<String, PDFObject>, data: &[u8]) {
    println!("    📋 Image Information:");
    println!("      Name: {}", name);
    println!("      Data size: {} bytes ({:.1} KB)", data.len(), data.len() as f64 / 1024.0);

    // Detect format from PDF filter information first (more reliable for PDF images)
    let format = if let Some(filter) = dict.get("Filter") {
        match filter {
            PDFObject::Name(filter_name) => {
                match filter_name.as_str() {
                    "DCTDecode" => ImageFormat::JPEG,
                    "JPXDecode" => ImageFormat::JPEG2000,
                    "JBIG2Decode" => ImageFormat::JBIG2,
                    "FlateDecode" => {
                        // FlateDecode images are raw compressed pixel data
                        // Check if it's actually a PNG (rare but possible)
                        if data.len() >= 4 {
                            let detected = ImageDecoder::detect_format(&data[..4]);
                            if detected != ImageFormat::Unknown {
                                detected
                            } else {
                                ImageFormat::Raw
                            }
                        } else {
                            ImageFormat::Raw
                        }
                    }
                    "CCITTFaxDecode" | "RunLengthDecode" => {
                        // For these filters, check the actual data header
                        if data.len() >= 4 {
                            ImageDecoder::detect_format(&data[..4])
                        } else {
                            ImageFormat::Unknown
                        }
                    }
                    _ => ImageFormat::Unknown,
                }
            }
            PDFObject::Array(filters) => {
                // Handle multiple filters - check if any indicate the image format
                let mut detected_format = ImageFormat::Unknown;
                for filter_obj in filters {
                    if let PDFObject::Name(filter_name) = &**filter_obj {
                        match filter_name.as_str() {
                            "DCTDecode" => {
                                detected_format = ImageFormat::JPEG;
                                break;
                            }
                            "JPXDecode" => {
                                detected_format = ImageFormat::JPEG2000;
                                break;
                            }
                            "JBIG2Decode" => {
                                detected_format = ImageFormat::JBIG2;
                                break;
                            }
                            "FlateDecode" => {
                                detected_format = ImageFormat::Raw;
                                break;
                            }
                            _ => continue,
                        }
                    }
                }

                if detected_format != ImageFormat::Unknown {
                    detected_format
                } else {
                    // If no format-specific filter found, check data header
                    if data.len() >= 4 {
                        ImageDecoder::detect_format(&data[..4])
                    } else {
                        ImageFormat::Unknown
                    }
                }
            }
            _ => {
                // Unknown filter type, check data header
                if data.len() >= 4 {
                    ImageDecoder::detect_format(&data[..4])
                } else {
                    ImageFormat::Unknown
                }
            }
        }
    } else {
        // No filter information, could be uncompressed raw data
        if data.len() >= 4 {
            let detected = ImageDecoder::detect_format(&data[..4]);
            if detected != ImageFormat::Unknown {
                detected
            } else {
                ImageFormat::Raw
            }
        } else {
            ImageFormat::Raw
        }
    };
    println!("      Format: {:?}", format);

    // Extract basic image properties from dictionary
    if let Some(width) = dict.get("Width") {
        if let PDFObject::Number(w) = width {
            println!("      Width: {} pixels", *w as u32);
        }
    }

    if let Some(height) = dict.get("Height") {
        if let PDFObject::Number(h) = height {
            println!("      Height: {} pixels", *h as u32);
        }
    }

    if let Some(bpc) = dict.get("BitsPerComponent") {
        if let PDFObject::Number(b) = bpc {
            println!("      Bits per component: {}", *b as u8);
        }
    }

    if let Some(colorspace) = dict.get("ColorSpace") {
        let cs_name = match colorspace {
            PDFObject::Name(name) => name.clone(),
            PDFObject::Ref { num, generation } => format!("Ref({} {})", num, generation),
            _ => "Unknown".to_string(),
        };
        println!("      Color space: {}", cs_name);
    }

    // Show filter/compression info concisely
    if let Some(filter) = dict.get("Filter") {
        let filter_name = match filter {
            PDFObject::Name(name) => name.clone(),
            PDFObject::Array(filters) => {
                let names: Vec<String> = filters.iter()
                    .filter_map(|f| if let PDFObject::Name(n) = &**f { Some(n.clone()) } else { None })
                    .collect();
                names.join(", ")
            }
            _ => "Unknown".to_string(),
        };
        println!("      Filter: {}", filter_name);
    }

    // Test if we can decode this format
    #[cfg(feature = "jpeg-decoding")]
    {
        match format {
            ImageFormat::JPEG | ImageFormat::PNG | ImageFormat::Raw => {
                // Attempt actual decoding
                if format == ImageFormat::Raw {
                    // For Raw images with FlateDecode, the stream data might already be decompressed
                    // by the parser, or it might be raw. Try to decode as-is first, then try decompressing.
                    let has_flate = if let Some(filter) = dict.get("Filter") {
                        matches!(filter, PDFObject::Name(name) if name == "FlateDecode")
                    } else {
                        false
                    };

                    // Try using data as-is first (might already be decompressed)
                    let mut raw_data = data.to_vec();

                    // Extract metadata for decoding
                    let width = dict.get("Width")
                        .and_then(|w| if let PDFObject::Number(n) = w { Some(*n as u32) } else { None })
                        .unwrap_or(0);

                    let height = dict.get("Height")
                        .and_then(|h| if let PDFObject::Number(n) = h { Some(*n as u32) } else { None })
                        .unwrap_or(0);

                    let bpc = dict.get("BitsPerComponent")
                        .and_then(|b| if let PDFObject::Number(n) = b { Some(*n as u8) } else { None })
                        .unwrap_or(8);

                    if let Some(colorspace_obj) = dict.get("ColorSpace") {
                        let color_space = ImageDecoder::parse_color_space(colorspace_obj);

                        if width > 0 && height > 0 {
                            // Try decoding with current data
                            match ImageDecoder::decode_raw_image(&raw_data, width, height, bpc, color_space.clone()) {
                                Ok(decoded) => {
                                    println!("      ✅ Decoded successfully: {}x{} ({} channels)",
                                        decoded.width, decoded.height, decoded.channels);
                                }
                                Err(e) => {
                                    // If decoding failed and we have FlateDecode, try decompressing first
                                    if has_flate {
                                        match decode_flate(&data) {
                                            Ok(mut decompressed) => {
                                                // Check for PNG predictor in DecodeParms
                                                if let Some(decode_parms) = dict.get("DecodeParms") {
                                                    if let PDFObject::Dictionary(parms) = decode_parms {
                                                        if let Some(PDFObject::Number(predictor)) = parms.get("Predictor") {
                                                            let pred_val = *predictor as u32;
                                                            // PNG predictor is 10-15
                                                            if pred_val >= 10 && pred_val <= 15 {
                                                                let colors = parms.get("Colors")
                                                                    .and_then(|c| if let PDFObject::Number(n) = c { Some(*n as usize) } else { None })
                                                                    .unwrap_or(1);

                                                                let columns = parms.get("Columns")
                                                                    .and_then(|c| if let PDFObject::Number(n) = c { Some(*n as usize) } else { None })
                                                                    .unwrap_or(width as usize);

                                                                match decode_png_predictor(&decompressed, colors, bpc as usize, columns) {
                                                                    Ok(unpredicted) => {
                                                                        decompressed = unpredicted;
                                                                    }
                                                                    Err(pred_err) => {
                                                                        println!("      ⚠️  PNG predictor failed: {:?}", pred_err);
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

                                                raw_data = decompressed;

                                                // Try decoding again with decompressed data
                                                match ImageDecoder::decode_raw_image(&raw_data, width, height, bpc, color_space) {
                                                    Ok(decoded) => {
                                                        println!("      ✅ Decoded successfully: {}x{} ({} channels)",
                                                            decoded.width, decoded.height, decoded.channels);
                                                    }
                                                    Err(decode_err) => {
                                                        println!("      ⚠️  Decoding failed: {:?}", decode_err);
                                                    }
                                                }
                                            }
                                            Err(decompress_err) => {
                                                println!("      ⚠️  Decompression failed: {:?}", decompress_err);
                                            }
                                        }
                                    } else {
                                        println!("      ⚠️  Decoding failed: {:?}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                // Format not supported - silently skip
            }
        }
    }

    #[cfg(not(feature = "jpeg-decoding"))]
    {
        // Image decoding not enabled
    }
}

fn show_decoder_status() {
    #[cfg(feature = "jpeg-decoding")]
    {
        println!("✅ JPEG decoding: Enabled (zune-jpeg - hayro's optimized decoder)");
        println!("✅ Raw image decoding: Enabled (FlateDecode with flate2)");
    }

    #[cfg(not(feature = "jpeg-decoding"))]
    {
        println!("⚠️  JPEG decoding: Disabled (enable with --features jpeg-decoding)");
    }

    #[cfg(feature = "png-decoding")]
    {
        println!("✅ PNG decoding: Enabled (image crate)");
    }

    #[cfg(not(feature = "png-decoding"))]
    {
        println!("⚠️  PNG decoding: Disabled (enable with --features png-decoding)");
    }

    #[cfg(feature = "advanced-image-formats")]
    {
        println!("✅ JPEG2000 decoding: Enabled (hayro-jpeg2000)");
        println!("✅ JBIG2 decoding: Enabled (hayro-jbig2)");
    }

    #[cfg(not(feature = "advanced-image-formats"))]
    {
        println!("📝 JPEG2000 decoding: Disabled (enable with --features advanced-image-formats)");
        println!("📝 JBIG2 decoding: Disabled (enable with --features advanced-image-formats)");
    }

    println!("\n💡 Available feature combinations:");
    println!("   --features jpeg-decoding      # JPEG support only");
    println!("   --features png-decoding       # PNG support only");
    println!("   --features advanced-image-formats  # JPEG2000 + JBIG2 support");
    println!("   --features jpeg-decoding,advanced-image-formats  # All formats");
}

fn print_document_info(info: &PDFObject) {
    if let PDFObject::Dictionary(dict) = info {
        // Common PDF info keys with nice display names
        let info_fields = [
            ("Title", "Title"),
            ("Author", "Author"),
            ("Subject", "Subject"),
            ("Keywords", "Keywords"),
            ("Creator", "Creator"),
            ("Producer", "Producer"),
            ("CreationDate", "Creation Date"),
            ("ModDate", "Modification Date"),
            ("Trapped", "Trapped"),
        ];

        for (key, label) in info_fields.iter() {
            if let Some(value) = dict.get(*key) {
                let value_str = match value {
                    PDFObject::String(s) => String::from_utf8_lossy(s).to_string(),
                    PDFObject::Name(n) => format!("/{}", n),
                    PDFObject::Number(n) => {
                        if n.fract() == 0.0 {
                            format!("{}", *n as i64)
                        } else {
                            format!("{}", n)
                        }
                    }
                    PDFObject::Boolean(b) => format!("{}", b),
                    _ => format!("{:?}", value),
                };
                println!("{}: {}", label, value_str);
            }
        }

        // Show any other fields not in our list
        for (key, value) in dict {
            if !info_fields.iter().any(|(k, _)| *k == key) {
                let value_str = match value {
                    PDFObject::String(s) => String::from_utf8_lossy(s).to_string(),
                    PDFObject::Name(n) => format!("/{}", n),
                    _ => format!("{:?}", value),
                };
                println!("{}: {}", key, value_str);
            }
        }
    } else {
        println!("Info is not a dictionary: {:?}", info);
    }
}

fn extract_fonts(doc: &mut PDFDocument) {
    let page_count = match doc.page_count() {
        Ok(count) => count,
        Err(e) => {
            println!("Error getting page count: {:?}", e);
            return;
        }
    };

    let mut font_set = std::collections::HashSet::new();

    for page_num in 0..page_count {
        if let Ok(page) = doc.get_page(page_num as usize) {
            // Get page resources
            if let Some(resources) = page.resources() {
                if let PDFObject::Dictionary(res_dict) = resources {
                    // Look for Font dictionary
                    if let Some(font_obj) = res_dict.get("Font") {
                        // Follow reference if needed
                        let font_dict = match doc.xref_mut().fetch_if_ref(font_obj) {
                            Ok(PDFObject::Dictionary(dict)) => dict,
                            Ok(_) => {
                                continue;
                            }
                            Err(_) => continue,
                        };

                        // Collect font names
                        for (font_name, _font_ref) in font_dict {
                            font_set.insert(font_name.clone());
                        }
                    }
                }
            }
        }
    }

    if font_set.is_empty() {
        println!("No fonts found in document");
    } else {
        println!("Found {} unique font(s):", font_set.len());
        let mut fonts: Vec<_> = font_set.into_iter().collect();
        fonts.sort();
        for font in fonts {
            println!("  /{}", font);
        }
    }
}

fn extract_all_text(doc: &mut PDFDocument) {
    let page_count = match doc.page_count() {
        Ok(count) => count,
        Err(e) => {
            println!("Error getting page count: {:?}", e);
            return;
        }
    };

    let mut total_chars = 0;

    for page_num in 0..page_count {
        if let Ok(page) = doc.get_page(page_num as usize) {
            println!("═══ Page {} ═══", page_num + 1);

            match page.extract_text(doc.xref_mut()) {
                Ok(text_items) => {
                    if text_items.is_empty() {
                        println!("  (No text on this page)");
                    } else {
                        // Group text by line (similar Y position)
                        let mut lines: std::collections::BTreeMap<i32, Vec<String>> = std::collections::BTreeMap::new();

                        for item in text_items {
                            let y_key = if let Some(pos) = item.position {
                                (pos.1 * 10.0) as i32
                            } else {
                                0
                            };
                            lines.entry(y_key).or_insert_with(Vec::new).push(item.text);
                        }

                        // Print lines
                        for (_, texts) in lines.into_iter().rev() {
                            let line = texts.join(" ");
                            println!("  {}", line);
                            total_chars += line.len();
                        }
                    }
                }
                Err(e) => {
                    println!("  Error extracting text: {:?}", e);
                }
            }
            println!();
        }
    }

    println!("Total characters extracted: {}", total_chars);
}

fn extract_outline(doc: &mut PDFDocument) {
    match doc.document_outline() {
        Ok(Some(outlines)) => {
            print_outline_items(doc, &outlines, 0);
        }
        Ok(None) => {
            println!("No document outline found");
        }
        Err(e) => {
            println!("Error retrieving outline: {:?}", e);
        }
    }
}

fn print_outline_items(doc: &mut PDFDocument, outline_obj: &PDFObject, indent: usize) {
    let indent_str = "  ".repeat(indent);

    match outline_obj {
        PDFObject::Dictionary(dict) => {
            // Print title
            if let Some(title_obj) = dict.get("Title") {
                let title = match title_obj {
                    PDFObject::String(s) => String::from_utf8_lossy(s).to_string(),
                    PDFObject::HexString(s) => {
                        let hex_str: String = s.iter().map(|b| format!("{:02x}", b)).collect();
                        String::from_utf8_lossy(&hex_str.as_bytes()).to_string()
                    }
                    _ => format!("{:?}", title_obj),
                };
                println!("{}{}", indent_str, title);

                // Show destination if available
                if let Some(dest_obj) = dict.get("Dest") {
                    match dest_obj {
                        PDFObject::String(s) => {
                            let dest = String::from_utf8_lossy(s);
                            println!("{}  → Destination: {}", indent_str, dest);
                        }
                        PDFObject::Array(arr) => {
                            if !arr.is_empty() {
                                match &*arr[0] {
                                    PDFObject::Ref { num, .. } => {
                                        println!("{}  → Page: {}", indent_str, num);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        PDFObject::Ref { num, .. } => {
                            println!("{}  → Page: {}", indent_str, num);
                        }
                        _ => {}
                    }
                }
            }

            // Recursively print child items (First)
            if let Some(first_obj) = dict.get("First") {
                let first = match doc.xref_mut().fetch_if_ref(first_obj) {
                    Ok(obj) => obj,
                    Err(_) => return,
                };
                print_outline_items(doc, &first, indent + 1);
            }

            // Print sibling items (Next)
            if let Some(next_obj) = dict.get("Next") {
                let next = match doc.xref_mut().fetch_if_ref(next_obj) {
                    Ok(obj) => obj,
                    Err(_) => return,
                };
                print_outline_items(doc, &next, indent);
            }
        }
        _ => {
            println!("{}Invalid outline item", indent_str);
        }
    }
}

fn show_statistics(doc: &mut PDFDocument, file_size: Option<u64>) {
    let page_count = doc.page_count().unwrap_or(0);
    let xref_len = doc.xref().len();
    
    println!("Page Count: {}", page_count);
    println!("XRef Entries: {}", xref_len);
    
    if let Some(size) = file_size {
        if size < 1024 * 1024 {
            println!("File Size: {:.2} KB", size as f64 / 1024.0);
        } else {
            println!("File Size: {:.2} MB", size as f64 / (1024.0 * 1024.0));
        }
        
        if page_count > 0 {
            let avg_page_size = size as f64 / page_count as f64;
            if avg_page_size < 1024.0 {
                println!("Avg Page Size: {:.2} KB", avg_page_size / 1024.0);
            } else {
                println!("Avg Page Size: {:.2} MB", avg_page_size / (1024.0 * 1024.0));
            }
        }
    }
    
    if doc.is_linearized() {
        println!("Linearized: Yes (optimized for fast web view)");
    } else {
        println!("Linearized: No");
    }
    
    match doc.pdf_version() {
        Ok(version) => println!("PDF Version: {}", version),
        Err(_) => println!("PDF Version: Unknown"),
    }
    
    // Count fonts
    let font_count = count_fonts(doc);
    if font_count > 0 {
        println!("Unique Fonts: {}", font_count);
    }
}

fn show_page_sizes_info(doc: &mut PDFDocument) {
    let page_count = match doc.page_count() {
        Ok(count) => count,
        Err(e) => {
            println!("Error getting page count: {:?}", e);
            return;
        }
    };
    
    for page_num in 0..page_count {
        match doc.get_page(page_num as usize) {
            Ok(page) => {
                if let Some(media_box_obj) = page.media_box() {
                    // MediaBox should be an array of 4 numbers [llx, lly, urx, ury]
                    match media_box_obj {
                        PDFObject::Array(arr) if arr.len() >= 4 => {
                            let values: Vec<f64> = arr.iter()
                                .filter_map(|obj| {
                                    if let PDFObject::Number(n) = obj.as_ref() {
                                        Some(*n)
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            if values.len() >= 4 {
                                let width = values[2] - values[0];
                                let height = values[3] - values[1];
                                println!("Page {}: {:.2} x {:.2} points ({:.2} x {:.2} inches)",
                                    page_num + 1,
                                    width, height,
                                    width / 72.0, height / 72.0
                                );
                            } else {
                                println!("Page {}: Invalid MediaBox format", page_num + 1);
                            }
                        }
                        _ => {
                            println!("Page {}: MediaBox is not an array", page_num + 1);
                        }
                    }
                } else {
                    println!("Page {}: Unable to determine page size", page_num + 1);
                }
            }
            Err(e) => {
                println!("Page {}: Error - {:?}", page_num + 1, e);
            }
        }

        // Only show first 10 pages
        if page_num >= 9 {
            let remaining = page_count - 10;
            if remaining > 0 {
                println!("... and {} more page(s)", remaining);
            }
            break;
        }
    }
}

fn count_fonts(doc: &mut PDFDocument) -> usize {
    use std::collections::HashSet;
    
    let page_count = match doc.page_count() {
        Ok(count) => count,
        Err(_) => return 0,
    };
    
    let mut font_set = HashSet::new();
    
    for page_num in 0..page_count {
        if let Ok(page) = doc.get_page(page_num as usize) {
            if let Some(resources) = page.resources() {
                if let PDFObject::Dictionary(res_dict) = resources {
                    if let Some(font_obj) = res_dict.get("Font") {
                        if let Ok(font_dict) = doc.xref_mut().fetch_if_ref(font_obj) {
                            if let PDFObject::Dictionary(dict) = font_dict {
                                for (font_name, _) in dict {
                                    font_set.insert(font_name.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    font_set.len()
}
