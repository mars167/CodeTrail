use std::path::Path;

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

pub(crate) fn lancedb_connect_uri(path: &Path) -> String {
    lancedb_connect_uri_from_display(&path.display().to_string())
}

pub(crate) fn lancedb_connect_uri_from_display(display: &str) -> String {
    let normalized = if let Some(rest) = display.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = display.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        display.to_string()
    };

    if looks_like_windows_path(&normalized) {
        normalize_separators(&normalized)
    } else {
        normalized
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
    }

    #[test]
    fn relative_path_uses_path_strip_prefix_for_native_paths() {
        let root = PathBuf::from("workspace");
        let path = root.join("src").join("main.rs");

        let rel = relative_path(&root, &path);

        assert_eq!(rel, "src/main.rs");
    }
}
