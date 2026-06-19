use std::{
    io::{self, IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::cli::OutputFormat;

pub struct ProgressIndicator {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    active: bool,
}

impl ProgressIndicator {
    pub fn start(format: &OutputFormat, message: impl Into<String>) -> Self {
        if !should_show_progress(format, io::stderr().is_terminal()) {
            return Self {
                running: Arc::new(AtomicBool::new(false)),
                handle: None,
                active: false,
            };
        }

        let message = message.into();
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let handle = thread::spawn(move || {
            let frames = ["-", "\\", "|", "/"];
            let mut idx = 0usize;
            while thread_running.load(Ordering::Relaxed) {
                let _ = write!(io::stderr(), "\r{} {}", frames[idx % frames.len()], message);
                let _ = io::stderr().flush();
                idx = idx.wrapping_add(1);
                thread::sleep(Duration::from_millis(120));
            }
            let _ = write!(io::stderr(), "\r{}\r", " ".repeat(message.len() + 4));
            let _ = io::stderr().flush();
        });
        Self {
            running,
            handle: Some(handle),
            active: true,
        }
    }

    pub fn finish(mut self, message: impl AsRef<str>) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if self.active {
            let message = message.as_ref();
            if !message.is_empty() {
                let _ = writeln!(io::stderr(), "{message}");
            }
        }
    }
}

fn should_show_progress(format: &OutputFormat, stderr_is_terminal: bool) -> bool {
    *format == OutputFormat::Text && stderr_is_terminal
}

pub fn stage_summary_line(
    label: &str,
    stages: &[(&str, Option<usize>)],
    elapsed: std::time::Duration,
) -> String {
    let rendered = stages
        .iter()
        .map(|(name, count)| match count {
            Some(count) => format!("{name}={count}"),
            None => (*name).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{label} complete ({rendered}) in {:.2}s",
        elapsed.as_secs_f64()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_indicator_is_enabled_only_for_text_tty_output() {
        assert!(should_show_progress(&OutputFormat::Text, true));
        assert!(!should_show_progress(&OutputFormat::Text, false));
        assert!(!should_show_progress(&OutputFormat::Json, true));
        assert!(!should_show_progress(&OutputFormat::CompactJson, true));
        assert!(!should_show_progress(&OutputFormat::Jsonl, true));
    }

    #[test]
    fn progress_stage_line_includes_elapsed_and_counts() {
        let line = stage_summary_line(
            "index build",
            &[("scan", Some(12)), ("proof", Some(12)), ("semantic", None)],
            std::time::Duration::from_millis(1250),
        );
        assert!(line.contains("index build complete"));
        assert!(line.contains("scan=12"));
        assert!(line.contains("proof=12"));
        assert!(line.contains("semantic"));
        assert!(line.contains("1.25s"));
    }
}
