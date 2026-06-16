use anyhow::Result;
use regex::Regex;
use serde_json::{json, Value};

use crate::{
    index, search,
    workspace::{FileRecord, ScanOptions, Workspace},
};

const PRODUCER: &str = "framework_route_scanner";
const PARSER_FACT: &str = "parser_fact";

pub fn scan(
    workspace: &Workspace,
    opts: &ScanOptions,
    pattern: Option<&str>,
    frameworks: &[String],
    methods: &[String],
) -> Result<search::QueryOutput> {
    let mut scan_opts = opts.clone();
    scan_opts.limit = 0;
    if scan_opts.lang.is_empty() {
        scan_opts.lang = vec![
            "java".to_string(),
            "go".to_string(),
            "python".to_string(),
            "typescript".to_string(),
            "javascript".to_string(),
            "ruby".to_string(),
        ];
    }
    let files = workspace.scan_files(&scan_opts)?;
    let mut results = Vec::new();
    let filter = RouteFilter::new(pattern, frameworks, methods, opts.case_sensitive);
    for file in &files {
        let path = workspace.abs_path(&file.path);
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        collect_file_routes(file, &content, &filter, &mut results)?;
    }
    results.sort_by(|left, right| {
        path_key(left)
            .cmp(&path_key(right))
            .then(line_key(left).cmp(&line_key(right)))
            .then(route_key(left).cmp(&route_key(right)))
    });

    let page = search::page_results(
        Value::Array(results),
        opts,
        "routes",
        json!({
            "pattern": pattern,
            "framework": frameworks,
            "method": methods,
            "producer": PRODUCER
        }),
        &workspace.snapshot_id,
    )?;
    let mut index_meta = index::live_scan_index_meta("route_scan_live");
    index_meta["scanSummary"] = workspace.scan_summary(&scan_opts)?;
    Ok(search::QueryOutput {
        results: page.results,
        index: index_meta,
        truncated: page.truncated,
        next_cursor: page.next_cursor,
        facets: page.facets,
        guard: page.guard,
        budget: json!({}),
        query_plan: json!({ "producer": PRODUCER }),
        scan_stats: json!({ "candidateFiles": files.len(), "searchedFiles": files.len(), "skippedFiles": 0 }),
    })
}

struct RouteFilter {
    pattern: Option<String>,
    frameworks: Vec<String>,
    methods: Vec<String>,
    case_sensitive: bool,
}

impl RouteFilter {
    fn new(
        pattern: Option<&str>,
        frameworks: &[String],
        methods: &[String],
        case_sensitive: bool,
    ) -> Self {
        Self {
            pattern: pattern.map(normalize_filter_value(case_sensitive)),
            frameworks: frameworks
                .iter()
                .map(|value| normalize_filter_value(false)(value))
                .collect(),
            methods: methods
                .iter()
                .map(|value| value.to_ascii_uppercase())
                .collect(),
            case_sensitive,
        }
    }

    fn accepts(&self, framework: &str, method: &str, route: &str) -> bool {
        let framework = framework.to_ascii_lowercase();
        if !self.frameworks.is_empty() && !self.frameworks.iter().any(|value| value == &framework) {
            return false;
        }
        if !self.methods.is_empty()
            && !self
                .methods
                .iter()
                .any(|value| value == &method.to_ascii_uppercase())
        {
            return false;
        }
        let Some(pattern) = &self.pattern else {
            return true;
        };
        let route = normalize_filter_value(self.case_sensitive)(route);
        route.contains(pattern)
    }
}

fn normalize_filter_value(case_sensitive: bool) -> impl Fn(&str) -> String {
    move |value| {
        if case_sensitive {
            value.to_string()
        } else {
            value.to_ascii_lowercase()
        }
    }
}

fn collect_file_routes(
    file: &FileRecord,
    content: &str,
    filter: &RouteFilter,
    results: &mut Vec<Value>,
) -> Result<()> {
    match file.language.as_str() {
        "java" => java_routes(file, content, filter, results)?,
        "go" => go_routes(file, content, filter, results)?,
        "python" => python_routes(file, content, filter, results)?,
        "typescript" | "javascript" => js_routes(file, content, filter, results)?,
        "ruby" => ruby_routes(file, content, filter, results)?,
        _ => {}
    }
    Ok(())
}

