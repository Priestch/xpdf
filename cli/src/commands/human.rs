//! Human-readable output rendering.
//!
//! Each command returns plain JSON data; this module turns that data into the
//! terminal output used when no `--json`/`--compact` flag is given.

use serde_json::Value;

/// Print command data in human-readable form.
pub fn print(command: &str, data: &Value) {
    match command {
        "info" | "all" => print_info(data),
        "stats" => print_stats(data),
        "version" => print_version(data),
        "page-sizes" => print_page_sizes(data),
        "xref" => print_xref(data),
        "fonts" => print_fonts(data),
        "text" => print_text(data),
        "outline" => print_outline(data),
        "annotations" => print_annotations(data),
        "images" => print_images(data),
        "operators" => print_operators(data),
        "stream" => print_stream(data),
        // catalog / trailer / pages / object: raw object dumps
        _ => print_object_data(data),
    }
}

fn print_info(data: &Value) {
    println!("Page Count: {}", data["page_count"].as_u64().unwrap_or(0));
    match &data["pdf_version"] {
        Value::String(v) => println!("PDF Version: {v}"),
        _ => println!("PDF Version: Unknown"),
    }
    if let Some(size) = data["file_size"].as_u64() {
        println!("File Size: {}", format_size(size));
    }
    println!(
        "Linearized: {}",
        if data["linearized"].as_bool().unwrap_or(false) {
            "Yes (optimized for web viewing)"
        } else {
            "No"
        }
    );
    println!(
        "Encrypted: {}",
        if data["encrypted"].as_bool().unwrap_or(false) {
            "Yes"
        } else {
            "No"
        }
    );
    match &data["document_info"] {
        Value::Object(info) if !info.is_empty() => {
            println!("\nDocument Info:");
            for (key, value) in info {
                println!("  {key}: {value}");
            }
        }
        _ => println!("\nNo document info dictionary found"),
    }
}

fn print_stats(data: &Value) {
    println!("Page Count: {}", data["page_count"].as_u64().unwrap_or(0));
    println!(
        "XRef Entries: {}",
        data["xref_entries"].as_u64().unwrap_or(0)
    );
    if let Some(size) = data["file_size"].as_u64() {
        println!("File Size: {}", format_size(size));
    }
    if let Some(avg) = data["avg_page_size"].as_f64() {
        println!("Avg Page Size: {}", format_size(avg as u64));
    }
    println!(
        "Linearized: {}",
        if data["linearized"].as_bool().unwrap_or(false) {
            "Yes"
        } else {
            "No"
        }
    );
    match &data["pdf_version"] {
        Value::String(v) => println!("PDF Version: {v}"),
        _ => println!("PDF Version: Unknown"),
    }
    if let Some(fonts) = data["unique_fonts"].as_u64() {
        if fonts > 0 {
            println!("Unique Fonts: {fonts}");
        }
    }
}

fn print_version(data: &Value) {
    match &data["pdf_version"] {
        Value::String(v) => println!("PDF Version: {v}"),
        _ => println!("PDF Version: Unknown"),
    }
}

fn print_page_sizes(data: &Value) {
    if let Value::Array(pages) = &data["pages"] {
        for page in pages {
            let num = page["page"].as_u64().unwrap_or(0);
            match (page["width"].as_f64(), page["height"].as_f64()) {
                (Some(w), Some(h)) => println!(
                    "Page {num}: {w:.2} x {h:.2} points ({:.2} x {:.2} inches)",
                    w / 72.0,
                    h / 72.0
                ),
                _ => println!("Page {num}: Unable to determine page size"),
            }
        }
    }
}

fn print_xref(data: &Value) {
    println!("Total entries: {}\n", data["total"].as_u64().unwrap_or(0));
    println!(
        "{:<8} {:<12} {:<14} {:<8}",
        "Object", "Type", "Offset/Ref", "Gen"
    );
    println!("{}", "─".repeat(50));

    if let Value::Array(entries) = &data["entries"] {
        for entry in entries {
            let num = entry["num"].as_u64().unwrap_or(0);
            match entry["type"].as_str().unwrap_or("?") {
                "free" => println!(
                    "{:<8} {:<12} {:<14} {:<8}",
                    num,
                    "free",
                    entry["next_free"].as_u64().unwrap_or(0),
                    entry["generation"].as_u64().unwrap_or(0)
                ),
                "uncompressed" => println!(
                    "{:<8} {:<12} {:<14} {:<8}",
                    num,
                    "uncompressed",
                    entry["offset"].as_u64().unwrap_or(0),
                    entry["generation"].as_u64().unwrap_or(0)
                ),
                "compressed" => println!(
                    "{:<8} {:<12} {:<14} {:<8}",
                    num,
                    "compressed",
                    format!(
                        "{}[{}]",
                        entry["obj_stream_num"].as_u64().unwrap_or(0),
                        entry["index"].as_u64().unwrap_or(0)
                    ),
                    "0"
                ),
                other => println!("{num:<8} {other:<12}"),
            }
        }
    }
}

