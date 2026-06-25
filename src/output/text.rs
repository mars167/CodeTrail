mod status;

use std::io::{self, Write};

use serde_json::Value;

use status::{is_status_like, render_text_status_like};

pub(super) fn render_text(value: &Value, out: &mut dyn Write) -> io::Result<()> {
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        let mut lines = message.lines();
        let first = lines.next().unwrap_or("unknown error").trim();
        writeln!(out, "error: {first}")?;
        for line in lines {
            let line = line.trim();
            if line.starts_with("caused by:") {
                writeln!(out, "  {line}")?;
            }
        }
        return Ok(());
    }

    if value.pointer("/guard/triggered").and_then(Value::as_bool) == Some(true) {
        let reason = value
            .pointer("/guard/reason")
            .and_then(Value::as_str)
            .unwrap_or("broad_query");
        let suppressed = value
            .pointer("/guard/suppressedResults")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        writeln!(
            out,
            "warning: broad query guard triggered ({reason}); suppressed {suppressed} results"
        )?;
        render_text_summary(value, out)?;
        render_text_results(value, out)?;
        return Ok(());
    }

    if value.get("noMatch").is_some() {
        let command = value
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("query");
        writeln!(out, "no matches for {command}")?;
        return Ok(());
    }

    if value
        .pointer("/ambiguity/triggered")
        .and_then(Value::as_bool)
        == Some(true)
    {
        let count = value
            .pointer("/ambiguity/candidateCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let result_count = value
            .get("results")
            .and_then(Value::as_array)
            .map(|results| results.len() as u64)
            .unwrap_or(0);
        let count = count.max(result_count);
        writeln!(out, "ambiguous results: {count} candidates")?;
        render_text_facets(value.pointer("/ambiguity/groups/kind"), out, "kinds")?;
        render_text_facets(value.pointer("/ambiguity/groups/topDir"), out, "top dirs")?;
    }

    render_text_results(value, out)?;
    render_text_page_hint(value, out)?;
    Ok(())
}

fn render_text_results(value: &Value, out: &mut dyn Write) -> io::Result<()> {
    if let Some(results) = value.get("results").and_then(Value::as_array) {
        let command = value.get("command").and_then(Value::as_str).unwrap_or("");
        if command == "call-hierarchy" {
            return render_text_call_hierarchy(value, results, out);
        }
        if matches!(command, "calls" | "callers") {
            return render_text_graph(value, results, out);
        }
        if command == "routes" {
            return render_text_routes(results, out);
        }
        if command == "read" {
            return render_text_read(results, out);
        }
        if is_status_like(command) {
            return render_text_status_like(command, results, out);
        }
        for result in results {
            render_text_result(result, out)?;
        }
        return Ok(());
    }

    writeln!(out, "{value}")?;
    Ok(())
}

fn render_text_routes(results: &[Value], out: &mut dyn Write) -> io::Result<()> {
    for result in results {
        let method = result
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("ANY");
        let route = result
            .get("routePattern")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let path = result.get("path").and_then(Value::as_str).unwrap_or("");
        let location = if path.is_empty() {
            String::new()
        } else {
            format_location(path, result.get("range"))
        };
        let mut details = Vec::new();
        if let Some(framework) = result.get("framework").and_then(Value::as_str) {
            details.push(framework.to_string());
        }
        if let Some(handler) = result.get("handler").and_then(Value::as_str) {
            details.push(format!("handler={handler}"));
        }
        let suffix = match (location.is_empty(), details.is_empty()) {
            (true, true) => String::new(),
            (false, true) => format!("  {location}"),
            (true, false) => format!("  {}", details.join(" ")),
            (false, false) => format!("  {location}  {}", details.join(" ")),
        };
        writeln!(out, "{method:<7} {route}{suffix}")?;
    }
    Ok(())
}