fn push_route(
    file: &FileRecord,
    content: &str,
    filter: &RouteFilter,
    route: RouteMatch<'_>,
    results: &mut Vec<Value>,
) {
    if !filter.accepts(route.framework, route.method, route.path) {
        return;
    }
    results.push(json!({
        "path": file.path,
        "range": byte_range(content, route.start, route.end),
        "language": file.language,
        "framework": route.framework,
        "method": route.method,
        "routePattern": route.path,
        "handler": route.handler,
        "handlerKind": route.handler_kind,
        "fileHash": file.hash,
        "producer": PRODUCER,
        "reliability": PARSER_FACT,
        "layer": PARSER_FACT
    }));
}

struct RouteMatch<'a> {
    framework: &'a str,
    method: &'a str,
    path: &'a str,
    handler: Option<String>,
    handler_kind: &'a str,
    start: usize,
    end: usize,
}

fn java_routes(
    file: &FileRecord,
    content: &str,
    filter: &RouteFilter,
    results: &mut Vec<Value>,
) -> Result<()> {
    let prefix = class_request_mapping_prefix(content)?;
    let verb = Regex::new(
        r"@(GetMapping|PostMapping|PutMapping|PatchMapping|DeleteMapping)\b\s*(?:\(([^)]*)\))?",
    )?;
    for found in verb.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        let method = match &found[1] {
            "GetMapping" => "GET",
            "PostMapping" => "POST",
            "PutMapping" => "PUT",
            "PatchMapping" => "PATCH",
            "DeleteMapping" => "DELETE",
            _ => "ANY",
        };
        let path = join_path(
            &prefix,
            first_quoted(found.get(2).map_or("", |m| m.as_str())).unwrap_or(""),
        );
        let handler = method_name_after(content, whole.end());
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "spring",
                method,
                path: &path,
                handler,
                handler_kind: "method",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }

    let request = Regex::new(r"@RequestMapping\b\s*(?:\(([^)]*)\))?")?;
    for found in request.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        if annotation_targets_class(content, whole.end()) {
            continue;
        }
        let args = found.get(1).map_or("", |m| m.as_str());
        let path = join_path(&prefix, first_quoted(args).unwrap_or(""));
        let method = request_method(args).unwrap_or("ANY".to_string());
        let handler = method_name_after(content, whole.end());
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "spring",
                method: &method,
                path: &path,
                handler,
                handler_kind: "method",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }
    Ok(())
}

fn go_routes(
    file: &FileRecord,
    content: &str,
    filter: &RouteFilter,
    results: &mut Vec<Value>,
) -> Result<()> {
    let gin = Regex::new(
        r#"\b\w+\.(GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD|CONNECT|TRACE)\s*\(\s*"([^"]+)"\s*,\s*([^)]+)\)"#,
    )?;
    for found in gin.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "gin",
                method: &found[1],
                path: &found[2],
                handler: tail_ident(&found[3]),
                handler_kind: "function",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }

    let chi_verb = Regex::new(
        r#"\b\w+\.(Get|Post|Put|Patch|Delete|Options|Head|Connect|Trace)\s*\(\s*"([^"]+)"\s*,\s*([^)]+)\)"#,
    )?;
    for found in chi_verb.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        let method = found[1].to_ascii_uppercase();
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "chi",
                method: &method,
                path: &found[2],
                handler: tail_ident(&found[3]),
                handler_kind: "function",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }

    let chi_method =
        Regex::new(r#"\b\w+\.Method(?:Func)?\s*\(\s*"([A-Z]+)"\s*,\s*"([^"]+)"\s*,\s*([^)]+)\)"#)?;
    for found in chi_method.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "chi",
                method: &found[1],
                path: &found[2],
                handler: tail_ident(&found[3]),
                handler_kind: "function",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }

    let gorilla = Regex::new(
        r#"\b\w+\.HandleFunc\s*\(\s*"([^"]+)"\s*,\s*([^)]+)\)\.Methods\s*\(\s*"([A-Z]+)""#,
    )?;
    for found in gorilla.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "gorilla",
                method: &found[3],
                path: &found[1],
                handler: tail_ident(&found[2]),
                handler_kind: "function",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }

    let stdlib = Regex::new(r#"\bhttp\.(Handle|HandleFunc)\s*\(\s*"([^"]+)"\s*,\s*([^)]+)\)"#)?;
    for found in stdlib.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "net/http",
                method: "ANY",
                path: &found[2],
                handler: tail_ident(&found[3]),
                handler_kind: "function",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }
    Ok(())
}

