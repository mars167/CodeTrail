use std::fs;

use assert_cmd::Command;
use serde_json::Value;
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
fn content_search_scopes_by_dir_extension_case_and_wildcard() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::create_dir_all(dir.path().join("src/test/java/example")).unwrap();
    fs::create_dir_all(dir.path().join("src/main/kotlin/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/FooService.java"),
        "class FooService { String value = \"FooXYZbar\"; }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/test/java/example/FooServiceTest.java"),
        "class FooServiceTest { String value = \"FooXYZbar\"; }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/main/kotlin/example/FooService.kt"),
        "class FooService { val value = \"FooXYZbar\" }\n",
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--dir", "src/main", "--ext", "java", "find", "foo*bar", "--mode", "wildcard",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json = parse_json(output);

    assert_eq!(json["query"]["mode"], "wildcard");
    assert_eq!(json["query"]["caseSensitive"], false);
    assert_eq!(
        json["query"]["scope"]["dirs"],
        serde_json::json!(["src/main"])
    );
    assert_eq!(
        json["query"]["scope"]["extensions"],
        serde_json::json!(["java"])
    );
    let paths = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["src/main/java/example/FooService.java"]);
    assert!(json["scanStats"]["candidateFiles"].as_u64().unwrap() < 3);

    let strict_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--dir",
            "src/main",
            "--ext",
            "java",
            "--case-sensitive",
            "find",
            "foo*bar",
            "--mode",
            "wildcard",
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
fn path_search_uses_wildcard_mode_and_file_pattern_scope() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/other")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/UserService.java"),
        "class UserService {}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/main/java/other/UserController.java"),
        "class UserController {}\n",
    )
    .unwrap();

    let files_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--dir", "src/main", "files", "*.java", "--mode", "wildcard"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let files_json = parse_json(files_output);
    assert_eq!(files_json["results"].as_array().unwrap().len(), 2);

    let find_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--file-pattern",
            "*Service.java",
            "--file-mode",
            "wildcard",
            "find",
            "class",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let find_json = parse_json(find_output);
    let paths = find_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["src/main/java/example/UserService.java"]);
}

#[test]
fn compatible_symbol_input_matches_qualified_signature_style_and_case_forms() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/UserService.java"),
        "package example;\nclass UserService {\n  int count;\n  void findUser(Long id) { selectUserById(); this.count++; }\n  void selectUserById() {}\n  void run() { helper(); }\n  void helper() {}\n}\n",
    )
    .unwrap();

    let defs_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--ext", "java", "defs", "UserService.findUser(Long)"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json = parse_json(defs_output);
    assert_eq!(defs_json["results"][0]["name"], "findUser");
    assert_eq!(
        defs_json["results"][0]["matchedInputVariant"]["kind"],
        "signature_tail"
    );

    let symbols_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--ext", "java", "symbols", "select_user_by_id"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let symbols_json = parse_json(symbols_output);
    assert_eq!(symbols_json["results"][0]["name"], "selectUserById");

    let calls_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--ext", "java", "calls", "UserService.run()"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let calls_json = parse_json(calls_output);
    assert_eq!(calls_json["results"][0]["enclosingSymbol"], "run");
    assert_eq!(calls_json["results"][0]["target"], "helper");

    let callers_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--ext", "java", "callers", "UserService.helper()"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json = parse_json(callers_output);
    assert_eq!(callers_json["results"][0]["enclosingSymbol"], "run");
    assert_eq!(callers_json["results"][0]["target"], "helper");

    let case_sensitive_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--ext", "java", "--case-sensitive", "defs", "finduser"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let case_sensitive_json = parse_json(case_sensitive_output);
    assert!(case_sensitive_json["results"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn strict_input_mode_disables_symbol_input_expansion() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/UserService.java"),
        "class UserService { void findUser(Long id) {} }\n",
    )
    .unwrap();

    let strict_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--ext",
            "java",
            "--input-mode",
            "strict",
            "defs",
            "UserService.findUser(Long)",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let strict_json = parse_json(strict_output);
    assert!(strict_json["results"].as_array().unwrap().is_empty());
    assert!(!strict_json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "query_input_expanded"));
}

#[test]
fn public_json_preserves_input_expansion_caveat_without_internal_stats() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/UserService.java"),
        "class UserService { void findUser(Long id) {} }\n",
    )
    .unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--output",
            "json",
            "--ext",
            "java",
            "defs",
            "UserService.findUser(Long)",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json = parse_json(output);
    assert!(json.get("scanStats").is_none());
    assert!(json["caveats"]
        .as_array()
        .unwrap()
        .iter()
        .any(|caveat| caveat["code"] == "query_input_expanded"
            && caveat["severity"] == "info"
            && caveat["category"] == "capability"));
}
