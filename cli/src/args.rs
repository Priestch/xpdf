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
    Text { positions: bool },
    Outline,
    Annotations,
    PageSizes,
    Images,
    Operators { summary: bool, max: Option<usize> },
    Stream(u32),
    RotatePage { page: usize, angle: u16 },
    SetInfo { entries: Vec<(String, Option<String>)> },
    Save,
    DeletePage { page: usize },
    MovePage { page: usize, to: usize },
    ReplaceText { page: usize, find: String, replace: String },
}

/// Parsed CLI arguments.
pub struct Args {
    pub path: String,
    pub command: Command,
    pub output: OutputMode,
    pub pages: Vec<usize>,
    pub output_path: Option<String>,
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
