use std::path::Path;

use serde_json::json;

use super::with_workspace_root;

#[test]
fn with_workspace_root_omits_source_targets_for_nonportable_paths() {
    let paths = [
        "/tmp/outside.rs",
        "C:/repo/src/main.rs",
        r"C:\repo\src\main.rs",
        "C:repo/src/main.rs",
        "//server/share/repo/src/main.rs",
        r"\\server\share\repo\src\main.rs",
        "../outside.rs",
        r"src\main.rs",
        "src:main.rs",
    ];

    for path in paths {
        let enriched = with_workspace_root(
            json!({
                "results": [{
                    "path": path,
                    "line": 1,
                    "preview": "fn main() {}"
                }]
            }),
            Path::new("/workspace"),
        );
        let result = &enriched["results"][0];
        assert!(result.get("sourceTarget").is_none(), "{path}");
    }
}

#[test]
fn with_workspace_root_keeps_source_targets_for_missing_portable_paths() {
    let enriched = with_workspace_root(
        json!({
            "results": [{
                "path": "-dash/src/main.rs",
                "range": {
                    "start": { "line": 2 },
                    "end": { "line": 2 }
                },
                "preview": "fn main() {}"
            }]
        }),
        Path::new("/workspace"),
    );

    assert_eq!(
        enriched["results"][0]["sourceTarget"],
        "-dash/src/main.rs:2"
    );
}
