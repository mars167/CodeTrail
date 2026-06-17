use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dunce::canonicalize;

use crate::project_graph::ProjectLanguage;

const DEFAULT_JDTLS_READY_TIMEOUT_MS: u64 = 30_000;
const JDTLS_READY_TIMEOUT_ENV: &str = "CODETRAIL_LSP_JAVA_READY_TIMEOUT_MS";

#[derive(Clone, Debug)]
pub struct ServerSpec {
    pub program: String,
    pub args: Vec<String>,
    pub provider_id: String,
    pub readiness: ReadinessStrategy,
}

#[derive(Clone, Debug)]
pub enum ReadinessStrategy {
    /// Server is ready immediately after initialize.
    Immediate,
    /// Wait for `$/progress` end notification (rust-analyzer).
    ProgressEnd { timeout_ms: u64 },
    /// Wait for `language/status` ServiceReady (jdtls).
    LanguageStatus { timeout_ms: u64 },
}

pub fn resolve_server(language: &ProjectLanguage) -> Option<ServerSpec> {
    if let Some(spec) = resolve_from_env(language) {
        return Some(spec);
    }
    match language {
        ProjectLanguage::Go => Some(ServerSpec {
            program: resolve_binary("gopls")?,
            args: vec!["serve".to_string()],
            provider_id: "gopls".to_string(),
            readiness: ReadinessStrategy::Immediate,
        }),
        ProjectLanguage::Rust => Some(ServerSpec {
            program: resolve_binary("rust-analyzer")?,
            args: Vec::new(),
            provider_id: "rust-analyzer".to_string(),
            readiness: ReadinessStrategy::ProgressEnd {
                timeout_ms: 120_000,
            },
        }),
        ProjectLanguage::Java => Some(ServerSpec {
            program: resolve_binary("jdtls")?,
            args: Vec::new(),
            provider_id: "jdtls".to_string(),
            readiness: ReadinessStrategy::LanguageStatus {
                timeout_ms: jdtls_ready_timeout_ms(),
            },
        }),
        ProjectLanguage::TypeScript => Some(ServerSpec {
            program: resolve_binary("typescript-language-server")?,
            args: vec!["--stdio".to_string()],
            provider_id: "typescript-language-server".to_string(),
            readiness: ReadinessStrategy::Immediate,
        }),
        ProjectLanguage::Ruby => Some(ServerSpec {
            program: resolve_binary("ruby-lsp")?,
            args: Vec::new(),
            provider_id: "ruby-lsp".to_string(),
            readiness: ReadinessStrategy::Immediate,
        }),
        ProjectLanguage::Swift => Some(ServerSpec {
            program: resolve_binary("sourcekit-lsp")?,
            args: Vec::new(),
            provider_id: "sourcekit-lsp".to_string(),
            readiness: ReadinessStrategy::Immediate,
        }),
    }
}

fn jdtls_ready_timeout_ms() -> u64 {
    env::var(JDTLS_READY_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|timeout_ms| *timeout_ms > 0)
        .unwrap_or(DEFAULT_JDTLS_READY_TIMEOUT_MS)
}

fn resolve_from_env(language: &ProjectLanguage) -> Option<ServerSpec> {
    let key = match language {
        ProjectLanguage::Go => "CODETRAIL_LSP_GO",
        ProjectLanguage::Rust => "CODETRAIL_LSP_RUST",
        ProjectLanguage::Java => "CODETRAIL_LSP_JAVA",
        ProjectLanguage::TypeScript => "CODETRAIL_LSP_TYPESCRIPT",
        ProjectLanguage::Ruby => "CODETRAIL_LSP_RUBY",
        ProjectLanguage::Swift => "CODETRAIL_LSP_SWIFT",
    };
    let value = env::var(key).ok()?;
    let mut parts = shell_words(&value).into_iter();
    let program = parts.next()?;
    let args = parts.collect();
    Some(ServerSpec {
        program,
        args,
        provider_id: format!("env:{key}"),
        readiness: ReadinessStrategy::Immediate,
    })
}

fn shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut word_started = false;
    let mut quote = None;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Some(active_quote) if ch == active_quote => {
                quote = None;
            }
            Some(active_quote) if ch == '\\' => {
                if let Some(&next) = chars.peek() {
                    if next == active_quote || next == '\\' {
                        current.push(chars.next().unwrap());
                    } else {
                        current.push(ch);
                    }
                } else {
                    current.push(ch);
                }
            }
            Some(_) => {
                current.push(ch);
            }
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                word_started = true;
            }
            None if ch.is_whitespace() => {
                if word_started {
                    words.push(std::mem::take(&mut current));
                    word_started = false;
                }
            }
            None if ch == '\\' => {
                if let Some(&next) = chars.peek() {
                    if next.is_whitespace() || next == '\'' || next == '"' || next == '\\' {
                        current.push(chars.next().unwrap());
                    } else {
                        current.push(ch);
                    }
                } else {
                    current.push(ch);
                }
                word_started = true;
            }
            None => {
                current.push(ch);
                word_started = true;
            }
        }
    }

    if word_started {
        words.push(current);
    }
    words
}