fn render_text_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    if let Some(path) = result.get("path").and_then(Value::as_str) {
        let location = format_location(path, result.get("range"));
        if result.get("name").is_some() || result.get("symbolName").is_some() {
            let name = result_symbol_label(result);
            let kind = result
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("symbol");
            writeln!(out, "{kind:<12} {name}  {location}")?;
            render_text_source_context(result, out)?;
            render_text_relation_summary(result, out)?;
            return Ok(());
        }
        if let Some(preview) = result.get("preview").and_then(Value::as_str) {
            writeln!(out, "{location}  {}", preview.trim())?;
            return Ok(());
        }
        writeln!(out, "{location}")?;
        return Ok(());
    }

    if let Some(path) = result.get("file").and_then(Value::as_str) {
        writeln!(out, "{path}")?;
        return Ok(());
    }

    writeln!(out, "{}", one_line_json(result))?;
    Ok(())
}

fn render_text_read(results: &[Value], out: &mut dyn Write) -> io::Result<()> {
    for (idx, result) in results.iter().enumerate() {
        if idx > 0 {
            writeln!(out)?;
        }
        let path = result.get("path").and_then(Value::as_str).unwrap_or("read");
        if result.get("binary").and_then(Value::as_bool) == Some(true) {
            writeln!(out, "{path}: binary file not displayed")?;
            continue;
        }
        if let Some(content) = result.get("content").and_then(Value::as_str) {
            write!(out, "{content}")?;
            if !content.ends_with('\n') {
                writeln!(out)?;
            }
        } else {
            writeln!(out, "{}", format_location(path, result.get("range")))?;
        }
    }
    Ok(())
}

fn render_text_graph(value: &Value, results: &[Value], out: &mut dyn Write) -> io::Result<()> {
    let command = value
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("calls");
    let identifier = value
        .pointer("/query/identifier")
        .and_then(Value::as_str)
        .unwrap_or("symbol");
    let title = if command == "callers" {
        format!("Callers of \"{identifier}\" ({})", results.len())
    } else {
        format!("Callees of \"{identifier}\" ({})", results.len())
    };
    writeln!(out, "{title}")?;
    if results.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    for result in results {
        let caller = first_string(
            result,
            &[
                "enclosingSymbolSignature",
                "enclosingSymbolDetail",
                "enclosingSymbol",
            ],
        )
        .map(display_graph_symbol)
        .unwrap_or_else(|| identifier.to_string());
        let callee = first_string(result, &["targetSignature", "targetDetail", "target"])
            .map(display_graph_symbol)
            .unwrap_or_else(|| identifier.to_string());
        let path = result.get("path").and_then(Value::as_str).unwrap_or("");
        let location = if path.is_empty() {
            String::new()
        } else {
            format_location(path, result.get("range"))
        };
        if location.is_empty() {
            writeln!(out, "{caller} -> {callee}")?;
        } else {
            writeln!(out, "{caller} -> {callee}  {location}")?;
        }
    }
    Ok(())
}

fn render_text_call_hierarchy(
    value: &Value,
    results: &[Value],
    out: &mut dyn Write,
) -> io::Result<()> {
    let identifier = value
        .pointer("/query/identifier")
        .and_then(Value::as_str)
        .unwrap_or("symbol");
    writeln!(
        out,
        "Call hierarchy for \"{identifier}\" ({})",
        results.len()
    )?;
    if results.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    for (idx, result) in results.iter().enumerate() {
        if idx > 0 {
            writeln!(out)?;
        }
        let root = result.get("root").unwrap_or(&Value::Null);
        let root_name = item_label(root).unwrap_or_else(|| identifier.to_string());
        let root_path = item_path(root).unwrap_or("");
        let root_location = item_location(root);
        if root_location.is_empty() {
            writeln!(out, "{root_name}")?;
        } else {
            writeln!(out, "{root_name}  {root_location}")?;
        }

        let mut rendered_section = false;
        if let Some(incoming) = result.get("incomingCalls").and_then(Value::as_array) {
            if !incoming.is_empty() {
                rendered_section = true;
                writeln!(out, "incoming:")?;
                render_text_hierarchy_edges(incoming, &root_name, root_path, true, 0, out)?;
            }
        }
        if let Some(outgoing) = result.get("outgoingCalls").and_then(Value::as_array) {
            if !outgoing.is_empty() {
                rendered_section = true;
                writeln!(out, "outgoing:")?;
                render_text_hierarchy_edges(outgoing, &root_name, root_path, false, 0, out)?;
            }
        }
        if !rendered_section {
            writeln!(out, "  no calls found for requested direction")?;
        }
    }
    Ok(())
}

