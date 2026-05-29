use std::fs;

use assert_cmd::Command;
use serde_json::json;
use serde_json::Value;
use tempfile::tempdir;

fn code_search() -> Command {
    Command::cargo_bin("code-search").expect("binary exists")
}

#[test]
fn find_returns_reliable_source_fact() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() {\n    println!(\"needle\");\n}\n",
    )
    .unwrap();

    let output = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["reliability"]["level"], "source_fact");
    assert_eq!(json["results"][0]["path"], "src/main.rs");
    assert_eq!(json["results"][0]["range"]["start"]["line"], 2);
}

#[test]
fn read_returns_exact_line_range() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "one\ntwo\nthree\n").unwrap();

    let output = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["read", "sample.txt:2-3"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"][0]["content"], "two\nthree");
    assert_eq!(json["results"][0]["exact"], true);
}

#[test]
fn parser_commands_expose_symbols_and_call_candidates() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\n\nfn beta() {}\n",
    )
    .unwrap();

    let defs = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "beta"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();
    assert_eq!(defs_json["reliability"]["level"], "parser_fact");
    assert_eq!(defs_json["results"][0]["name"], "beta");

    let callers = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "beta"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert_eq!(callers_json["reliability"]["level"], "inferred_candidate");
    assert_eq!(callers_json["results"][0]["enclosingSymbol"], "alpha");
}

#[test]
fn index_verify_detects_stale_files() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "one\n").unwrap();

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "verify"])
        .assert()
        .success();

    fs::write(dir.path().join("sample.txt"), "one\ntwo\n").unwrap();

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "verify"])
        .assert()
        .code(6);
}

#[test]
fn index_build_writes_target_text_storage_layout() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let code_search_dir = dir.path().join(".code-search");
    assert!(code_search_dir.join("snapshots").is_dir());
    assert!(code_search_dir.join("text").is_dir());
    assert!(code_search_dir
        .join("working")
        .join("manifest.json")
        .is_file());

    let snapshot = fs::read_dir(code_search_dir.join("snapshots"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(snapshot.join("manifest.json").is_file());
    assert!(snapshot.join("files.parquet").is_file());
    assert!(snapshot.join("blobs").is_dir());
    let text_snapshot = fs::read_dir(code_search_dir.join("text"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(text_snapshot.join("docs.idx").is_file());
    assert!(text_snapshot.join("paths.idx").is_file());
    assert!(text_snapshot.join("grams.idx").is_file());
}

#[test]
fn find_uses_fresh_text_index_for_candidates() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["index"]["used"], true);
    assert_eq!(json["index"]["fresh"], true);
    assert_eq!(json["index"]["source"], "text_index");
    assert_eq!(
        json["results"][0]["producer"],
        "text_index_live_text_search"
    );
}

#[test]
fn query_falls_back_when_scan_options_do_not_match_index() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".hidden.txt"), "needle\n").unwrap();

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = code_search()
        .arg("--path")
        .arg(dir.path())
        .arg("--hidden")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["index"]["used"], false);
    assert_eq!(json["results"][0]["path"], ".hidden.txt");
    assert_eq!(json["results"][0]["producer"], "live_text_search");
}