fn python_routes(
    file: &FileRecord,
    content: &str,
    filter: &RouteFilter,
    results: &mut Vec<Value>,
) -> Result<()> {
    let django = Regex::new(
        r#"\b(path|re_path|url)\s*\(\s*r?['"]([^'"]+)['"]\s*,\s*([A-Za-z_][\w.]*(?:\s*\([^)]*\))?)"#,
    )?;
    for found in django.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "django",
                method: "ANY",
                path: &found[2],
                handler: python_handler(&found[3]),
                handler_kind: "view",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }

    let drf = Regex::new(r#"\.register\s*\(\s*r?['"]([^'"]+)['"]\s*,\s*([A-Za-z_][\w.]*)"#)?;
    for found in drf.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        let path = format!("/{}", found[1].trim_matches('/'));
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "django",
                method: "VIEWSET",
                path: &path,
                handler: tail_ident(&found[2]),
                handler_kind: "viewset",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }

    let fastapi_receivers = fastapi_route_receivers(content)?;
    let decorator = Regex::new(
        r#"(?m)@([A-Za-z_][\w.]*)\.(get|post|put|patch|delete|options|head|route|api_route)\s*\(([^)]*)\)\s*(?:\r?\n\s*)+(?:async\s+)?def\s+([A-Za-z_]\w*)"#,
    )?;
    for found in decorator.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        let method = if matches!(&found[2], "route" | "api_route") {
            "ANY".to_string()
        } else {
            found[2].to_ascii_uppercase()
        };
        let framework = python_decorator_framework(&found[1], &fastapi_receivers);
        let path = first_quoted(&found[3]).unwrap_or("");
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework,
                method: &method,
                path,
                handler: Some(found[4].to_string()),
                handler_kind: "function",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }
    Ok(())
}

fn fastapi_route_receivers(content: &str) -> Result<Vec<String>> {
    let assignment =
        Regex::new(r#"(?m)\b([A-Za-z_]\w*)\s*=\s*(?:[A-Za-z_]\w*\.)?(FastAPI|APIRouter)\s*\("#)?;
    Ok(assignment
        .captures_iter(content)
        .map(|found| found[1].to_string())
        .collect())
}

fn python_decorator_framework<'a>(receiver: &str, fastapi_receivers: &'a [String]) -> &'static str {
    let receiver_tail = receiver.rsplit('.').next().unwrap_or(receiver);
    if receiver.contains("router")
        || fastapi_receivers
            .iter()
            .any(|candidate| candidate == receiver || candidate == receiver_tail)
    {
        "fastapi"
    } else {
        "flask"
    }
}

fn js_routes(
    file: &FileRecord,
    content: &str,
    filter: &RouteFilter,
    results: &mut Vec<Value>,
) -> Result<()> {
    let express = Regex::new(
        r#"\b(?:app|router)\.(get|post|put|patch|delete|options|head|all|use)\s*\(\s*['"]([^'"]+)['"]\s*,"#,
    )?;
    for found in express.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        let method = found[1].to_ascii_uppercase();
        let open = content[whole.start()..]
            .find('(')
            .map(|offset| whole.start() + offset);
        let close = open
            .and_then(|idx| matching_paren(content, idx))
            .unwrap_or(whole.end());
        let args = &content[whole.end()..close];
        let handler = if args.contains("=>") {
            Some("<inline>".to_string())
        } else {
            tail_ident(args)
        };
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "express",
                method: &method,
                path: &found[2],
                handler,
                handler_kind: "function",
                start: whole.start(),
                end: close,
            },
            results,
        );
    }

    let prefix = nest_controller_prefix(content)?;
    let nest = Regex::new(r"@(Get|Post|Put|Patch|Delete|Options|Head|All)\b\s*(?:\(([^)]*)\))?")?;
    for found in nest.captures_iter(content) {
        let Some(whole) = found.get(0) else {
            continue;
        };
        let method = found[1].to_ascii_uppercase();
        let path = join_path(
            &prefix,
            first_quoted(found.get(2).map_or("", |m| m.as_str())).unwrap_or(""),
        );
        push_route(
            file,
            content,
            filter,
            RouteMatch {
                framework: "nestjs",
                method: &method,
                path: &path,
                handler: js_method_name_after(content, whole.end()),
                handler_kind: "method",
                start: whole.start(),
                end: whole.end(),
            },
            results,
        );
    }
    Ok(())
}