fn render_text_hierarchy_edges(
    calls: &[Value],
    parent_name: &str,
    parent_path: &str,
    incoming: bool,
    depth: usize,
    out: &mut dyn Write,
) -> io::Result<()> {
    let indent = "  ".repeat(depth + 1);
    for call in calls {
        let item_key = if incoming { "from" } else { "to" };
        let item = call.get(item_key).unwrap_or(&Value::Null);
        let other_name = item_label(item).unwrap_or_else(|| "<unknown>".to_string());
        let other_path = item_path(item).unwrap_or("");
        let callsite_path = if incoming { other_path } else { parent_path };
        let location = hierarchy_call_location(call, callsite_path, item);
        let relation = if incoming {
            format!("{other_name} -> {parent_name}")
        } else {
            format!("{parent_name} -> {other_name}")
        };
        if location.is_empty() {
            writeln!(out, "{indent}{relation}")?;
        } else {
            writeln!(out, "{indent}{relation}  {location}")?;
        }
        if let Some(children) = call.get("children").and_then(Value::as_array) {
            render_text_hierarchy_edges(
                children,
                &other_name,
                other_path,
                incoming,
                depth + 1,
                out,
            )?;
        }
    }
    Ok(())
}

fn result_symbol_label(result: &Value) -> String {
    first_string(
        result,
        &["qualifiedName", "signature", "detail", "symbolName", "name"],
    )
    .map(display_symbol_label)
    .unwrap_or_else(|| "<unknown>".to_string())
}

fn display_graph_symbol(symbol: &str) -> String {
    display_symbol_label(symbol)
}

fn item_label(item: &Value) -> Option<String> {
    first_string(item, &["signature", "detail", "name"]).map(display_symbol_label)
}

fn first_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
    })
}

fn item_path(item: &Value) -> Option<&str> {
    item.get("path").and_then(Value::as_str)
}

fn item_location(item: &Value) -> String {
    let path = item.get("path").and_then(Value::as_str).unwrap_or("");
    if path.is_empty() {
        return String::new();
    }
    format_location(
        path,
        item.get("selectionRange").or_else(|| item.get("range")),
    )
}

fn hierarchy_call_location(call: &Value, path: &str, item: &Value) -> String {
    if path.is_empty() {
        return String::new();
    }
    let range = call
        .get("fromRanges")
        .and_then(Value::as_array)
        .and_then(|ranges| ranges.first())
        .or_else(|| item.get("selectionRange"))
        .or_else(|| item.get("range"));
    format_location(path, range)
}

fn render_text_page_hint(value: &Value, out: &mut dyn Write) -> io::Result<()> {
    let Some(cursor) = next_cursor(value) else {
        return Ok(());
    };
    let shown = value
        .get("results")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    writeln!(out)?;
    if shown > 0 {
        writeln!(
            out,
            "more: showing first {shown} results; use --cursor {cursor} for the next page or increase --limit"
        )?;
    } else {
        writeln!(
            out,
            "more: additional results available; use --cursor {cursor} for the next page or increase --limit"
        )?;
    }
    Ok(())
}

fn next_cursor(value: &Value) -> Option<&str> {
    value
        .get("nextCursor")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/page/nextCursor").and_then(Value::as_str))
        .filter(|cursor| !cursor.is_empty())
}

