use std::io::{self, Write};

use anyhow::Error;
use serde::Serialize;
use serde_json::{json, Value};

use crate::cli::OutputFormat;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reliability {
    pub level: &'static str,
    pub source: &'static str,
    pub exact: bool,
    pub llm_instruction: &'static str,
}

pub fn source_fact() -> Reliability {
    Reliability {
        level: "source_fact",
        source: "text_path_git_filesystem",
        exact: true,
        llm_instruction: "这些结果是可验证源码事实。修改前仍应使用 code-search read 读取精确范围。",
    }
}

pub fn parser_fact() -> Reliability {
    Reliability {
        level: "parser_fact",
        source: "tree_sitter_ast",
        exact: false,
        llm_instruction:
            "这些结果是 parser fact，不能等同于 precise semantic reference resolution。",
    }
}

pub fn inferred_candidate() -> Reliability {
    Reliability {
        level: "inferred_candidate",
        source: "tree_sitter_ast_heuristic",
        exact: false,
        llm_instruction:
            "这些结果只能作为候选关系，不是完整调用图。推理前必须用 code-search read 验证每个匹配。",
    }
}

pub fn freshness() -> Reliability {
    Reliability {
        level: "freshness",
        source: "index_manifest_git_status",
        exact: false,
        llm_instruction: "这些结果描述缓存新鲜度和 watcher 状态，不提升代码事实准确性。",
    }
}

pub fn response(
    command: &str,
    canonical_command: &str,
    query: Value,
    snapshot_id: &str,
    reliability: Reliability,
    results: Value,
    warnings: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "command": command,
        "canonicalCommand": canonical_command,
        "query": query,
        "snapshot_id": snapshot_id,
        "reliability": reliability,
        "index": {
            "used": false,
            "fresh": false,
            "fallback": true,
            "reason": "live_scan"
        },
        "results": results,
        "warnings": warnings
    })
}

pub fn error_response(error: Error) -> Value {
    json!({
        "ok": false,
        "error": {
            "message": error.to_string()
        }
    })
}

pub fn emit(format: &OutputFormat, value: &Value) -> io::Result<()> {
    match format {
        OutputFormat::Json => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer_pretty(&mut handle, value)?;
            writeln!(handle)?;
        }
        OutputFormat::Text => {
            let mut handle = io::stdout().lock();
            render_text(value, &mut handle)?;
        }
    }
    Ok(())
}

fn render_text(value: &Value, out: &mut dyn Write) -> io::Result<()> {
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        writeln!(
            out,
            "error: {}",
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        )?;
        return Ok(());
    }

    if let Some(results) = value.get("results").and_then(Value::as_array) {
        for result in results {
            if let Some(path) = result.get("path").and_then(Value::as_str) {
                if let Some(range) = result.get("range") {
                    let line = range
                        .pointer("/start/line")
                        .and_then(Value::as_u64)
                        .unwrap_or(1);
                    writeln!(out, "{path}:{line}")?;
                } else {
                    writeln!(out, "{path}")?;
                }
            } else {
                writeln!(out, "{result}")?;
            }
        }
        return Ok(());
    }

    writeln!(out, "{value}")?;
    Ok(())
}

pub fn no_match_exit(results: &Value) -> i32 {
    match results.as_array() {
        Some(values) if values.is_empty() => 2,
        _ => 0,
    }
}