fn ruby_routes(
    file: &FileRecord,
    content: &str,
    filter: &RouteFilter,
    results: &mut Vec<Value>,
) -> Result<()> {
    let root = Regex::new(r#"^\s*root\s+(?:to:\s*)?['"]([^'"]+)#([^'"]+)['"]"#)?;
    let verb = Regex::new(
        r#"^\s*(get|post|put|patch|delete)\s+['"]([^'"]+)['"].*?(?:to:\s*)?['"]([^'"]+)#([^'"]+)['"]"#,
    )?;
    let resource = Regex::new(r#"^\s*resources?\s+:([A-Za-z_]\w*)"#)?;
    let mut offset = 0;
    for line in content.lines() {
        if let Some(found) = root.captures(line) {
            push_ruby_line(
                file,
                content,
                filter,
                results,
                "GET",
                "/",
                &found[1],
                &found[2],
                offset,
                line.len(),
            );
        }
        if let Some(found) = verb.captures(line) {
            push_ruby_line(
                file,
                content,
                filter,
                results,
                &found[1].to_ascii_uppercase(),
                &found[2],
                &found[3],
                &found[4],
                offset,
                line.len(),
            );
        }
        if let Some(found) = resource.captures(line) {
            let name = found[1].to_string();
            let path = format!("/{name}");
            push_route(
                file,
                content,
                filter,
                RouteMatch {
                    framework: "rails",
                    method: "RESOURCE",
                    path: &path,
                    handler: Some(format!("{name}#*")),
                    handler_kind: "controller",
                    start: offset,
                    end: offset + line.len(),
                },
                results,
            );
        }
        offset += line.len() + 1;
    }
    Ok(())
}

fn push_ruby_line(
    file: &FileRecord,
    content: &str,
    filter: &RouteFilter,
    results: &mut Vec<Value>,
    method: &str,
    path: &str,
    controller: &str,
    action: &str,
    start: usize,
    len: usize,
) {
    push_route(
        file,
        content,
        filter,
        RouteMatch {
            framework: "rails",
            method,
            path,
            handler: Some(format!("{controller}#{action}")),
            handler_kind: "controller_action",
            start,
            end: start + len,
        },
        results,
    );
}

fn class_request_mapping_prefix(content: &str) -> Result<String> {
    let class = Regex::new(
        r"@RequestMapping\s*(?:\(([^)]*)\))?[\s\r\n]*(?:@[A-Za-z.]+(?:\([^)]*\))?\s*)*(?:public\s+|final\s+|abstract\s+)*class\b",
    )?;
    Ok(class
        .captures(content)
        .and_then(|found| found.get(1).and_then(|m| first_quoted(m.as_str())))
        .unwrap_or("")
        .to_string())
}

fn nest_controller_prefix(content: &str) -> Result<String> {
    let controller = Regex::new(r"@Controller\s*(?:\(([^)]*)\))?")?;
    Ok(controller
        .captures(content)
        .and_then(|found| found.get(1).and_then(|m| first_quoted(m.as_str())))
        .unwrap_or("")
        .to_string())
}

fn annotation_targets_class(content: &str, start: usize) -> bool {
    let Some(tail) = utf8_window(content, start, 500) else {
        return false;
    };
    let class_at =
        Regex::new(r"\b(?:(?:public|private|protected|abstract|final|static)\s+)*class\b")
            .ok()
            .and_then(|class| class.find(tail).map(|m| m.start()));
    let method_at =
        Regex::new(r"\b(?:public|private|protected)\s+(?:static\s+)?[^;{=]*?\s+[A-Za-z_]\w*\s*\(")
            .ok()
            .and_then(|method| method.find(tail).map(|m| m.start()));
    class_at.is_some() && (method_at.is_none() || class_at < method_at)
}

fn request_method(args: &str) -> Option<String> {
    let raw = args
        .split("RequestMethod.")
        .nth(1)
        .or_else(|| args.split("method").nth(1))?;
    let method = raw
        .chars()
        .skip_while(|ch| !ch.is_ascii_alphabetic())
        .take_while(|ch| ch.is_ascii_alphabetic())
        .collect::<String>();
    (!method.is_empty()).then(|| method.to_ascii_uppercase())
}

fn method_name_after(content: &str, start: usize) -> Option<String> {
    let tail = utf8_window(content, start, 700)?;
    let method = Regex::new(
        r"\b(?:public|private|protected)\s+(?:static\s+)?[^;{=]*?\s+([A-Za-z_]\w*)\s*\(",
    )
    .ok()?;
    method
        .captures(tail)
        .and_then(|found| found.get(1).map(|m| m.as_str().to_string()))
}

fn js_method_name_after(content: &str, start: usize) -> Option<String> {
    let tail = utf8_window(content, start, 500)?;
    let method = Regex::new(r"\b(?:async\s+)?([A-Za-z_$][\w$]*)\s*\(").ok()?;
    method
        .captures(tail)
        .and_then(|found| found.get(1).map(|m| m.as_str().to_string()))
}

fn utf8_window(content: &str, start: usize, max_len: usize) -> Option<&str> {
    if start > content.len() || !content.is_char_boundary(start) {
        return None;
    }
    let mut end = content.len().min(start.saturating_add(max_len));
    while end > start && !content.is_char_boundary(end) {
        end -= 1;
    }
    Some(&content[start..end])
}

fn python_handler(expr: &str) -> Option<String> {
    let target = expr.trim().trim_end_matches(".as_view()");
    tail_ident(target)
}

fn tail_ident(expr: &str) -> Option<String> {
    let mut ident = String::new();
    for ch in expr.chars().rev() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            ident.push(ch);
        } else if !ident.is_empty() {
            break;
        }
    }
    if ident.is_empty() {
        None
    } else {
        Some(ident.chars().rev().collect())
    }
}

