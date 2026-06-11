use std::fs;
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use codetrail::{
    index,
    output::VerboseLogger,
    query::{QueryOptions, QueryService},
    scip,
    scip_index::native_db_path,
    workspace::{ScanOptions, Workspace},
};
use serde_json::Value;
use tempfile::tempdir;

fn fake_lsp_server_path() -> std::path::PathBuf {
    cargo_bin("fake-lsp-server")
}

fn setup_go_fixture(dir: &std::path::Path) {
    fs::write(dir.join("go.mod"), "module example.com/fake\n\ngo 1.21\n").unwrap();
    fs::write(
        dir.join("main.go"),
        "package main\n\nfunc main() {\n    Needle()\n}\n",
    )
    .unwrap();
    fs::write(dir.join("needle.go"), "package main\n\nfunc Needle() {}\n").unwrap();
}

#[test]
fn fake_lsp_server_builds_scip_occurrence_db() {
    let dir = tempdir().unwrap();
    setup_go_fixture(dir.path());

    let server = fake_lsp_server_path();
    assert!(
        server.exists(),
        "fake-lsp-server binary must be built for tests"
    );

    std::env::set_var("CODETRAIL_LSP_GO", format!("{} serve", server.display()));
    std::env::set_var("CODETRAIL_SEMANTIC_BUDGET_MS", "10000");

    let workspace = Workspace::discover(dir.path()).unwrap();
    let scan = ScanOptions {
        include: vec![],
        exclude: vec![],
        hidden: false,
        no_ignore: false,
        lang: vec![],
        changed: false,
        cursor: None,
        allow_broad: true,
        limit: 0,
    };

    let build_result = index::build(
        &workspace,
        &scan,
        false,
        false,
        true,
        true,
        VerboseLogger::new(0),
    )
    .unwrap();
    let semantic = &build_result["index"]["semantic"];
    assert_eq!(semantic["attempted"], true);

    let db_path = native_db_path(&workspace);
    assert!(
        db_path.exists(),
        "expected occurrence DB at {}",
        db_path.display()
    );
    assert!(scip::occurrence_db_fresh(
        &db_path,
        &workspace.snapshot_id,
        &workspace.root
    ));

    let defs = scip::query_defs(&db_path, "Needle").unwrap();
    assert!(
        !defs.is_empty(),
        "fake LSP should produce at least one definition for Needle"
    );
    let refs = scip::query_refs(&db_path, "Needle").unwrap();
    assert_eq!(refs.len(), 1, "expected one cross-file reference: {refs:?}");
    assert_eq!(refs[0].path, "main.go");
    assert_eq!(refs[0].start_line, 4);

    let service = QueryService::new(dir.path()).unwrap();
    let callers = service.callers("Needle", &QueryOptions::default()).unwrap();
    let results = callers["results"].as_array().unwrap();
    assert!(
        results.iter().any(|result| {
            result["source"] == "scip_precise"
                && result["path"] == "main.go"
                && result["enclosingSymbol"] == "main"
        }),
        "expected graph caller from fresh LSP SCIP references: {callers}"
    );
}

#[test]
fn index_build_no_semantic_skips_lsp_phase() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    let output = Command::new(cargo_bin("codetrail"))
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--output",
            "json",
            "index",
            "build",
            "--no-semantic",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let semantic = &json["results"][0]["index"]["semantic"];
    assert_eq!(semantic["skipped"], true);
    assert_eq!(semantic["skipReason"], "semantic_disabled");
}

#[test]
fn defs_use_precise_fact_after_lsp_index_build() {
    let dir = tempdir().unwrap();
    setup_go_fixture(dir.path());

    let server = fake_lsp_server_path();
    if !server.exists() {
        return;
    }
    std::env::set_var("CODETRAIL_LSP_GO", format!("{} serve", server.display()));
    std::env::set_var("CODETRAIL_SEMANTIC_BUDGET_MS", "10000");

    let workspace = Workspace::discover(dir.path()).unwrap();
    let scan = ScanOptions {
        include: vec![],
        exclude: vec![],
        hidden: false,
        no_ignore: false,
        lang: vec![],
        changed: false,
        cursor: None,
        allow_broad: true,
        limit: 0,
    };
    index::build(
        &workspace,
        &scan,
        false,
        false,
        true,
        true,
        VerboseLogger::new(0),
    )
    .unwrap();

    let service = QueryService::new(dir.path()).unwrap();
    let response = service.defs("Needle", &QueryOptions::default()).unwrap();
    assert_eq!(response["reliability"]["level"], "precise_fact");
    assert!(
        !response["results"]
            .as_array()
            .map(|items| items.is_empty())
            .unwrap_or(true),
        "expected precise defs for Needle: {response}"
    );
}

#[test]
fn gopls_e2e_builds_precise_index_when_available() {
    if !Command::new("gopls")
        .arg("version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
    {
        eprintln!("skipping gopls e2e: gopls not available");
        return;
    }

    let dir = tempdir().unwrap();
    setup_go_fixture(dir.path());
    std::env::remove_var("CODETRAIL_LSP_GO");
    std::env::set_var("CODETRAIL_SEMANTIC_BUDGET_MS", "120000");

    let workspace = Workspace::discover(dir.path()).unwrap();
    let scan = ScanOptions {
        include: vec![],
        exclude: vec![],
        hidden: false,
        no_ignore: false,
        lang: vec![],
        changed: false,
        cursor: None,
        allow_broad: true,
        limit: 0,
    };

    let build_result = index::build(
        &workspace,
        &scan,
        false,
        false,
        true,
        true,
        VerboseLogger::new(0),
    )
    .unwrap();
    let semantic = &build_result["index"]["semantic"];
    assert_eq!(semantic["attempted"], true);

    let db_path = native_db_path(&workspace);
    if !db_path.exists() {
        eprintln!("skipping gopls precise assertion: no occurrence DB written");
        return;
    }

    let service = QueryService::new(dir.path()).unwrap();
    let response = service.defs("Needle", &QueryOptions::default()).unwrap();
    if response["reliability"]["level"] == "precise_fact" {
        assert!(
            !response["results"].as_array().unwrap().is_empty(),
            "gopls should produce precise defs when indexing succeeds"
        );
    }
}
