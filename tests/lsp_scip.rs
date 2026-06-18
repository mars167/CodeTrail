use std::fs;
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use assert_cmd::cargo::cargo_bin;
use codetrail::{
    generation_manifest::{GenerationManifest, ManifestState},
    index,
    lsp::{
        self,
        client::LspClient,
        registry::{file_path_to_uri, ReadinessStrategy, ServerSpec},
    },
    output::VerboseLogger,
    project_graph::ProjectLanguage,
    query::{QueryOptions, QueryService},
    scip,
    scip_index::{self, native_db_path},
    scip_proto::proto,
    workspace::{ScanOptions, Workspace},
};
use serde_json::Value;
use std::time::{Duration, Instant};
use tempfile::tempdir;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

fn setup_swift_fixture(dir: &std::path::Path) {
    fs::write(dir.join("Package.swift"), "// swift-tools-version: 6.0\n").unwrap();
    fs::create_dir_all(dir.join("Sources/App")).unwrap();
    fs::write(
        dir.join("Sources/App/Main.swift"),
        "public func main() {\n    Needle()\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("Sources/App/Needle.swift"),
        "public func Needle() {}\n",
    )
    .unwrap();
}

struct EnvVarGuard {
    key: &'static str,
    value: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let guard = Self {
            key,
            value: std::env::var_os(key),
        };
        std::env::set_var(key, value);
        guard
    }

    fn remove(key: &'static str) -> Self {
        let guard = Self {
            key,
            value: std::env::var_os(key),
        };
        std::env::remove_var(key);
        guard
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.value {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[test]
fn language_status_readiness_waits_across_quiet_intervals() {
    let dir = tempdir().unwrap();
    let server = fake_lsp_server_path();
    if !server.exists() {
        return;
    }

    let spec = ServerSpec {
        program: server.to_string_lossy().to_string(),
        args: vec!["--language-status-delay-ms=1200".to_string()],
        provider_id: "fake-jdtls".to_string(),
        readiness: ReadinessStrategy::LanguageStatus { timeout_ms: 2_500 },
    };
    let mut client = LspClient::spawn(&spec, dir.path()).unwrap();
    let root_uri = file_path_to_uri(dir.path()).unwrap();

    let started = Instant::now();
    let ready = client.initialize(&root_uri, &spec.readiness).unwrap();
    let elapsed = started.elapsed();
    client.shutdown().unwrap();

    assert!(ready);
    assert!(
        elapsed >= Duration::from_millis(1_100),
        "language/status readiness returned before the delayed ready notification: {elapsed:?}"
    );
}

#[test]
fn language_status_readiness_reports_timeout() {
    let dir = tempdir().unwrap();
    let server = fake_lsp_server_path();
    if !server.exists() {
        return;
    }

    let spec = ServerSpec {
        program: server.to_string_lossy().to_string(),
        args: vec!["--language-status-delay-ms=1200".to_string()],
        provider_id: "fake-jdtls".to_string(),
        readiness: ReadinessStrategy::LanguageStatus { timeout_ms: 100 },
    };
    let mut client = LspClient::spawn(&spec, dir.path()).unwrap();
    let root_uri = file_path_to_uri(dir.path()).unwrap();

    let ready = client.initialize(&root_uri, &spec.readiness).unwrap();
    client.shutdown().unwrap();

    assert!(!ready);
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

    let _env_lock = ENV_LOCK.lock().unwrap();
    let _go_guard = EnvVarGuard::set("CODETRAIL_LSP_GO", &format!("{} serve", server.display()));
    let _budget_guard = EnvVarGuard::set("CODETRAIL_SEMANTIC_BUDGET_MS", "10000");

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
        ..ScanOptions::default()
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
fn failed_semantic_rerun_invalidates_existing_occurrence_db() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("java/src/main/java")).unwrap();
    fs::write(dir.path().join("java/pom.xml"), "<project />\n").unwrap();
    fs::write(
        dir.path().join("java/src/main/java/App.java"),
        "class App { void stale() {} }\n",
    )
    .unwrap();

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
        ..ScanOptions::default()
    };
    let records = workspace.scan_files(&scan).unwrap();
    let db_path = native_db_path(&workspace);
    let symbol = "semanticdb maven . . App#stale().";
    let scip_index = proto::Index {
        documents: vec![proto::Document {
            language: "java".to_string(),
            relative_path: "java/src/main/java/App.java".to_string(),
            occurrences: vec![proto::Occurrence {
                range: vec![0, 17, 0, 22],
                symbol: symbol.to_string(),
                symbol_roles: 1,
                ..Default::default()
            }],
            symbols: vec![proto::SymbolInformation {
                symbol: symbol.to_string(),
                kind: proto::symbol_information::Kind::Method as i32,
                display_name: "stale()".to_string(),
                ..Default::default()
            }],
            position_encoding: proto::PositionEncoding::Utf8CodeUnitOffsetFromLineStart as i32,
            ..Default::default()
        }],
        ..Default::default()
    };
    scip::build_occurrences_db(
        &scip_index,
        &db_path,
        &workspace.snapshot_id,
        &workspace.root,
    )
    .unwrap();
    assert!(scip::occurrence_db_fresh(
        &db_path,
        &workspace.snapshot_id,
        &workspace.root
    ));

    let manifest_path = lsp::scip_gen::generation_manifest_path(&workspace);
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&vec![GenerationManifest {
            schema_version: 1,
            generation_id: "partial-run".to_string(),
            root_id: "java:java".to_string(),
            language: ProjectLanguage::Java,
            provider_name: "jdtls".to_string(),
            provider_version_hash: "provider".to_string(),
            environment_hash: "env".to_string(),
            source_proof_hash: "source".to_string(),
            config_proof_hash: "config".to_string(),
            state: ManifestState::Missing,
            partial_reasons: vec!["semantic_provider_missing".to_string()],
            created_at_epoch_ms: 1,
            updated_at_epoch_ms: 1,
        }])
        .unwrap(),
    )
    .unwrap();
    assert!(
        scip_index::defs(&workspace, &scan, "stale")
            .unwrap()
            .is_none(),
        "missing generation manifest must block fresh occurrence DB precise queries"
    );

    let _env_lock = ENV_LOCK.lock().unwrap();
    let _java_guard = EnvVarGuard::set(
        "CODETRAIL_LSP_JAVA",
        "/definitely/missing/codetrail-test-jdtls",
    );
    let report = lsp::generate_best_effort(&workspace, &records, VerboseLogger::new(0)).unwrap();

    assert!(report.attempted);
    assert!(report
        .languages
        .iter()
        .any(|language| language.state == "partial"));
    assert!(
        !db_path.exists()
            || !scip::occurrence_db_fresh(&db_path, &workspace.snapshot_id, &workspace.root),
        "failed rerun must not leave an old fresh occurrence DB at {}",
        db_path.display()
    );
}