fn first_quoted(input: &str) -> Option<&str> {
    let mut chars = input.char_indices();
    while let Some((idx, ch)) = chars.next() {
        if ch != '\'' && ch != '"' {
            continue;
        }
        let quote = ch;
        let start = idx + ch.len_utf8();
        for (end, next) in chars.by_ref() {
            if next == quote {
                return Some(&input[start..end]);
            }
        }
        return None;
    }
    None
}

fn join_path(prefix: &str, path: &str) -> String {
    let left = prefix.trim_matches('/');
    let right = path.trim_matches('/');
    match (left.is_empty(), right.is_empty()) {
        (true, true) => "/".to_string(),
        (true, false) => format!("/{right}"),
        (false, true) => format!("/{left}"),
        (false, false) => format!("/{left}/{right}"),
    }
}

fn matching_paren(content: &str, open: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    if bytes.get(open).copied()? != b'(' {
        return None;
    }
    let mut depth = 0usize;
    for (idx, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn byte_range(content: &str, start: usize, end: usize) -> Value {
    let (start_line, start_column) = line_column(content, start);
    let (end_line, end_column) = line_column(content, end);
    json!({
        "start": { "line": start_line, "column": start_column },
        "end": { "line": end_line, "column": end_column }
    })
}

fn line_column(content: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    for (idx, byte) in content.as_bytes().iter().enumerate() {
        if idx >= offset {
            break;
        }
        if *byte == b'\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn path_key(value: &Value) -> String {
    value
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn route_key(value: &Value) -> String {
    value
        .get("routePattern")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn line_key(value: &Value) -> u64 {
    value
        .pointer("/range/start/line")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}