fn print_fonts(data: &Value) {
    if let Value::Array(pages) = &data["pages"] {
        for page in pages {
            let num = page["page"].as_u64().unwrap_or(0);
            match &page["fonts"] {
                Value::Array(fonts) if !fonts.is_empty() => {
                    println!("Page {num}:");
                    for font in fonts {
                        let name = font["name"].as_str().unwrap_or("?");
                        if let Some(err) = font["error"].as_str() {
                            println!("  /{name}: error - {err}");
                            continue;
                        }
                        println!(
                            "  /{}: {} (base: {}, ToUnicode: {}, embedded: {})",
                            name,
                            font["font_type"].as_str().unwrap_or("?"),
                            font["base_font"].as_str().unwrap_or("?"),
                            yes_no(font["has_to_unicode"].as_bool()),
                            yes_no(font["has_embedded_font"].as_bool()),
                        );
                    }
                }
                _ => println!("Page {num}: no fonts"),
            }
        }
    }
}

fn print_text(data: &Value) {
    if let Value::Array(pages) = &data["pages"] {
        for page in pages {
            let num = page["page"].as_u64().unwrap_or(0);
            println!("═══ Page {num} ═══");
            if let Value::Array(items) = &page["items"] {
                for item in items {
                    let mut line = format!("  {}", item["text"].as_str().unwrap_or(""));
                    if let Some(font) = item["font_name"].as_str() {
                        line.push_str(&format!(" [{font}"));
                        if let Some(size) = item["font_size"].as_f64() {
                            line.push_str(&format!(" {size:.1}pt"));
                        }
                        line.push(']');
                    }
                    if let Value::Array(pos) = &item["position"] {
                        if pos.len() == 2 {
                            line.push_str(&format!(
                                " @ ({:.1}, {:.1})",
                                pos[0].as_f64().unwrap_or(0.0),
                                pos[1].as_f64().unwrap_or(0.0)
                            ));
                        }
                    }
                    println!("{line}");
                }
            } else if let Some(text) = page["text"].as_str() {
                if text.is_empty() {
                    println!("  (No text on this page)");
                } else {
                    println!("  {text}");
                }
            }
            println!();
        }
    }
    if let Some(total) = data["total_characters"].as_u64() {
        println!("Total characters extracted: {total}");
    }
}

fn print_outline(data: &Value) {
    if let Value::Array(items) = &data["items"] {
        if items.is_empty() {
            println!("No document outline found");
            return;
        }
        for item in items {
            print_outline_item(item, 0);
        }
    }
}

fn print_outline_item(item: &Value, indent: usize) {
    let indent_str = "  ".repeat(indent);
    let mut title = item["title"].as_str().unwrap_or("").to_string();
    if item["bold"].as_bool().unwrap_or(false) {
        title = format!("[B] {title}");
    }
    if item["italic"].as_bool().unwrap_or(false) {
        title = format!("[I] {title}");
    }
    println!("{indent_str}{title}");

    if let Value::Object(dest) = &item["dest"] {
        match dest["type"].as_str().unwrap_or("?") {
            "explicit" => println!(
                "{}  → Page {} ({})",
                indent_str,
                dest["page"].as_u64().unwrap_or(0),
                dest["dest_type"].as_str().unwrap_or("?")
            ),
            "named" => println!(
                "{}  → Named: {}",
                indent_str,
                dest["name"].as_str().unwrap_or("?")
            ),
            "url" => println!(
                "{}  → URL: {}",
                indent_str,
                dest["url"].as_str().unwrap_or("?")
            ),
            "goto_remote" => println!(
                "{}  → Remote PDF: {}",
                indent_str,
                dest["url"].as_str().unwrap_or("?")
            ),
            _ => {}
        }
    }

    if let Value::Array(color) = &item["color"] {
        if color.len() == 3 {
            println!(
                "{}  Color: #{:02x}{:02x}{:02x}",
                indent_str,
                color[0].as_u64().unwrap_or(0),
                color[1].as_u64().unwrap_or(0),
                color[2].as_u64().unwrap_or(0)
            );
        }
    }

    if let Some(count) = item["count"].as_i64() {
        if count < 0 {
            println!("{}  [{} descendants, closed]", indent_str, count.abs());
        } else {
            println!("{indent_str}  [{count} descendants, open]");
        }
    }

    if let Value::Array(children) = &item["children"] {
        for child in children {
            print_outline_item(child, indent + 1);
        }
    }
}