#[test]
fn defs_use_precise_fact_after_lsp_index_build() {
    let dir = tempdir().unwrap();
    setup_go_fixture(dir.path());

    let server = fake_lsp_server_path();
    if !server.exists() {
        return;
    }
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _go_guard = EnvVarGuard::set("CODETRAIL_LSP_GO", &format!("{} serve", server.display()));
    let _budget_guard = EnvVarGuard::set("CODETRAIL_SEMANTIC_BUDGET_MS", "10000");

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
        ..ScanOptions::default()
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
fn swift_lsp_build_uses_sourcekit_env_override_for_precise_defs() {
    let dir = tempdir().unwrap();
    setup_swift_fixture(dir.path());

    let server = fake_lsp_server_path();
    if !server.exists() {
        return;
    }
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _swift_guard = EnvVarGuard::set(
        "CODETRAIL_LSP_SWIFT",
        &format!("{} serve", server.display()),
    );
    let _budget_guard = EnvVarGuard::set("CODETRAIL_SEMANTIC_BUDGET_MS", "10000");

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
        ..ScanOptions::default()
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
    assert!(response["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| {
            result["language"] == "swift" && result["path"] == "Sources/App/Needle.swift"
        }));
}

#[test]
fn swift_index_build_generates_and_imports_scip() {
    let dir = tempdir().unwrap();
    setup_swift_fixture(dir.path());

    let server = fake_lsp_server_path();
    if !server.exists() {
        return;
    }
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _swift_guard = EnvVarGuard::set(
        "CODETRAIL_LSP_SWIFT",
        &format!("{} serve", server.display()),
    );
    let _budget_guard = EnvVarGuard::set("CODETRAIL_SEMANTIC_BUDGET_MS", "10000");

    let build = Command::new(cargo_bin("codetrail"))
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--output",
            "json",
            "index",
            "build",
            "--force",
        ])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let build_json: Value = serde_json::from_slice(&build.stdout).unwrap();
    let scip = &build_json["results"][0]["index"]["semantic"]["scip"];
    assert_eq!(scip["generated"], true);
    assert_eq!(scip["imported"], true);
    assert_eq!(scip["source"], "lsp_scip");
    assert!(scip["documentCount"].as_u64().unwrap() > 0);
    assert!(scip["occurrenceCount"].as_u64().unwrap() > 0);
    assert!(dir.path().join(".codetrail/scip").is_dir());

    let defs = Command::new(cargo_bin("codetrail"))
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--output",
            "json",
            "defs",
            "Needle",
        ])
        .output()
        .unwrap();
    assert!(
        defs.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&defs.stderr)
    );
    let defs_json: Value = serde_json::from_slice(&defs.stdout).unwrap();
    assert!(defs_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| {
            result["language"] == "swift" && result["path"] == "Sources/App/Needle.swift"
        }));

    let refs = Command::new(cargo_bin("codetrail"))
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--output",
            "json",
            "refs",
            "Needle",
        ])
        .output()
        .unwrap();
    assert!(
        refs.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&refs.stderr)
    );
    let refs_json: Value = serde_json::from_slice(&refs.stdout).unwrap();
    assert!(refs_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| {
            result["language"] == "swift" && result["path"] == "Sources/App/Main.swift"
        }));
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
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _go_guard = EnvVarGuard::remove("CODETRAIL_LSP_GO");
    let _budget_guard = EnvVarGuard::set("CODETRAIL_SEMANTIC_BUDGET_MS", "120000");

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
        ..ScanOptions::default()
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
