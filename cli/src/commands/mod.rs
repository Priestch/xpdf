//! Command implementations.

pub mod annotations;
pub mod fonts;
pub mod images;
pub mod info;
pub mod operators;
pub mod outline;
pub mod structure;
pub mod text;

use crate::args::{Command, OutputMode};
use pdf_x_core::PDFDocument;
use serde_json::Value;

/// Shared context for command execution.
pub struct Ctx<'a> {
    pub doc: &'a mut PDFDocument,
    pub path: &'a str,
    pub file_size: Option<u64>,
    pub pages: Vec<usize>, // 0-based page indices
    pub output: OutputMode,
}

impl Ctx<'_> {
    pub fn page_indices(&mut self) -> Vec<usize> {
        if self.pages.is_empty() {
            (0..self.doc.page_count().unwrap_or(0))
                .map(|p| p as usize)
                .collect()
        } else {
            self.pages.clone()
        }
    }

    pub fn print(&self, command: &str, data: Value) {
        match self.output {
            OutputMode::Human => human::print(command, &data),
            OutputMode::Json => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&crate::jsonutil::envelope(
                        command, self.path, data,
                    ))
                    .unwrap_or_default()
                );
            }
            OutputMode::JsonCompact => {
                println!(
                    "{}",
                    serde_json::to_string(&crate::jsonutil::envelope(command, self.path, data,))
                        .unwrap_or_default()
                );
            }
        }
    }

    pub fn print_error(&self, command: &str, msg: &str) {
        if self.output != OutputMode::Human {
            println!(
                "{}",
                serde_json::to_string(&crate::jsonutil::error(command, self.path, msg))
                    .unwrap_or_default()
            );
        }
        eprintln!("Error: {msg}");
    }
}

/// Dispatch a parsed command.
pub fn run(ctx: &mut Ctx, command: &Command) {
    use Command::*;
    let result: Result<Value, String> = match command {
        All | Info => info::run(ctx),
        Stats => info::run_stats(ctx),
        Xref => structure::run_xref(ctx),
        Catalog => structure::run_catalog(ctx),
        Trailer => structure::run_trailer(ctx),
        Pages => structure::run_pages(ctx),
        Object(n) => structure::run_object(ctx, *n),
        Version => info::run_version(ctx),
        Fonts => fonts::run(ctx),
        Text { positions } => text::run(ctx, *positions),
        Outline => outline::run(ctx),
        Annotations => annotations::run(ctx),
        PageSizes => info::run_page_sizes(ctx),
        Images => images::run(ctx),
        Operators { summary, max } => operators::run(ctx, *summary, *max),
        Stream(n) => structure::run_stream(ctx, *n),
    };

    let cmd_name = match command {
        All => "all",
        Info => "info",
        Stats => "stats",
        Xref => "xref",
        Catalog => "catalog",
        Trailer => "trailer",
        Pages => "pages",
        Object(_) => "object",
        Version => "version",
        Fonts => "fonts",
        Text { .. } => "text",
        Outline => "outline",
        Annotations => "annotations",
        PageSizes => "page-sizes",
        Images => "images",
        Operators { .. } => "operators",
        Stream(_) => "stream",
    };

    match result {
        Ok(data) => ctx.print(cmd_name, data),
        Err(msg) => ctx.print_error(cmd_name, &msg),
    }
}

mod human;