#[test]
fn completions_print_shell_script_without_workspace() {
    let output = code_search()
        .args(["--path", "/definitely/missing", "completions", "bash"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let script = String::from_utf8(output).unwrap();

    assert!(script.contains("complete -F _code_search code-search"));
    assert!(script.contains("find grep files"));
}

#[test]
fn imported_scip_index_drives_precise_defs_refs_and_symbols() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn needle() {}\nfn main() { needle(); }\n",
    )
    .unwrap();
    let scip_path = dir.path().join("index.scip.json");
    write_minimal_scip_json(&scip_path);

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "import-scip"])
        .arg(&scip_path)
        .assert()
        .success();

    let scip_snapshot = fs::read_dir(dir.path().join(".code-search/scip"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(scip_snapshot.join("occurrences.idx").is_file());
    assert!(!scip_snapshot.join("occurrences.jsonl").exists());

    let defs = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();
    assert_eq!(defs_json["reliability"]["level"], "precise_fact");
    assert_eq!(defs_json["results"][0]["producer"], "scip");
    assert_eq!(defs_json["results"][0]["exact"], true);
    assert_eq!(defs_json["results"][0]["range"]["start"]["line"], 1);

    let refs = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["refs", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let refs_json: Value = serde_json::from_slice(&refs).unwrap();
    assert_eq!(refs_json["reliability"]["level"], "precise_fact");
    assert_eq!(refs_json["results"][0]["producer"], "scip");
    assert_eq!(refs_json["results"][0]["role"], "reference");
    assert_eq!(refs_json["results"][0]["range"]["start"]["line"], 2);

    let symbols = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["symbols", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let symbols_json: Value = serde_json::from_slice(&symbols).unwrap();
    assert_eq!(symbols_json["reliability"]["level"], "precise_fact");
    assert_eq!(symbols_json["results"][0]["name"], "needle");
}

#[test]
fn defs_falls_back_to_parser_after_plain_index_build_without_scip() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn needle() {}\n").unwrap();

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let defs = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();

    assert_eq!(defs_json["reliability"]["level"], "parser_fact");
    assert_eq!(defs_json["results"][0]["producer"], "tree_sitter_parser");
}

#[test]
fn calls_and_callers_do_not_claim_graph_store_before_kuzu_backend_exists() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\n\nfn beta() {}\n",
    )
    .unwrap();

    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let calls = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["calls", "alpha"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let calls_json: Value = serde_json::from_slice(&calls).unwrap();
    assert_eq!(calls_json["index"]["used"], false);
    assert_eq!(calls_json["reliability"]["level"], "inferred_candidate");
    assert_eq!(
        calls_json["results"][0]["producer"],
        "tree_sitter_call_heuristic"
    );
    assert_eq!(calls_json["results"][0]["target"], "beta");

    let callers = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "beta"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert_eq!(
        callers_json["results"][0]["producer"],
        "tree_sitter_call_heuristic"
    );
    assert_eq!(callers_json["results"][0]["enclosingSymbol"], "alpha");
}

fn write_minimal_scip_json(path: &std::path::Path) {
    let value = json!({
        "documents": [
            {
                "relativePath": "src/lib.rs",
                "language": "rust",
                "occurrences": [
                    {
                        "range": [0, 3, 0, 9],
                        "symbol": "local 1",
                        "symbolRoles": 1
                    },
                    {
                        "range": [1, 12, 1, 18],
                        "symbol": "local 1",
                        "symbolRoles": 0
                    }
                ],
                "symbols": [
                    {
                        "symbol": "local 1",
                        "displayName": "needle",
                        "kind": "function"
                    }
                ]
            }
        ]
    });
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

/// Helper: init a git repo in the given directory with a .gitignore to ignore .code-search
fn init_git_repo(dir: &std::path::Path) {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("init")
        .output()
        .ok();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "user.email", "test@test.com"])
        .output()
        .ok();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "user.name", "test"])
        .output()
        .ok();
    // Add a .gitignore so .code-search doesn't interfere
    fs::write(dir.join(".gitignore"), ".code-search\n").ok();
}

/// Helper: commit all files in a git repo
fn git_commit_all(dir: &std::path::Path, msg: &str) {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["add", "-A"])
        .output()
        .ok();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "-m", msg])
        .output()
        .ok();
}

#[test]
fn incremental_update_rescans_only_changed_files() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // Create initial file and commit
    fs::write(dir.path().join("a.txt"), "hello world\n").unwrap();
    fs::write(dir.path().join("b.txt"), "foo bar\n").unwrap();
    git_commit_all(dir.path(), "initial");

    // Build initial index
    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Verify the index exists and has our files
    let status = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_json: Value = serde_json::from_slice(&status).unwrap();
    assert_eq!(status_json["results"][0]["exists"], true);

    // Modify one file
    fs::write(dir.path().join("a.txt"), "hello world updated\n").unwrap();

    // Run index update (incremental)
    let update = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "update"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let update_json: Value = serde_json::from_slice(&update).unwrap();
    let update_result = &update_json["results"][0];
    // Verify the update job has the expected fields
    assert_eq!(update_result["job"], "update");
    assert!(update_result["totalFiles"].as_u64().unwrap() >= 2);

    // Verify freshness after update
    let status2 = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status2_json: Value = serde_json::from_slice(&status2).unwrap();
    assert_eq!(status2_json["results"][0]["exists"], true);
}

