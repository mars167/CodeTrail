use std::fs;

use assert_cmd::Command;
use codetrail::workspace::Workspace;
use serde_json::{json, Value};
use tempfile::tempdir;

fn codetrail() -> Command {
    let mut command = Command::cargo_bin("codetrail").expect("binary exists");
    command
        .env("CODETRAIL_INTERNAL_JSON", "1")
        .arg("--output")
        .arg("json");
    command
}

fn raw_codetrail() -> Command {
    Command::cargo_bin("codetrail").expect("binary exists")
}

fn parse_json(output: Vec<u8>) -> Value {
    serde_json::from_slice(&output).unwrap()
}

#[test]
fn graph_calls_respect_scope_after_index_build() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::create_dir_all(dir.path().join("src/test/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/UserService.java"),
        "package example; class UserService { void run() { mainTarget(); } void mainTarget() {} }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/test/java/example/UserServiceTest.java"),
        "package example; class UserServiceTest { void run() { testTarget(); } void testTarget() {} }\n",
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let scoped_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--dir", "src/main", "--ext", "java", "calls", "run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let scoped_json = parse_json(scoped_output);
    let paths = scoped_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(scoped_json["query"]["producer"], "graph");
    assert_eq!(paths, vec!["src/main/java/example/UserService.java"]);

    let compatible_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--dir",
            "src/main",
            "--ext",
            "java",
            "calls",
            "UserService.run()",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let compatible_json = parse_json(compatible_output);
    assert_eq!(compatible_json["query"]["producer"], "graph");
    assert_eq!(
        compatible_json["results"][0]["matchedInputVariant"]["kind"],
        "signature_tail"
    );
    assert_eq!(
        compatible_json["results"][0]["path"],
        "src/main/java/example/UserService.java"
    );
}

#[test]
fn precise_scip_respects_default_ignore_case_and_strict_mode() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/UserService.java"),
        "package example; class UserService { void findUser() {} }\n",
    )
    .unwrap();
    let scip_path = dir.path().join("index.scip.json");
    fs::write(
        &scip_path,
        r##"{
  "documents": [
    {
      "relativePath": "src/main/java/example/UserService.java",
      "language": "java",
      "occurrences": [
        { "range": [0, 43, 51], "symbol": "semanticdb maven . . UserService#findUser().", "symbolRoles": 1 }
      ],
      "symbols": [
        { "symbol": "semanticdb maven . . UserService#findUser().", "displayName": "findUser", "kind": "method" }
      ]
    }
  ]
}"##,
    )
    .unwrap();

    let workspace = Workspace::discover(dir.path()).unwrap();
    codetrail::scip_index::import_scip_json(&workspace, &scip_path).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--ext", "java", "defs", "finduser"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json = parse_json(output);
    assert_eq!(json["query"]["producer"], "scip");
    assert_eq!(json["results"][0]["name"], "findUser");
    assert!(json["results"][0].get("matchedInputVariant").is_some());

    let strict_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--ext",
            "java",
            "--input-mode",
            "strict",
            "defs",
            "finduser",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let strict_json = parse_json(strict_output);
    assert!(strict_json["results"].as_array().unwrap().is_empty());
}

#[test]
fn mcp_tools_list_advertises_semantic_scope_options_only() {
    let dir = tempdir().unwrap();
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list"
    });
    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("mcp")
        .write_stdin(format!("{request}\n"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    let response: Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    let tools = response["result"]["tools"].as_array().unwrap();
    assert!(!tools.iter().any(|tool| tool["name"] == "codetrail_find"));
    let defs = tool_schema(tools, "codetrail_defs");
    for field in [
        "dir",
        "ext",
        "filePattern",
        "fileMode",
        "caseSensitive",
        "inputMode",
    ] {
        assert!(
            defs["properties"].get(field).is_some(),
            "codetrail_defs missing {field}"
        );
    }
}

fn tool_schema<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools
        .iter()
        .find(|tool| tool["name"] == name)
        .and_then(|tool| tool.get("inputSchema"))
        .unwrap_or_else(|| panic!("missing tool schema for {name}"))
}
