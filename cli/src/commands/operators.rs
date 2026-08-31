//! `operators` command: content-stream operator listing/summary.

use crate::commands::Ctx;
use crate::jsonutil::pdf_object_to_value;
use pdf_x_core::core::base_stream::BaseStream;
use pdf_x_core::core::content_stream::ContentStreamEvaluator;
use pdf_x_core::core::lexer::Lexer;
use pdf_x_core::core::parser::Parser;
use pdf_x_core::core::stream::Stream;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// `operators [--summary] [--max K]`: parse the content streams of the
/// selected pages (via the resolve-then-interpret boundary) and either list
/// operations or summarize operator frequencies.
pub fn run(ctx: &mut Ctx, summary: bool, max: Option<usize>) -> Result<Value, String> {
    let indices = ctx.page_indices();
    let doc = &mut *ctx.doc;

    let mut pages = Vec::new();
    let mut total_ops = 0usize;
    let mut freq: BTreeMap<String, usize> = BTreeMap::new();

    for idx in indices {
        let resolved = doc
            .resolve_page(idx)
            .map_err(|e| format!("page {} resolution failed: {e:?}", idx + 1))?;

        let stream = Stream::from_bytes(resolved.contents().to_vec());
        let stream: Box<dyn BaseStream> = Box::new(stream);
        let lexer = Lexer::new(stream).map_err(|e| format!("page {idx} lexer error: {e:?}"))?;
        let parser = Parser::new(lexer).map_err(|e| format!("page {idx} parser error: {e:?}"))?;
        let mut evaluator = ContentStreamEvaluator::new(parser);

        let mut ops = Vec::new();
        let mut page_op_count = 0usize;
        loop {
            let op = evaluator
                .read_operation()
                .map_err(|e| format!("page {idx} content stream error: {e:?}"))?;
            let Some(op) = op else { break };

            let command = op.op.to_command().to_string();
            *freq.entry(command.clone()).or_insert(0) += 1;
            page_op_count += 1;
            total_ops += 1;

            if !summary {
                let limit_hit = max.is_some_and(|k| ops.len() >= k);
                if !limit_hit {
                    ops.push(json!({
                        "op": command,
                        "args": op.args.iter().map(pdf_object_to_value).collect::<Vec<_>>(),
                    }));
                }
            }
        }

        pages.push(json!({
            "page": idx + 1,
            "operation_count": page_op_count,
            "operations": ops,
        }));
    }

    if summary {
        let mut counts: Vec<(String, usize)> = freq.into_iter().collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        if let Some(k) = max {
            counts.truncate(k);
        }
        let summary_json: Vec<Value> = counts
            .into_iter()
            .map(|(op, count)| json!({ "op": op, "count": count }))
            .collect();
        Ok(json!({
            "pages": pages,
            "total_operations": total_ops,
            "summary": summary_json,
        }))
    } else {
        Ok(json!({
            "pages": pages,
            "total_operations": total_ops,
        }))
    }
}
