use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellPathMode {
    Unix,
    Windows,
}

impl ShellPathMode {
    const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

pub(crate) fn normalize_separators(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn relative_path(root: &Path, path: &Path) -> String {
    normalize_separators(
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .as_ref(),
    )
}

pub(crate) fn native_path(path: &Path) -> PathBuf {
    let display = path.display().to_string();
    match native_path_from_display_for_os(&display, ShellPathMode::current()) {
        Cow::Borrowed(_) => path.to_path_buf(),
        Cow::Owned(converted) => PathBuf::from(converted),
    }
}

pub(crate) fn lancedb_connect_uri(path: &Path) -> String {
    lancedb_connect_uri_from_display_for_os(&path.display().to_string(), ShellPathMode::current())
}

#[cfg(test)]
fn lancedb_connect_uri_from_display(display: &str) -> String {
    lancedb_connect_uri_from_display_for_os(display, ShellPathMode::Unix)
}

fn lancedb_connect_uri_from_display_for_os(
    display: &str,
    shell_path_mode: ShellPathMode,
) -> String {
    let normalized = if let Some(rest) = display.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = display.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        display.to_string()
    };

    let native = native_path_from_display_for_os(&normalized, shell_path_mode);
    if looks_like_windows_path(&native) {
        normalize_separators(&native)
    } else {
        native.into_owned()
    }
}

pub(crate) fn is_portable_relative_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains(':')
    {
        return false;
    }

    path.split('/')
        .all(|component| !matches!(component, "" | "." | ".."))
}

fn looks_like_windows_path(value: &str) -> bool {
    is_windows_drive_absolute(value) || value.starts_with(r"\\")
}

fn native_path_from_display_for_os(display: &str, shell_path_mode: ShellPathMode) -> Cow<'_, str> {
    match shell_path_mode {
        ShellPathMode::Unix => Cow::Borrowed(display),
        ShellPathMode::Windows => msys_drive_absolute_to_windows(display)
            .map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(display)),
    }
}

fn msys_drive_absolute_to_windows(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'/' || !bytes[1].is_ascii_alphabetic() {
        return None;
    }

    let drive = char::from(bytes[1].to_ascii_uppercase());
    match bytes.get(2) {
        None => Some(format!("{drive}:/")),
        Some(b'/') => Some(format!("{drive}:{}", normalize_separators(&value[2..]))),
        Some(_) => None,
    }
}

fn is_windows_drive_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn relative_path_normalizes_separators_when_path_text_contains_backslashes() {
        let path = r"src\nested\main.rs";

        let normalized = normalize_separators(path);

        assert_eq!(normalized, "src/nested/main.rs");
    }

    #[test]
    fn portable_relative_path_accepts_only_workspace_relative_slash_paths() {
        let valid = ["src/main.rs", "src dir/main.rs", "-dash/file.rs"];

        for path in valid {
            assert!(is_portable_relative_path(path), "{path}");
        }
    }

    #[test]
    fn portable_relative_path_rejects_windows_absolute_and_escape_paths() {
        let invalid = [
            "",
            ".",
            "..",
            "../outside.rs",
            "src/../outside.rs",
            "/etc/passwd",
            "/d/dev/repo/src/main.rs",
            r"C:\repo\src\main.rs",
            "C:/repo/src/main.rs",
            "C:repo/src/main.rs",
            r"\\server\share\repo\src\main.rs",
            "//server/share/repo/src/main.rs",
            r"\\?\C:\repo\src\main.rs",
            r"\\?\UNC\server\share\repo\src\main.rs",
            r"src\main.rs",
            "src:main.rs",
        ];

        for path in invalid {
            assert!(!is_portable_relative_path(path), "{path}");
        }
    }

    #[test]
    fn lancedb_connect_uri_keeps_unix_paths_unchanged() {
        assert_eq!(
            lancedb_connect_uri_from_display("/foo/bar/.codetrail/index.lance"),
            "/foo/bar/.codetrail/index.lance"
        );
        assert_eq!(
            lancedb_connect_uri_from_display_for_os(
                "/d/dev/repo/.codetrail/index.lance",
                ShellPathMode::Unix
            ),
            "/d/dev/repo/.codetrail/index.lance"
        );
    }

    #[test]
    fn lancedb_connect_uri_normalizes_windows_paths_before_uri_parsing() {
        assert_eq!(
            lancedb_connect_uri_from_display(r"C:\Users\mars\repo\.codetrail\index.lance"),
            "C:/Users/mars/repo/.codetrail/index.lance"
        );
        assert_eq!(
            lancedb_connect_uri_from_display(r"\\?\C:\Users\mars\repo\.codetrail\index.lance"),
            "C:/Users/mars/repo/.codetrail/index.lance"
        );
        assert_eq!(
            lancedb_connect_uri_from_display(r"\\?\UNC\server\share\repo\.codetrail\index.lance"),
            "//server/share/repo/.codetrail/index.lance"
        );
        assert_eq!(
            lancedb_connect_uri_from_display(r"\\server\share\repo\.codetrail\index.lance"),
            "//server/share/repo/.codetrail/index.lance"
        );
        assert_eq!(
            lancedb_connect_uri_from_display_for_os(
                "/d/dev/repo/.codetrail/index.lance",
                ShellPathMode::Windows
            ),
            "D:/dev/repo/.codetrail/index.lance"
        );
    }

    #[test]
    fn native_path_normalizes_windows_shell_drive_paths_only_on_windows() {
        assert_eq!(
            native_path_from_display_for_os("/d/dev/repo", ShellPathMode::Windows),
            "D:/dev/repo"
        );
        assert_eq!(
            native_path_from_display_for_os("/D/dev/repo", ShellPathMode::Windows),
            "D:/dev/repo"
        );
        assert_eq!(
            native_path_from_display_for_os("/d", ShellPathMode::Windows),
            "D:/"
        );
        assert_eq!(
            native_path_from_display_for_os("/d/dev/repo", ShellPathMode::Unix),
            "/d/dev/repo"
        );
        assert_eq!(
            native_path_from_display_for_os("/dev/repo", ShellPathMode::Windows),
            "/dev/repo"
        );
    }

    #[test]
    fn relative_path_uses_path_strip_prefix_for_native_paths() {
        let root = PathBuf::from("workspace");
        let path = root.join("src").join("main.rs");

        let rel = relative_path(&root, &path);

        assert_eq!(rel, "src/main.rs");
    }
}
