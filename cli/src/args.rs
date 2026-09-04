//! Command-line argument parsing.

use std::env;

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
    JsonCompact,
}

/// Parsed command.
#[derive(Debug)]
pub enum Command {
    All,
    Info,
    Stats,
    Xref,
    Catalog,
    Trailer,
    Pages,
    Object(u32),
    Version,
    Fonts,
    Text {
        positions: bool,
    },
    Outline,
    Annotations,
    PageSizes,
    Images,
    Operators {
        summary: bool,
        max: Option<usize>,
    },
    Stream(u32),
    RotatePage {
        page: usize,
        angle: u16,
    },
    SetInfo {
        entries: Vec<(String, Option<String>)>,
    },
    Save,
    DeletePage {
        page: usize,
    },
    MovePage {
        page: usize,
        to: usize,
    },
    ReplaceText {
        page: usize,
        find: String,
        replace: String,
    },
    AddAnnotation {
        page: usize,
        kind: String,
        quads: Vec<[f64; 8]>,
        color: Option<[f64; 3]>,
    },
    DeleteAnnotation {
        page: usize,
        object: u32,
    },
    EditAnnotation {
        page: usize,
        object: u32,
        color: Option<[f64; 3]>,
        contents: Option<String>,
    },
    Fields,
    SetField {
        name: String,
        value: String,
    },
}

/// Parsed CLI arguments.
pub struct Args {
    pub path: String,
    pub command: Command,
    pub output: OutputMode,
    pub pages: Vec<usize>,
    pub output_path: Option<String>,
}

/// Parse `n` comma-separated floats ("100,720,300,700").
fn parse_numbers(raw: &str, n: usize) -> Result<Vec<f64>, String> {
    let values: Vec<f64> = raw
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<f64>()
                .map_err(|_| format!("invalid number: {s}"))
        })
        .collect::<Result<_, _>>()?;
    if values.len() != n {
        return Err(format!(
            "expected {n} comma-separated numbers, got {}",
            values.len()
        ));
    }
    Ok(values)
}

/// Parse one quad: 8 comma-separated numbers (pdf.js normalized order).
fn parse_quadpoints(raw: &str) -> Result<[f64; 8], String> {
    let v = parse_numbers(raw, 8)?;
    Ok([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]])
}

