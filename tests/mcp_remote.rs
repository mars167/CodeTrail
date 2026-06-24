use std::fs;

use assert_cmd::Command;
use serde_json::{json, Value};
use tempfile::tempdir;

fn codetrail_raw() -> Command {
    Command::cargo_bin("codetrail").expect("binary exists")
}

fn mcp_call(root: &std::path::Path, tool: &str, arguments: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": arguments
        }
    });
    let output = codetrail_raw()
        .arg("--path")
        .arg(root)
        .arg("mcp")
        .write_stdin(format!("{request}\n"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).unwrap();
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

#[test]
fn mcp_find_remote_only_is_legacy_and_rejected() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() {\n    let marker = \"remote-needle\";\n}\n",
    )
    .unwrap();

    let result = mcp_call(
        dir.path(),
        "codetrail_find",
        json!({
            "text": "remote-needle",
            "remoteMode": "only",
            "remoteSnapshot": "snapshot",
            "allowBroad": true
        }),
    );

    assert!(result["results"].as_array().unwrap().is_empty());
    assert_eq!(result["error"]["code"], "unknown_tool");
    assert!(result.get("caveats").is_none());
}
