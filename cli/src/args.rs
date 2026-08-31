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
}

/// Parsed CLI arguments.
pub struct Args {
    pub path: String,
    pub command: Command,
    pub output: OutputMode,
    pub pages: Vec<usize>,
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

        let mut i = 2;
        while i < raw.len() {
            match raw[i].as_str() {
                "--json" => output = OutputMode::Json,
                "--compact" => output = OutputMode::JsonCompact,
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
        })
    }

    fn usage() -> String {
        [
            "PDF Structure Inspector (pdf-x)",
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
            "",
            "Options:",
            "  --json            JSON output",
            "  --compact         Single-line JSON",
            "  --page <N>        Filter to specific page(s) (1-based)",
            "",
            "Legacy flags still work: --all --catalog --xref --trailer --pages",
            "  --object N --version --info --fonts --extract-text --outline",
            "  --annotations --stats --page-sizes --images",
        ]
        .join("\n")
    }
}