fn render_text_source_context(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let Some(source) = result.get("source") else {
        return Ok(());
    };
    let Some(content) = source.get("content").and_then(Value::as_str) else {
        return Ok(());
    };
    if content.is_empty() {
        return Ok(());
    }
    let start_line = source.get("startLine").and_then(Value::as_u64).unwrap_or(1);
    writeln!(out, "  source:")?;
    for (idx, line) in content.lines().enumerate() {
        writeln!(out, "    {:>4} | {}", start_line + idx as u64, line)?;
    }
    if source.get("truncated").and_then(Value::as_bool) == Some(true) {
        writeln!(out, "    ...")?;
    }
    Ok(())
}

fn render_text_relation_summary(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let Some(relations) = result.get("relations") else {
        return Ok(());
    };
    let calls = relation_names(relations.get("calls"), false);
    let callers = relation_names(relations.get("callers"), true);
    if calls.is_empty() && callers.is_empty() {
        return Ok(());
    }
    if !calls.is_empty() {
        writeln!(out, "  calls: {}", calls.join(", "))?;
    }
    if !callers.is_empty() {
        writeln!(out, "  callers: {}", callers.join(", "))?;
    }
    Ok(())
}

fn relation_names(value: Option<&Value>, prefer_enclosing: bool) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(5)
        .filter_map(|relation| {
            let target = relation.get("target").and_then(Value::as_str);
            let enclosing = relation.get("enclosingSymbol").and_then(Value::as_str);
            if prefer_enclosing {
                enclosing.or(target)
            } else {
                target.or(enclosing)
            }
            .map(display_symbol)
        })
        .collect()
}

fn format_location(path: &str, range: Option<&Value>) -> String {
    let Some(range) = range else {
        return path.to_string();
    };
    let start = range
        .pointer("/start/line")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let end = range
        .pointer("/end/line")
        .and_then(Value::as_u64)
        .unwrap_or(start);
    if start == end {
        format!("{path}:{start}")
    } else {
        format!("{path}:{start}-{end}")
    }
}

fn display_symbol(symbol: &str) -> String {
    let symbol = symbol.trim();
    if symbol.contains("::") {
        return symbol.to_string();
    }
    symbol
        .rsplit(['.', '/', '#'])
        .find(|part| !part.is_empty())
        .unwrap_or(symbol)
        .trim_start_matches("function")
        .trim_start_matches('-')
        .to_string()
}

fn display_symbol_label(symbol: &str) -> String {
    symbol.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn one_line_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn render_text_summary(value: &Value, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "summary:")?;
    if let Some(matches) = value
        .pointer("/guard/estimatedMatches")
        .and_then(Value::as_u64)
    {
        writeln!(out, "  estimated matches: {matches}")?;
    }
    if let Some(files) = value.pointer("/guard/matchedFiles").and_then(Value::as_u64) {
        writeln!(out, "  matched files: {files}")?;
    }
    render_text_facets(
        value.pointer("/summary/facets/language"),
        out,
        "top languages",
    )?;
    render_text_facets(value.pointer("/summary/facets/topDir"), out, "top dirs")?;
    Ok(())
}

fn render_text_facets(facets: Option<&Value>, out: &mut dyn Write, label: &str) -> io::Result<()> {
    let Some(values) = facets.and_then(Value::as_array) else {
        return Ok(());
    };
    if values.is_empty() {
        return Ok(());
    }
    let rendered = values
        .iter()
        .take(5)
        .filter_map(|facet| {
            let value = facet.get("value").and_then(Value::as_str)?;
            let count = facet.get("count").and_then(Value::as_u64)?;
            Some(format!("{value}={count}"))
        })
        .collect::<Vec<_>>();
    if !rendered.is_empty() {
        writeln!(out, "  {label}: {}", rendered.join(", "))?;
    }
    Ok(())
}
