//! pdf-x CLI entry point: parse args, open the document, dispatch a command.

mod args;
mod commands;
mod jsonutil;

use args::Args;
use commands::Ctx;
use std::path::Path;
use std::process;

fn main() {
    let args = match Args::parse() {
        Ok(args) => args,
        Err(usage) => {
            eprintln!("{usage}");
            process::exit(1);
        }
    };

    if !Path::new(&args.path).exists() {
        eprintln!("Error: File not found: {}", args.path);
        process::exit(1);
    }

    // Read original bytes up-front for incremental save.
    let original_bytes = match std::fs::read(&args.path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading file: {e}");
            process::exit(1);
        }
    };

    // Progressive/chunked load: 64KB fetch windows driven by the source shell.
    let mut doc = match pdf_x_source_file::open_file(&args.path, None, None) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("Error parsing PDF: {e:?}");
            process::exit(1);
        }
    };

    let file_size = Some(original_bytes.len() as u64);

    let mut ctx = Ctx {
        doc: &mut doc,
        path: &args.path,
        original_bytes: &original_bytes,
        file_size,
        pages: args.pages,
        output: args.output,
        output_path: args.output_path,
    };

    commands::run(&mut ctx, &args.command);
}
