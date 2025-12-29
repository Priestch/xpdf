use pdf_x::{PDFDocument, PDFObject, XRefEntry};
use pdf_x::core::{ImageDecoder, ImageFormat, Page};
use pdf_x::core::decode::{decode_flate, decode_png_predictor};
use std::env;
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
        eprintln!("  --images         Extract and show image information");
        eprintln!("  --object <num>   Show specific object by number");
        eprintln!("  --version        Show PDF version");
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
    let show_images = args.contains(&"--images".to_string());

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