fn print_annotations(data: &Value) {
    let mut printed_any = false;
    if let Value::Array(pages) = &data["pages"] {
        for page in pages {
            let Value::Array(annots) = &page["annotations"] else {
                continue;
            };
            if annots.is_empty() {
                continue;
            }
            printed_any = true;
            let num = page["page"].as_u64().unwrap_or(0);
            println!("Page {num} ({} annotation(s)):", annots.len());
            for (i, annot) in annots.iter().enumerate() {
                println!("  [{}] {}", i + 1, annot["type"].as_str().unwrap_or("?"));
                if let Value::Array(rect) = &annot["rect"] {
                    if rect.len() == 4 {
                        println!(
                            "      Location: [{}, {}, {}, {}]",
                            rect[0], rect[1], rect[2], rect[3]
                        );
                    }
                }
                if let Some(contents) = annot["contents"].as_str() {
                    if !contents.is_empty() {
                        let preview = truncate(contents, 50);
                        println!("      Contents: {preview}");
                    }
                }
                if let Value::Object(link) = &annot["data"] {
                    match link.get("action").and_then(|v| v.as_str()).unwrap_or("") {
                        "uri" => println!(
                            "      → URL: {}",
                            link.get("url").and_then(|v| v.as_str()).unwrap_or("?")
                        ),
                        "goto" => println!(
                            "      → Page {} ({})",
                            link.get("page").and_then(|v| v.as_u64()).unwrap_or(0),
                            link.get("dest").and_then(|v| v.as_str()).unwrap_or("?")
                        ),
                        "goto_named" => println!(
                            "      → Named destination: {}",
                            link.get("name").and_then(|v| v.as_str()).unwrap_or("?")
                        ),
                        _ => {}
                    }
                }
                if let Value::Array(color) = &annot["color"] {
                    if color.len() >= 3 {
                        println!(
                            "      Color: RGB({:.0}, {:.0}, {:.0})",
                            color[0].as_f64().unwrap_or(0.0) * 255.0,
                            color[1].as_f64().unwrap_or(0.0) * 255.0,
                            color[2].as_f64().unwrap_or(0.0) * 255.0
                        );
                    }
                }
            }
            println!();
        }
    }
    if !printed_any {
        println!("No annotations found");
    }
}

fn print_images(data: &Value) {
    if let Value::Array(pages) = &data["pages"] {
        for page in pages {
            let num = page["page"].as_u64().unwrap_or(0);
            match &page["images"] {
                Value::Array(images) if !images.is_empty() => {
                    println!("Page {num}:");
                    for img in images {
                        let alpha = if img["has_alpha"].as_bool().unwrap_or(false) {
                            ", alpha"
                        } else {
                            ""
                        };
                        let color_space =
                            format!("{}{}", img["color_space"].as_str().unwrap_or("?"), alpha);
                        println!(
                            "  {}: {} {}x{}, {} bpc, color space {}",
                            img["name"].as_str().unwrap_or("?"),
                            img["format"].as_str().unwrap_or("?"),
                            img["width"].as_u64().unwrap_or(0),
                            img["height"].as_u64().unwrap_or(0),
                            img["bits_per_component"].as_u64().unwrap_or(0),
                            color_space,
                        );
                        if let Some(len) = img["data_length"].as_u64() {
                            println!("      Data size: {}", format_size(len));
                        }
                    }
                }
                _ => println!("Page {num}: no images"),
            }
        }
    }
    if let Some(total) = data["total"].as_u64() {
        println!("\nTotal images found: {total}");
    }
}

