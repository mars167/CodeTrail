use std::io::{self, Write};

use serde::Serialize;
use serde_json::Value;

use super::projection::{public_response, PublicPage};

#[derive(Debug, Serialize)]
struct ResultEvent<'a> {
    event: &'static str,
    result: &'a Value,
}

#[derive(Debug, Serialize)]
struct PageEvent {
    event: &'static str,
    page: PublicPage,
}

#[derive(Debug, Serialize)]
struct ErrorEvent<'a> {
    event: &'static str,
    error: &'a Value,
}

pub(super) fn render_jsonl(value: &Value, out: &mut dyn Write) -> io::Result<()> {
    let public = public_response(value);
    if let Some(error) = public.error.as_ref() {
        let event = ErrorEvent {
            event: "error",
            error,
        };
        serde_json::to_writer(&mut *out, &event)?;
        writeln!(out)?;
        return Ok(());
    }
    if let Some(results) = public.results.as_array() {
        for result in results {
            let event = ResultEvent {
                event: "result",
                result,
            };
            serde_json::to_writer(&mut *out, &event)?;
            writeln!(out)?;
        }
    }
    let event = PageEvent {
        event: "page",
        page: public.page,
    };
    serde_json::to_writer(&mut *out, &event)?;
    writeln!(out)?;
    Ok(())
}