pub fn resolve_binary(name: &str) -> Option<String> {
    if name.contains(std::path::MAIN_SEPARATOR) {
        let path = PathBuf::from(name);
        if path.is_file() {
            return Some(path.to_string_lossy().to_string());
        }
        return None;
    }
    let path_var = env::var_os("PATH")?;
    let pathext = pathext_extensions();
    for dir in env::split_paths(&path_var) {
        for ext in &pathext {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn pathext_extensions() -> Vec<String> {
    #[cfg(windows)]
    {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string())
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(|ext| ext.to_ascii_lowercase())
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![String::new()]
    }
}

pub fn path_to_uri(workspace_root: &Path, relative_path: &str) -> Result<String> {
    let abs = canonicalize(workspace_root.join(relative_path))
        .with_context(|| format!("failed to canonicalize {relative_path}"))?;
    file_path_to_uri(&abs)
}

pub fn file_path_to_uri(path: &Path) -> Result<String> {
    let normalized = dunce::simplified(path);
    let mut path_str = normalized.to_string_lossy().replace('\\', "/");
    if path_str.starts_with("//") {
        let encoded = percent_encode_path(path_str.trim_start_matches('/'));
        return Ok(format!("file://{encoded}"));
    }
    if is_windows_drive_path(&path_str) {
        path_str.insert(0, '/');
    }
    let encoded = percent_encode_path(&path_str);
    Ok(format!("file://{encoded}"))
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn percent_encode_path(path: &str) -> String {
    let mut out = String::new();
    for ch in path.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '/' | ':' => out.push(ch),
            _ => {
                for byte in ch.to_string().as_bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

pub fn uri_to_relative_path(workspace_root: &Path, uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://")?;
    let mut decoded = percent_decode(path);
    if decoded.starts_with('/') && decoded.get(1..).is_some_and(is_windows_drive_path) {
        decoded.remove(0);
    }
    let abs = PathBuf::from(decoded);
    let root = canonicalize(workspace_root).ok()?;
    let abs = canonicalize(abs).ok()?;
    abs.strip_prefix(&root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_graph::ProjectLanguage;

    #[test]
    fn all_supported_languages_have_registry_entries() {
        for language in [
            ProjectLanguage::Go,
            ProjectLanguage::Rust,
            ProjectLanguage::Java,
            ProjectLanguage::TypeScript,
            ProjectLanguage::Ruby,
            ProjectLanguage::Swift,
        ] {
            let _ = resolve_server(&language);
        }
    }

    #[test]
    fn env_override_takes_precedence() {
        let key = "CODETRAIL_LSP_GO";
        let previous = std::env::var(key).ok();
        std::env::set_var(key, "\"/tmp/fake gopls\" --mode \"test value\"");
        let spec = resolve_server(&ProjectLanguage::Go).expect("env override spec");
        assert_eq!(spec.program, "/tmp/fake gopls");
        assert_eq!(
            spec.args,
            vec!["--mode".to_string(), "test value".to_string()]
        );
        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn ruby_env_override_takes_precedence() {
        let key = "CODETRAIL_LSP_RUBY";
        let previous = std::env::var(key).ok();
        std::env::set_var(key, "\"/tmp/fake ruby-lsp\" --stdio");
        let spec = resolve_server(&ProjectLanguage::Ruby).expect("ruby env override spec");
        assert_eq!(spec.provider_id, "env:CODETRAIL_LSP_RUBY");
        assert_eq!(spec.program, "/tmp/fake ruby-lsp");
        assert_eq!(spec.args, vec!["--stdio".to_string()]);
        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn jdtls_readiness_timeout_is_configurable() {
        let key = JDTLS_READY_TIMEOUT_ENV;
        let previous = std::env::var(key).ok();
        std::env::remove_var(key);
        assert_eq!(jdtls_ready_timeout_ms(), DEFAULT_JDTLS_READY_TIMEOUT_MS);

        std::env::set_var(key, "60000");
        assert_eq!(jdtls_ready_timeout_ms(), 60_000);

        std::env::set_var(key, "0");
        assert_eq!(jdtls_ready_timeout_ms(), DEFAULT_JDTLS_READY_TIMEOUT_MS);

        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn file_uri_roundtrip_unix_style() {
        let root = std::env::temp_dir().join("codetrail-lsp-test");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("src/lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let uri = file_path_to_uri(&file).unwrap();
        assert!(uri.starts_with("file://"));
        let rel = uri_to_relative_path(&root, &uri).unwrap();
        assert_eq!(rel, "src/lib.rs");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_uri_uses_windows_drive_slash() {
        let uri = file_path_to_uri(Path::new(r"C:\Program Files\jdtls\bin\jdtls.cmd")).unwrap();
        assert_eq!(uri, "file:///C:/Program%20Files/jdtls/bin/jdtls.cmd");
    }
}