fn print_operators(data: &Value) {
    if let Value::Array(summary) = &data["summary"] {
        for entry in summary {
            println!(
                "{:<12} {}",
                entry["op"].as_str().unwrap_or("?"),
                entry["count"]
            );
        }
    } else if let Value::Array(pages) = &data["pages"] {
        for page in pages {
            let num = page["page"].as_u64().unwrap_or(0);
            let count = page["operation_count"].as_u64().unwrap_or(0);
            println!("═══ Page {num} ({count} operations) ═══");
            if let Value::Array(ops) = &page["operations"] {
                for op in ops {
                    let args: Vec<String> = match &op["args"] {
                        Value::Array(args) => args.iter().map(render_scalar).collect(),
                        _ => Vec::new(),
                    };
                    println!("  {} {}", args.join(" "), op["op"].as_str().unwrap_or("?"));
                }
            }
            println!();
        }
    }
    if let Some(total) = data["total_operations"].as_u64() {
        println!("Total operations: {total}");
    }
}

fn print_stream(data: &Value) {
    println!("Object {} 0", data["object"].as_u64().unwrap_or(0));
    println!("Length: {} bytes", data["length"].as_u64().unwrap_or(0));
    println!("Dictionary:");
    print_value(&data["dict"], 1);
    if let Some(hex) = data["bytes_hex"].as_str() {
        let preview: String = hex.chars().take(96).collect();
        if hex.len() > 96 {
            println!("Bytes (hex, truncated): {preview}...");
        } else {
            println!("Bytes (hex): {preview}");
        }
    }
}

fn print_object_data(data: &Value) {
    // `object` command output: render the ref as a header line.
    if let Some(num) = data["object"].as_u64() {
        println!(
            "Object {num} {} R",
            data["generation"].as_u64().unwrap_or(0)
        );
        for (key, value) in data.as_object().into_iter().flatten() {
            if key == "object" || key == "generation" {
                continue;
            }
            println!("{key}:");
            print_value(value, 1);
        }
        return;
    }
    for (key, value) in data.as_object().into_iter().flatten() {
        println!("{key}:");
        print_value(value, 1);
    }
}

/// Recursively print a JSON-encoded PDF object.
fn print_value(value: &Value, indent: usize) {
    let indent_str = "  ".repeat(indent);
    match value {
        Value::Array(items) if !items.is_empty() && !items.iter().all(is_inline) => {
            println!("{indent_str}[");
            for item in items {
                print_value(item, indent + 1);
            }
            println!("{indent_str}]");
        }
        Value::Object(map) if !map.is_empty() && !is_wrapper(value) => {
            println!("{indent_str}<<");
            for (key, item) in map {
                if is_inline(item) {
                    println!("{indent_str}  /{key}: {}", render_scalar(item));
                } else {
                    println!("{indent_str}  /{key}:");
                    print_value(item, indent + 2);
                }
            }
            println!("{indent_str}>>");
        }
        other => println!("{indent_str}{}", render_scalar(other)),
    }
}

/// A value that fits on a single line: a JSON scalar or one of the
/// `pdf_object_to_value` single-key wrapper objects.
fn is_inline(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => true,
        Value::Array(items) => items.iter().all(is_inline),
        Value::Object(_) => is_wrapper(value),
    }
}

/// Is this a `pdf_object_to_value` wrapper object (`{"name": ..}` etc.)?
fn is_wrapper(value: &Value) -> bool {
    let Value::Object(map) = value else {
        return false;
    };
    if map.len() != 1 {
        return false;
    }
    matches!(
        map.keys().next().expect("single-entry object").as_str(),
        "string" | "name" | "hex" | "op" | "ref" | "inline_image" | "stream"
    )
}

/// Render a scalar value on one line, honoring the pdf_object_to_value
/// conventions (`{"string": ..}`, `{"name": ..}`, `{"hex": ..}`, `{"ref": ..}`).
fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(s) => truncate(s, 60),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(render_scalar)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) if map.len() == 1 => {
            let (key, inner) = map.iter().next().expect("single-entry object");
            match (key.as_str(), inner) {
                ("string", Value::String(s)) => format!("({})", truncate(s, 60)),
                ("name", Value::String(s)) => format!("/{s}"),
                ("hex", Value::String(s)) => format!("<{}>", truncate(s, 60)),
                ("op", Value::String(s)) => s.clone(),
                ("ref", Value::Array(r)) if r.len() == 2 => format!("{} {} R", r[0], r[1]),
                ("inline_image", Value::Object(img)) => format!(
                    "inline image data ({} bytes)",
                    img.get("length").and_then(Value::as_u64).unwrap_or(0)
                ),
                _ => value.to_string(),
            }
        }
        _ => value.to_string(),
    }
}

fn yes_no(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "yes",
        _ => "no",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        // Split at a char boundary to avoid panicking on multi-byte text.
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