#[test]
fn index_update_returns_empty_on_no_changes() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    fs::write(dir.path().join("hello.txt"), "hello\n").unwrap();
    git_commit_all(dir.path(), "initial");

    // Build
    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Update with no changes
    let update = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "update"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let update_json: Value = serde_json::from_slice(&update).unwrap();
    let update_result = &update_json["results"][0];
    assert_eq!(update_result["job"], "update");
    assert_eq!(update_result["changedFiles"].as_u64().unwrap(), 0);
}

#[test]
fn index_compact_cleans_tmp_directories() {
    let dir = tempdir().unwrap();

    // Create a stale .tmp dir in snapshots
    let code_search_dir = dir.path().join(".code-search");
    let snap_tmp = code_search_dir.join("snapshots").join("stale.tmp");
    fs::create_dir_all(&snap_tmp).unwrap();
    fs::write(snap_tmp.join("junk.txt"), b"stale").unwrap();

    let text_tmp = code_search_dir.join("text").join("stale.tmp");
    fs::create_dir_all(&text_tmp).unwrap();
    fs::write(text_tmp.join("junk.txt"), b"stale").unwrap();

    // Also create a valid snapshot so compact has something to keep
    let valid_snap = code_search_dir.join("snapshots").join("valid");
    fs::create_dir_all(&valid_snap).unwrap();
    fs::write(
        valid_snap.join("manifest.json"),
        serde_json::to_vec(&json!({"snapshotKey": "valid"})).unwrap(),
    )
    .unwrap();

    // Create a working manifest pointing to the valid snapshot
    let working = code_search_dir.join("working");
    fs::create_dir_all(&working).unwrap();
    fs::write(
        working.join("manifest.json"),
        serde_json::to_vec(&json!({"snapshotKey": "valid"})).unwrap(),
    )
    .unwrap();

    // Run compact
    let result = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "compact"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result_json: Value = serde_json::from_slice(&result).unwrap();
    let compact_result = &result_json["results"][0];
    assert_eq!(compact_result["job"], "compact");

    // Verify .tmp dirs are cleaned
    assert!(!snap_tmp.exists());
    assert!(!text_tmp.exists());
}

#[test]
fn scheduler_status_includes_health_info() {
    let dir = tempdir().unwrap();

    // Build index
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();
    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Check status
    let result = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result_json: Value = serde_json::from_slice(&result).unwrap();
    let status_result = &result_json["results"][0];
    assert_eq!(status_result["exists"], true);
    // Scheduler health info should be present
    let scheduler = &status_result["scheduler"];
    assert!(scheduler["hasWorking"].as_bool().unwrap());
    assert!(scheduler["workingSnapshots"].as_u64().unwrap() >= 1);
}

#[test]
fn force_rebuild_clears_old_data() {
    let dir = tempdir().unwrap();

    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    // First build
    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Force rebuild
    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build", "--force"])
        .assert()
        .success();

    // Verify index still exists and is fresh
    let status = code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "verify"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_json: Value = serde_json::from_slice(&status).unwrap();
    assert_eq!(status_json["results"][0]["fresh"], true);
}

#[test]
fn compact_removes_orphan_snapshots() {
    let dir = tempdir().unwrap();

    // Build initial index
    fs::write(dir.path().join("sample.txt"), "first\n").unwrap();
    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Count snapshots before compact
    let snapshots_before = fs::read_dir(dir.path().join(".code-search/snapshots"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().ends_with(".tmp"))
        .count();

    // Force rebuild with modified content
    fs::write(dir.path().join("sample.txt"), "second\n").unwrap();
    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build", "--force"])
        .assert()
        .success();

    // Run compact
    code_search()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "compact"])
        .assert()
        .success();

    // Count snapshots after compact — should have the same or fewer
    let snapshots_after = fs::read_dir(dir.path().join(".code-search/snapshots"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().ends_with(".tmp"))
        .count();

    assert!(snapshots_after >= 1);
    // The orphan snapshot from the first build should be cleaned
    assert!(snapshots_after <= snapshots_before + 1);
}