impl Args {
    pub fn parse() -> Result<Self, String> {
        let raw: Vec<String> = env::args().collect();
        if raw.len() < 2 {
            return Err(Self::usage());
        }
        let path = raw[1].clone();

        let mut output = OutputMode::Human;
        let mut pages: Vec<usize> = Vec::new();
        let mut command = Command::All;
        let mut output_path: Option<String> = None;

        let mut i = 2;
        while i < raw.len() {
            match raw[i].as_str() {
                "--json" => output = OutputMode::Json,
                "--compact" => output = OutputMode::JsonCompact,
                "--output" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("--output requires a file path".into());
                    }
                    output_path = Some(raw[i].clone());
                }
                "--page" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("--page requires a page number".into());
                    }
                    let p: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                    if p == 0 {
                        return Err("page numbers are 1-based".into());
                    }
                    pages.push(p - 1);
                }
                // Old flags (backward compat) and new subcommands
                "--all" => command = Command::All,
                "--catalog" => command = Command::Catalog,
                "--xref" => command = Command::Xref,
                "--trailer" => command = Command::Trailer,
                "--pages" => command = Command::Pages,
                "--version" => command = Command::Version,
                "--info" => command = Command::Info,
                "--fonts" => command = Command::Fonts,
                "--extract-text" => command = Command::Text { positions: false },
                "--outline" => command = Command::Outline,
                "--annotations" => command = Command::Annotations,
                "--stats" => command = Command::Stats,
                "--page-sizes" => command = Command::PageSizes,
                "--images" => command = Command::Images,
                "--object" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("--object requires an object number".into());
                    }
                    let n: u32 = raw[i].parse().map_err(|_| "invalid object number")?;
                    command = Command::Object(n);
                }
                // New subcommands
                "operators" => {
                    command = Command::Operators {
                        summary: false,
                        max: None,
                    }
                }
                "text" => command = Command::Text { positions: false },
                "stream" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("stream requires an object number".into());
                    }
                    let n: u32 = raw[i].parse().map_err(|_| "invalid object number")?;
                    command = Command::Stream(n);
                }
                "rotate-page" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("rotate-page requires a page number".into());
                    }
                    let p: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                    if p == 0 {
                        return Err("page numbers are 1-based".into());
                    }
                    let angle = loop {
                        i += 1;
                        if i >= raw.len() {
                            return Err("rotate-page requires --angle <degrees>".into());
                        }
                        if raw[i] == "--angle" {
                            i += 1;
                            if i >= raw.len() {
                                return Err("--angle requires a degree value".into());
                            }
                            let a: u16 = raw[i].parse().map_err(|_| "invalid angle")?;
                            if a % 90 != 0 {
                                return Err("angle must be a multiple of 90".into());
                            }
                            break a;
                        }
                    };
                    command = Command::RotatePage {
                        page: p - 1,
                        angle,
                    };
                }
                "set-info" => {
                    let mut entries = Vec::new();
                    const KEYS: &[&str] = &[
                        "title", "author", "subject", "keywords", "creator", "producer",
                        "creation-date", "mod-date",
                    ];
                    while i + 1 < raw.len() {
                        let flag = &raw[i + 1];
                        if let Some(key) = flag.strip_prefix("--") {
                            let canon = key.to_lowercase();
                            if KEYS.contains(&canon.as_str()) {
                                i += 2;
                                let val = if i < raw.len() {
                                    Some(raw[i].clone())
                                } else {
                                    return Err(format!("--{key} requires a value"));
                                };
                                let pdf_key = canon[..1].to_uppercase() + &canon[1..];
                                let pdf_key = pdf_key.replace('-', " ");
                                entries.push((pdf_key, val));
                                continue;
                            }
                        }
                        break;
                    }
                    if entries.is_empty() {
                        return Err("set-info requires at least one --key value".into());
                    }
                    command = Command::SetInfo { entries };
                }
                "delete-page" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("delete-page requires a page number".into());
                    }
                    let p: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                    if p == 0 {
                        return Err("page numbers are 1-based".into());
                    }
                    command = Command::DeletePage { page: p - 1 };
                }
                "move-page" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("move-page requires a page number".into());
                    }
                    let p: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                    if p == 0 {
                        return Err("page numbers are 1-based".into());
                    }
                    let mut to: Option<usize> = None;
                    while i + 1 < raw.len() {
                        match raw[i + 1].as_str() {
                            "--to" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--to requires a page number".into());
                                }
                                let t: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                                if t == 0 {
                                    return Err("page numbers are 1-based".into());
                                }
                                to = Some(t - 1);
                            }
                            _ => break,
                        }
                    }
                    let Some(to) = to else {
                        return Err("move-page requires --to <page>".into());
                    };
                    command = Command::MovePage { page: p - 1, to };
                }
                "replace-text" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("replace-text requires <page>".into());
                    }
                    let p: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                    if p == 0 {
                        return Err("page numbers are 1-based".into());
                    }
                    let mut find = String::new();
                    let mut replace = String::new();
                    while i + 1 < raw.len() {
                        match raw[i + 1].as_str() {
                            "--find" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--find requires a value".into());
                                }
                                find = raw[i].to_string();
                            }
                            "--replace" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--replace requires a value".into());
                                }
                                replace = raw[i].to_string();
                            }
                            _ => break,
                        }
                    }
                    if find.is_empty() {
                        return Err("replace-text requires --find <text>".into());
                    }
                    if replace.is_empty() {
                        return Err("replace-text requires --replace <text>".into());
                    }
                    command = Command::ReplaceText { page: p - 1, find, replace };
                }
                "save" => {
                    command = Command::Save;
                }
                "add-annotation" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("add-annotation requires a page number".into());
                    }
                    let p: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                    if p == 0 {
                        return Err("page numbers are 1-based".into());
                    }
                    let mut kind = String::from("highlight");
                    let mut quads: Vec<[f64; 8]> = Vec::new();
                    let mut color: Option<[f64; 3]> = None;
                    while i + 1 < raw.len() {
                        match raw[i + 1].as_str() {
                            "--kind" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--kind requires a value".into());
                                }
                                kind = raw[i].to_lowercase();
                            }
                            "--quadpoints" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err(
                                        "--quadpoints requires 8 comma-separated numbers".into()
                                    );
                                }
                                quads.push(parse_quadpoints(&raw[i])?);
                            }
                            "--rect" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--rect requires x1,y1,x2,y2".into());
                                }
                                let rect = parse_numbers(&raw[i], 4)?;
                                if rect[2] <= rect[0] || rect[3] <= rect[1] {
                                    return Err("--rect must have positive size".into());
                                }
                                // One quad in the normalized pdf.js order:
                                // top-left, top-right, bottom-left, bottom-right.
                                quads.push([
                                    rect[0], rect[3], rect[2], rect[3], rect[0], rect[1], rect[2],
                                    rect[1],
                                ]);
                            }
                            "--color" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--color requires r,g,b".into());
                                }
                                let c = parse_numbers(&raw[i], 3)?;
                                color = Some([c[0], c[1], c[2]]);
                            }
                            _ => break,
                        }
                    }
                    if quads.is_empty() {
                        return Err(
                            "add-annotation requires --rect x1,y1,x2,y2 or --quadpoints ...".into(),
                        );
                    }
                    if !matches!(
                        kind.as_str(),
                        "highlight" | "underline" | "squiggly" | "strikeout"
                    ) {
                        return Err(
                            "--kind must be highlight, underline, squiggly or strikeout".into()
                        );
                    }
                    command = Command::AddAnnotation {
                        page: p - 1,
                        kind,
                        quads,
                        color,
                    };
                }
                "delete-annotation" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("delete-annotation requires a page number".into());
                    }
                    let p: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                    if p == 0 {
                        return Err("page numbers are 1-based".into());
                    }
                    let mut object: Option<u32> = None;
                    while i + 1 < raw.len() {
                        match raw[i + 1].as_str() {
                            "--object" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--object requires an object number".into());
                                }
                                object = Some(raw[i].parse().map_err(|_| "invalid object number")?);
                            }
                            _ => break,
                        }
                    }
                    let Some(object) = object else {
                        return Err("delete-annotation requires --object <N>".into());
                    };
                    command = Command::DeleteAnnotation {
                        page: p - 1,
                        object,
                    };
                }
                "edit-annotation" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("edit-annotation requires a page number".into());
                    }
                    let p: usize = raw[i].parse().map_err(|_| "invalid page number")?;
                    if p == 0 {
                        return Err("page numbers are 1-based".into());
                    }
                    let mut object: Option<u32> = None;
                    let mut color: Option<[f64; 3]> = None;
                    let mut contents: Option<String> = None;
                    while i + 1 < raw.len() {
                        match raw[i + 1].as_str() {
                            "--object" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--object requires an object number".into());
                                }
                                object = Some(raw[i].parse().map_err(|_| "invalid object number")?);
                            }
                            "--color" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--color requires r,g,b".into());
                                }
                                let c = parse_numbers(&raw[i], 3)?;
                                color = Some([c[0], c[1], c[2]]);
                            }
                            "--contents" => {
                                i += 2;
                                if i >= raw.len() {
                                    return Err("--contents requires a value".into());
                                }
                                contents = Some(raw[i].clone());
                            }
                            _ => break,
                        }
                    }
                    let Some(object) = object else {
                        return Err("edit-annotation requires --object <N>".into());
                    };
                    command = Command::EditAnnotation {
                        page: p - 1,
                        object,
                        color,
                        contents,
                    };
                }
                "object" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("object requires an object number".into());
                    }
                    let n: u32 = raw[i].parse().map_err(|_| "invalid object number")?;
                    command = Command::Object(n);
                }
                "info" => command = Command::Info,
                "version" => command = Command::Version,
                "page-sizes" => command = Command::PageSizes,
                "stats" => command = Command::Stats,
                "xref" => command = Command::Xref,
                "catalog" => command = Command::Catalog,
                "trailer" => command = Command::Trailer,
                "pages" => command = Command::Pages,
                "fonts" => command = Command::Fonts,
                "outline" => command = Command::Outline,
                "annotations" => command = Command::Annotations,
                "images" => command = Command::Images,
                "fields" => command = Command::Fields,
                "set-field" => {
                    i += 1;
                    if i >= raw.len() || raw[i].starts_with('-') {
                        return Err("set-field requires <name> <value>".into());
                    }
                    let name = raw[i].clone();
                    i += 1;
                    if i >= raw.len() {
                        return Err("set-field requires <name> <value>".into());
                    }
                    let value = raw[i].clone();
                    command = Command::SetField { name, value };
                }
                // Subcommand flags
                "--positions" => {
                    if let Command::Text { .. } = command {
                        command = Command::Text { positions: true };
                    }
                }
                "--summary" => {
                    if let Command::Operators { .. } = command {
                        command = Command::Operators {
                            summary: true,
                            max: None,
                        };
                    }
                }
                "--max" => {
                    i += 1;
                    if i >= raw.len() {
                        return Err("--max requires a number".into());
                    }
                    let k: usize = raw[i].parse().map_err(|_| "invalid --max value")?;
                    if let Command::Operators { summary, .. } = command {
                        command = Command::Operators {
                            summary,
                            max: Some(k),
                        };
                    }
                }
                "--help" | "-h" => return Err(Self::usage()),
                other if other.starts_with('-') => {
                    return Err(format!("unknown option: {other}\n{}", Self::usage()));
                }
                _ => {}
            }
            i += 1;
        }

        Ok(Args {
            path,
            command,
            output,
            pages,
            output_path,
        })
    }

    fn usage() -> String {
        [
            "PDF Structure Inspector & Editor (pdf-x)",
            "",
            "Usage: pdf-x <file> <command> [options]",
            "",
            "Commands:",
            "  info              Document metadata and basic info",
            "  stats             Summary statistics",
            "  xref              Cross-reference table",
            "  catalog           Document catalog object",
            "  trailer           Trailer dictionary",
            "  pages             Pages dictionary",
            "  object <N>        Show specific object",
            "  text [--positions] Extract text (with positions)",
            "  operators [--summary] [--max K]  Content stream operators",
            "  stream <N>        Raw stream bytes (for byte-exact diff)",
            "  fonts             Font details (type, encoding, widths)",
            "  images            Image information",
            "  outline           Document outline (bookmarks)",
            "  annotations       Annotations",
            "  page-sizes        Page dimensions",
            "  rotate-page <P> --angle <DEG>  Rotate page (90/180/270)",
            "  set-info --title T --author A ...  Set document metadata",
            "                                    (empty value removes the key)",
            "  delete-page <P>                   Delete a page",
            "  move-page <P> --to <P>            Move a page (final position, 1-based)",
            "  replace-text <P> --find <T> --replace <R>  Replace text (v1: Tj)",
            "  add-annotation <P> --kind <K> --rect x1,y1,x2,y2 [--color r,g,b]",
            "                                     Add markup annotation (K = highlight",
            "                                     | underline | squiggly | strikeout);",
            "                                     --quadpoints q repeats for multi-quad",
            "  delete-annotation <P> --object <N>  Delete annotation by object number",
            "  edit-annotation <P> --object <N> [--color r,g,b] [--contents T]",
            "                                     Edit annotation color/contents",
            "  fields            List AcroForm fields (name/type/value/pages)",
            "  set-field <NAME> <VALUE>           Set a form field value (Text or",
            "                                     Checkbox: true/false)",
            "  save --output <path>               Save edits to file",
            "",
            "Edit commands accept --output to save in one step:",
            "  pdf-x in.pdf rotate-page 1 --angle 90 --output out.pdf",
            "  pdf-x in.pdf set-info --title 'X' --output out.pdf",
            "",
            "Options:",
            "  --json            JSON output",
            "  --compact         Single-line JSON",
            "  --output <path>   Write output file (required for save/edit)",
            "  --page <N>        Filter to specific page(s) (1-based)",
            "",
            "Legacy flags still work: --all --catalog --xref --trailer --pages",
            "  --object N --version --info --fonts --extract-text --outline",
            "  --annotations --stats --page-sizes --images",
        ]
        .join("\n")
    }
}
