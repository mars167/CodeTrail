use std::fs;

use assert_cmd::Command;
use serde_json::json;
use serde_json::Value;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Helpers shared by security regression tests
// ---------------------------------------------------------------------------

/// Build a minimal valid pack manifest JSON blob.
fn pack_manifest_bytes() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "schemaVersion": 1u32,
        "snapshot_id": "test:abc123",
        "snapshotKey": "test_abc123",
        "timestamp": 0u64,
        "source": "packed_remote",
        "toolVersion": "0.0.0",
        "originalRepoRoot": "/tmp/test-repo",
        "head": null,
        "dirty": false,
        "fileCount": 0u32,
        "scanOptions": {
            "include": [],
            "exclude": [],
            "hidden": false,
            "noIgnore": false,
            "lang": [],
            "changed": false
        }
    }))
    .unwrap()
}

/// Write a raw (POSIX ustar) tar header block without path validation.
///
/// This bypasses the `tar` crate's writer-side safety checks so that tests can
/// craft archives containing `..` segments or absolute paths.
fn raw_tar_header(path: &str, content_len: usize) -> [u8; 512] {
    let mut header = [0u8; 512];

    // name field: bytes 0-99  (truncated, null-terminated by the zero array)
    let path_bytes = path.as_bytes();
    let copy_len = path_bytes.len().min(99);
    header[..copy_len].copy_from_slice(&path_bytes[..copy_len]);

    // mode: "0000644\0"
    header[100..108].copy_from_slice(b"0000644\0");
    // uid / gid
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    // size (11 octal digits + null)
    let size_str = format!("{:011o}\0", content_len);
    header[124..136].copy_from_slice(size_str.as_bytes());
    // mtime
    header[136..148].copy_from_slice(b"00000000000\0");
    // checksum placeholder — spaces
    header[148..156].copy_from_slice(b"        ");
    // typeflag: regular file
    header[156] = b'0';
    // ustar magic + version
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");

    // Compute and write checksum
    let sum: u32 = header.iter().map(|&b| b as u32).sum();
    let cksum = format!("{:06o}\0 ", sum);
    header[148..156].copy_from_slice(cksum.as_bytes());

    header
}

/// Build a `.tar.gz` archive whose paths are written as raw bytes,
/// bypassing the `tar` crate's writer-side path validation.
///
/// `extra_entries` is `(path_in_tar, content)`.  Each entry is also
/// added to `checksums.txt` so that the `unpack()` checksum validation
/// passes — letting the path-safety guard trigger instead.
fn build_raw_tar_gz(extra_entries: &[(&str, &[u8])]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let manifest = pack_manifest_bytes();

    let mut checksums = String::new();
    let mut manifest_hasher = Sha256::new();
    manifest_hasher.update(&manifest);
    checksums.push_str(&format!(
        "{:x}  manifest.json\n",
        manifest_hasher.finalize()
    ));
    for (path, content) in extra_entries {
        let mut hasher = Sha256::new();
        hasher.update(content);
        let hash = format!("{:x}", hasher.finalize());
        checksums.push_str(&format!("{hash}  {path}\n"));
    }

    // Assemble raw tar bytes
    let mut tar_bytes = Vec::new();

    let mut write_entry = |path: &str, data: &[u8]| {
        tar_bytes.extend_from_slice(&raw_tar_header(path, data.len()));
        tar_bytes.extend_from_slice(data);
        let pad = (512 - (data.len() % 512)) % 512;
        tar_bytes.extend(std::iter::repeat(0u8).take(pad));
    };

    write_entry("manifest.json", &manifest);
    for (path, content) in extra_entries {
        write_entry(path, content);
    }
    write_entry("checksums.txt", checksums.as_bytes());

    // Two zero blocks = end-of-archive
    tar_bytes.extend(std::iter::repeat(0u8).take(1024));

    // Gzip compress
    let mut gz = Vec::new();
    {
        let mut enc = GzEncoder::new(&mut gz, Compression::default());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap();
    }
    gz
}

/// Build a safe `.tar.gz` archive using the `tar` crate's validated writer.
/// Suitable for tests that only need well-formed paths (e.g. entry-count limits).
fn build_safe_tar_gz(extra_entries: &[(&str, &[u8])]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    use sha2::{Digest, Sha256};

    let manifest = pack_manifest_bytes();

    let mut checksums = String::new();
    for (path, content) in extra_entries {
        let mut hasher = Sha256::new();
        hasher.update(content);
        let hash = format!("{:x}", hasher.finalize());
        checksums.push_str(&format!("{hash}  {path}\n"));
    }

    let mut archive_data = Vec::new();
    let encoder = GzEncoder::new(&mut archive_data, Compression::default());
    let mut tar = tar::Builder::new(encoder);

    let append = |tar: &mut tar::Builder<_>, name: &str, data: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_path(name).expect("set_path");
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, data).expect("append");
    };

    append(&mut tar, "manifest.json", &manifest);
    for (path, content) in extra_entries {
        append(&mut tar, path, content);
    }
    append(&mut tar, "checksums.txt", checksums.as_bytes());

    let enc = tar.into_inner().unwrap();
    enc.finish().unwrap();
    archive_data
}

fn codetrail() -> Command {
    let mut command = raw_codetrail();
    command
        .env("CODETRAIL_INTERNAL_JSON", "1")
        .arg("--output")
        .arg("json");
    command
}

fn raw_codetrail() -> Command {
    Command::cargo_bin("codetrail").expect("binary exists")
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn init_git_repo(path: &std::path::Path) {
    std::process::Command::new("git")
        .arg("init")
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["config", "user.email", "test@test.com"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["config", "user.name", "Test"])
        .output()
        .unwrap();
}

fn assert_no_public_caveats(json: &Value) {
    assert!(
        json.get("caveats").is_none(),
        "public output must not include caveats: {json}"
    );
}

fn assert_no_public_reliability_labels(json: &Value) {
    let text = serde_json::to_string(json).unwrap();
    for label in [
        "source_fact",
        "precise_fact",
        "parser_fact",
        "inferred_candidate",
        "freshness",
        "remote_verified",
        "remote_unverified",
    ] {
        assert!(
            !text.contains(label),
            "public output must not include reliability label {label}: {text}"
        );
    }
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

    let output = codetrail()
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
    assert_eq!(json["schemaVersion"], "1.0");
    assert_eq!(json["reliability"]["level"], "source_fact");
    assert_eq!(json["query"]["normalized"], true);
    assert_eq!(json["results"][0]["path"], "src/main.rs");
    assert_eq!(json["results"][0]["range"]["start"]["line"], 2);
}

#[test]
fn schema_contract_covers_core_commands_and_errors() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    for args in [
        vec!["files", "main"],
        vec!["glob", "**/*.rs"],
        vec!["status"],
        vec!["changed"],
        vec!["index", "status"],
    ] {
        let output = codetrail()
            .arg("--path")
            .arg(dir.path())
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["schemaVersion"], "1.0");
        assert_eq!(json["query"]["normalized"], true);
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "main", "--mode", "bogus"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["schemaVersion"], "1.0");
    assert_eq!(json["error"]["code"], "cli_usage_error");
}

#[test]
fn index_build_text_output_suppresses_progress_when_stderr_is_not_tty() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    let assert = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("Indexed 1 files"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        stdout.contains("Backend: lancedb"),
        "unexpected stdout: {stdout}"
    );
    assert!(stderr.is_empty(), "progress leaked to stderr: {stderr:?}");
}

#[test]
fn hooks_text_output_reports_state() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    let first_install = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "text", "hooks", "install"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first_install = String::from_utf8(first_install).unwrap();
    assert!(
        first_install.contains("pre-commit: created"),
        "unexpected hooks install output: {first_install}"
    );
    assert!(
        first_install.contains(".git/hooks/pre-commit"),
        "unexpected hooks install output: {first_install}"
    );

    let second_install = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "text", "hooks", "install"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second_install = String::from_utf8(second_install).unwrap();
    assert!(
        second_install.contains("pre-commit: unchanged"),
        "unexpected hooks reinstall output: {second_install}"
    );

    let status = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "text", "hooks", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status = String::from_utf8(status).unwrap();
    assert!(
        status.contains("pre-commit: installed"),
        "unexpected hooks status output: {status}"
    );

    let uninstall = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "text", "hooks", "uninstall"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let uninstall = String::from_utf8(uninstall).unwrap();
    assert!(
        uninstall.contains("pre-commit: removed"),
        "unexpected hooks uninstall output: {uninstall}"
    );

    let status_after_uninstall = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "text", "hooks", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_after_uninstall = String::from_utf8(status_after_uninstall).unwrap();
    assert!(
        status_after_uninstall.contains("pre-commit: missing"),
        "unexpected hooks status output after uninstall: {status_after_uninstall}"
    );
}

#[test]
fn index_build_reports_scip_java_install_help_with_parser_fallback() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("pom.xml"), "<project></project>\n").unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/App.java"),
        "package example; class App { void run() {} }\n",
    )
    .unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .env("PATH", "")
        .env_remove("CODETRAIL_SCIP_JAVA")
        .args(["--output", "json", "index", "build"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let help = json["results"][0]["index"]["semantic"]["providerInstallHelp"]
        .as_array()
        .expect("provider install help");
    assert!(help.iter().any(|item| {
        item["language"] == "java"
            && item["provider"] == "scip-java"
            && item["envKey"] == "CODETRAIL_SCIP_JAVA"
            && item["fallback"] == "tree_sitter_parser"
    }));
}

#[test]
fn index_build_exposes_mybatis_mapper_xml_as_config_facts() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("pom.xml"), "<project></project>\n").unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/com/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/com/example/SysUserMapper.java"),
        "package com.example;\npublic interface SysUserMapper { SysUser selectUserByLoginName(String userName); }\nclass SysUser {}\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src/main/resources/mapper/system")).unwrap();
    fs::write(
        dir.path()
            .join("src/main/resources/mapper/system/SysUserMapper.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<mapper namespace="com.example.SysUserMapper">
  <resultMap id="SysUserResult" type="com.example.SysUser">
    <id property="userId" column="user_id"/>
  </resultMap>
  <select id="selectUserByLoginName" parameterType="String" resultMap="SysUserResult">
    select user_id, login_name from sys_user where login_name = #{userName}
  </select>
</mapper>
"#,
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build", "--no-semantic"])
        .assert()
        .success();

    let status_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    let languages = status["results"][0]["indexedLanguages"]
        .as_array()
        .expect("indexed languages");
    assert!(languages
        .iter()
        .any(|language| language["language"] == "xml"
            && language["fileCount"].as_u64().unwrap_or(0) >= 1));

    let symbols_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["symbols", "SysUserMapper"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let symbols: Value = serde_json::from_slice(&symbols_output).unwrap();
    assert!(symbols["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "src/main/resources/mapper/system/SysUserMapper.xml"
            && result["kind"] == "mapper_namespace"
            && result["language"] == "xml"
            && result["reliability"] == "config_fact"
    }));

    let defs_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "selectUserByLoginName"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs: Value = serde_json::from_slice(&defs_output).unwrap();
    assert!(defs["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "src/main/resources/mapper/system/SysUserMapper.xml"
            && result["kind"] == "mapper_statement"
            && result["name"] == "com.example.SysUserMapper.selectUserByLoginName"
            && result["role"] == "definition"
            && result["valuePreview"] == "select"
    }));

    let text_output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "selectUserByLoginName"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(text_output).unwrap();
    assert!(text.contains("mapper_statement com.example.SysUserMapper.selectUserByLoginName"));
    assert!(!text.contains("mapper_statementcom.example.SysUserMapper.selectUserByLoginName"));
}

#[test]
fn precise_scip_results_include_matching_mybatis_xml_config_facts() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("pom.xml"), "<project></project>\n").unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/com/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/com/example/SysUserMapper.java"),
        "package com.example;\npublic interface SysUserMapper {\n    SysUser selectUserByLoginName(String userName);\n}\nclass UseMapper { SysUserResult result; }\nclass SysUser {}\nclass SysUserResult {}\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src/main/resources/mapper/system")).unwrap();
    fs::write(
        dir.path()
            .join("src/main/resources/mapper/system/SysUserMapper.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<mapper namespace="com.example.SysUserMapper">
  <resultMap id="SysUserResult" type="com.example.SysUser">
    <id property="userId" column="user_id"/>
  </resultMap>
  <select id="selectUserByLoginName" parameterType="String" resultMap="SysUserResult">
    select user_id, login_name from sys_user where login_name = #{userName}
  </select>
</mapper>
"#,
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build", "--no-semantic"])
        .assert()
        .success();

    let scip_path = dir.path().join("index.scip");
    write_java_mapper_scip_index(&scip_path);
    build_native_scip_db_from_file(dir.path(), &scip_path);

    let defs_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "selectUserByLoginName"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs: Value = serde_json::from_slice(&defs_output).unwrap();
    assert_eq!(defs["reliability"]["level"], "precise_fact");
    assert!(defs["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "src/main/java/com/example/SysUserMapper.java"
            && result["producer"] == "scip"
            && result["reliability"] == "precise_fact"
    }));
    assert!(defs["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "src/main/resources/mapper/system/SysUserMapper.xml"
            && result["kind"] == "mapper_statement"
            && result["name"] == "com.example.SysUserMapper.selectUserByLoginName"
            && result["layer"] == "config_fact"
            && result["reliability"] == "config_fact"
    }));
    assert_eq!(defs["index"]["source"], "scip_native");
    assert_eq!(defs["index"]["configFacts"]["source"], "config_facts");

    let limited_defs_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "selectUserByLoginName", "--limit", "1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let limited_defs: Value = serde_json::from_slice(&limited_defs_output).unwrap();
    assert_eq!(limited_defs["results"].as_array().unwrap().len(), 1);

    let symbols_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["symbols", "selectUserByLoginName"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let symbols: Value = serde_json::from_slice(&symbols_output).unwrap();
    assert_eq!(symbols["reliability"]["level"], "precise_fact");
    assert!(symbols["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "src/main/resources/mapper/system/SysUserMapper.xml"
            && result["kind"] == "mapper_statement"
            && result["layer"] == "config_fact"
    }));

    let refs_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["refs", "SysUserResult"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let refs: Value = serde_json::from_slice(&refs_output).unwrap();
    assert_eq!(refs["reliability"]["level"], "precise_fact");
    assert!(refs["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "src/main/java/com/example/SysUserMapper.java"
            && result["producer"] == "scip"
            && result["reliability"] == "precise_fact"
    }));
    assert!(refs["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "src/main/resources/mapper/system/SysUserMapper.xml"
            && result["kind"] == "mapper_reference"
            && result["name"] == "com.example.SysUserMapper.SysUserResult"
            && result["layer"] == "config_fact"
            && result["reliability"] == "config_fact"
    }));

    let limited_refs_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["refs", "SysUserResult", "--limit", "1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let limited_refs: Value = serde_json::from_slice(&limited_refs_output).unwrap();
    assert_eq!(limited_refs["results"].as_array().unwrap().len(), 1);
}

#[test]
fn verbose_index_build_emits_diagnostics_to_stderr() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    let output = codetrail()
        .arg("-v")
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout_json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout_json["results"][0]["index"]["used"], true);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("codetrail: command=index build"),
        "missing command diagnostic: {stderr:?}"
    );
    assert!(
        stderr.contains("codetrail: index build: catalog files=1"),
        "missing catalog diagnostic: {stderr:?}"
    );
    assert!(
        stderr.contains("codetrail: index build: writing LanceDB file proofs"),
        "missing LanceDB diagnostic: {stderr:?}"
    );
}

#[test]
fn warnings_are_structured_with_stable_codes() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn helper() {}\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["refs", "helper"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        json["warnings"][0]["code"],
        "precise_scip_index_unavailable"
    );
    assert_eq!(json["warnings"][0]["severity"], "info");
    assert_eq!(json["warnings"][0]["category"], "capability");
    assert!(json["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .contains("fresh SCIP occurrence index"));
}

#[test]
fn public_json_omits_caveats_for_advanced_commands() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\nfn beta() {}\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src/api")).unwrap();
    fs::create_dir_all(dir.path().join("src/web")).unwrap();
    fs::write(
        dir.path().join("src/api/User.java"),
        "public class User {}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/web/User.java"),
        "public class User {}\n",
    )
    .unwrap();

    for args in [
        ["defs", "beta"],
        ["symbols", "beta"],
        ["refs", "beta"],
        ["calls", "alpha"],
        ["callers", "beta"],
    ] {
        let mut assert = raw_codetrail()
            .arg("--path")
            .arg(dir.path())
            .args(["--output", "json"])
            .args(args)
            .assert();
        assert = if args[0] == "refs" {
            assert.code(2)
        } else {
            assert.success()
        };
        let output = assert.get_output().stdout.clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_no_public_caveats(&json);
        assert_no_public_reliability_labels(&json);
    }

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "symbols", "User"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_no_public_caveats(&json);
}

#[test]
fn public_json_keeps_only_results_and_page() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "before\nneedle\nafter\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "--context", "1", "find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let keys = json
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["page", "results"]);
    assert!(json["page"]["nextCursor"].is_null());
    assert_eq!(json["page"]["truncated"], false);
    assert_no_public_caveats(&json);

    let result = &json["results"][0];
    assert_eq!(result["path"], "sample.txt");
    assert!(result.get("readCommand").is_none());
    assert!(result.get("readCommandArgv").is_none());
    assert!(result.get("sourceTarget").is_none());
    assert!(result.get("producer").is_none());
    assert!(result["context"][0].get("truncated").is_none());
    assert!(result["context"][0].get("truncatedReason").is_none());
}

#[test]
fn public_json_symbols_include_code_returns_source_and_relations() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\n\nfn beta() {}\n\nfn caller() {\n    alpha();\n}\n",
    )
    .unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "symbols", "alpha", "--include-code"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let result = &json["results"][0];

    assert_eq!(result["path"], "src/lib.rs");
    assert_eq!(result["source"]["path"], "src/lib.rs");
    assert_eq!(result["source"]["rangeKind"], "body");
    assert_eq!(result["source"]["truncated"], false);
    assert!(result["source"]["content"]
        .as_str()
        .unwrap()
        .contains("beta();"));
    assert!(result.get("sourceTarget").is_none());

    let calls = result["relations"]["calls"].as_array().unwrap();
    assert!(calls.iter().any(|call| call["target"] == "beta"));
    let callers = result["relations"]["callers"].as_array().unwrap();
    assert!(callers
        .iter()
        .any(|caller| caller["enclosingSymbol"] == "caller"));
    assert_eq!(result["relations"]["truncated"], false);
    assert!(calls.iter().all(|call| call.get("fileHash").is_none()));
    assert_no_public_caveats(&json);
}

#[test]
fn public_json_defs_include_code_truncates_large_symbol() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    let one = 1;\n    let two = 2;\n    let three = one + two;\n    println!(\"{}\", three);\n}\n",
    )
    .unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--output",
            "json",
            "defs",
            "alpha",
            "--include-code",
            "--code-max-lines",
            "2",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let source = &json["results"][0]["source"];

    assert_eq!(source["truncated"], true);
    assert_eq!(source["truncatedReason"], "code_max_lines");
    assert_eq!(source["content"].as_str().unwrap().lines().count(), 2);
    assert_eq!(json["page"]["truncated"], true);
    assert_no_public_caveats(&json);
}

#[test]
fn code_context_options_require_include_code() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("lib.rs"), "fn alpha() {}\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--output",
            "json",
            "symbols",
            "alpha",
            "--code-context",
            "1",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["error"]["code"], "cli_usage_error");
    assert_no_public_caveats(&json);
}

#[test]
fn public_json_uses_cursor_without_truncated_caveat_for_limited_pages() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    for path in ["src/a.rs", "src/b.rs", "src/c.rs"] {
        fs::write(dir.path().join(path), "needle\n").unwrap();
    }

    let first_output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "--limit", "1", "find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first: Value = serde_json::from_slice(&first_output).unwrap();
    let cursor = first["page"]["nextCursor"].as_str().unwrap().to_string();

    assert_eq!(first["results"].as_array().unwrap().len(), 1);
    assert_eq!(first["page"]["truncated"], false);
    assert_no_public_caveats(&first);

    let second_output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "--limit", "1", "--cursor"])
        .arg(cursor)
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second: Value = serde_json::from_slice(&second_output).unwrap();

    assert_eq!(second["results"].as_array().unwrap().len(), 1);
    assert_eq!(second["page"]["truncated"], false);
    assert!(second["page"]["nextCursor"].as_str().is_some());
    assert_no_public_caveats(&second);
    assert_ne!(first["results"][0]["path"], second["results"][0]["path"]);
}

#[test]
fn l0_literal_and_regex_modes_are_predictable() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "literal a.b\nregex acb\n").unwrap();

    let find_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "a.b"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let find_json: Value = serde_json::from_slice(&find_output).unwrap();
    assert_eq!(find_json["query"]["mode"], "literal");
    assert_eq!(find_json["results"].as_array().unwrap().len(), 1);
    assert_eq!(find_json["results"][0]["matchText"], "a.b");

    let grep_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["grep", "a.b"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let grep_json: Value = serde_json::from_slice(&grep_output).unwrap();
    let grep_matches: Vec<&str> = grep_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["matchText"].as_str())
        .collect();
    assert_eq!(grep_json["query"]["mode"], "regex");
    assert!(grep_matches.contains(&"a.b"));
    assert!(grep_matches.contains(&"acb"));

    let literal_grep = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["grep", "a.b", "--mode", "literal"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let literal_json: Value = serde_json::from_slice(&literal_grep).unwrap();
    assert_eq!(literal_json["query"]["mode"], "literal");
    assert_eq!(literal_json["results"].as_array().unwrap().len(), 1);
}

#[test]
fn refs_requires_precise_scip_index_and_does_not_text_search() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "struct User;\nfn main() {\n    let user = User;\n    let profile = UserProfile;\n}\n",
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["refs", "User"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"].as_array().unwrap().len(), 0);
    assert_eq!(json["reliability"]["level"], "freshness");
    assert_eq!(
        json["warnings"][0]["code"],
        "precise_scip_index_unavailable"
    );
    assert!(json["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .contains("use ripgrep for textual matches"));
    let source = fs::read_to_string(dir.path().join("src/main.rs")).unwrap();
    assert!(source.contains("let user = User;"));
}

#[test]
fn refs_without_scip_ignores_textual_occurrences_even_when_identifier_exists() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn needle() {\n    needle();\n}\nfn main() {\n    needle();\n}\n",
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["refs", "needle"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let results = json["results"].as_array().unwrap();

    assert!(results.is_empty());
    assert!(json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "precise_scip_index_unavailable"));
}

#[test]
fn find_no_match_returns_structured_next_actions() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "MissingThing"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["results"].as_array().unwrap().len(), 0);
    assert_eq!(json["noMatch"]["reason"], "no_results");
    assert_eq!(json["noMatch"]["query"]["pattern"], "MissingThing");
    assert!(json["noMatch"]["index"]["fallback"].as_bool().is_some());
    assert!(json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "no_match"));
    assert!(json["nextActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["kind"] == "try_regex"
            && action["command"]
                .as_str()
                .unwrap()
                .contains("grep MissingThing")
            && action["command"].as_str().unwrap().contains("--path")));
}

#[test]
fn invalid_regex_is_not_reported_as_no_match() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["grep", "["])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], false);
    assert!(json.get("noMatch").is_none());
    assert_ne!(json["error"]["code"], "no_match");

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "[", "--mode", "regex"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], false);
    assert!(json.get("noMatch").is_none());
    assert_ne!(json["error"]["code"], "no_match");
}

#[test]
fn files_is_path_substring_while_glob_is_strict_glob() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.path().join("src/*.rs"), "literal star path\n").unwrap();

    let files_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["files", "src/*.rs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let files_json: Value = serde_json::from_slice(&files_output).unwrap();
    let files_paths: Vec<&str> = files_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["path"].as_str())
        .collect();
    assert_eq!(files_json["query"]["mode"], "path_substring");
    assert_eq!(files_paths, vec!["src/*.rs"]);

    let glob_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["glob", "src/*.rs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let glob_json: Value = serde_json::from_slice(&glob_output).unwrap();
    let glob_paths: Vec<&str> = glob_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["path"].as_str())
        .collect();
    assert_eq!(glob_json["query"]["mode"], "strict_glob");
    assert!(glob_paths.contains(&"src/main.rs"));
}

#[test]
fn removed_non_index_subcommands_return_usage_errors() {
    let dir = tempdir().unwrap();
    for command in ["list", "tree", "read"] {
        let output = codetrail()
            .arg("--path")
            .arg(dir.path())
            .arg(command)
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["error"]["code"], "cli_usage_error");
    }
}

#[test]
fn lang_scope_filters_find_and_is_echoed() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "let value = \"needle\";\n").unwrap();
    fs::write(dir.path().join("src/app.py"), "value = 'needle'\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--lang")
        .arg("rust")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["query"]["scope"]["lang"], json!(["rust"]));
    assert_eq!(json["results"].as_array().unwrap().len(), 1);
    assert_eq!(json["results"][0]["path"], "src/lib.rs");
}

#[test]
fn lang_scope_filters_symbols() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn alpha() {}\n").unwrap();
    fs::write(dir.path().join("src/app.py"), "def alpha():\n    pass\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--lang")
        .arg("rust")
        .args(["symbols", "alpha"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let paths: Vec<&str> = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["path"].as_str())
        .collect();

    assert_eq!(json["query"]["scope"]["lang"], json!(["rust"]));
    assert_eq!(paths, vec!["src/lib.rs"]);
}

#[test]
fn routes_scans_mainstream_frameworks_across_five_languages() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::create_dir_all(dir.path().join("go")).unwrap();
    fs::create_dir_all(dir.path().join("py")).unwrap();
    fs::create_dir_all(dir.path().join("web")).unwrap();
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/UserController.java"),
        r#"
@RestController
@RequestMapping("/api")
class UserController {
  @GetMapping("/users")
  public String listUsers() { return ""; }
}
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("go/routes.go"),
        r#"
package main
import "net/http"
func mount(r Router) {
  r.GET("/go/gin", handlers.ListGin)
  r.Get("/go/chi", handlers.ListChi)
  r.Method("POST", "/go/chi-method", handlers.PostChi)
  r.HandleFunc("/go/gorilla", handlers.ListGorilla).Methods("GET")
  http.HandleFunc("/go/http", handlers.ListHTTP)
}
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("py/app.py"),
        r#"
from flask import Flask
app = Flask(__name__)
@app.get("/py/users")
def py_users():
    pass
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("py/fastapi.py"),
        r#"
from fastapi import FastAPI
app = FastAPI()
@app.get("/py/fastapi")
def py_fastapi():
    pass
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("web/routes.js"),
        r#"
const router = require("express").Router();
router.post("/js/users", createUser);
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("config/routes.rb"),
        r#"
Rails.application.routes.draw do
  get "/ruby/users", to: "users#index"
end
"#,
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let routes = json["results"].as_array().unwrap();
    let route_patterns = routes
        .iter()
        .map(|route| route["routePattern"].as_str().unwrap())
        .collect::<Vec<_>>();

    assert!(route_patterns.contains(&"/api/users"));
    assert!(route_patterns.contains(&"/go/gin"));
    assert!(route_patterns.contains(&"/go/chi"));
    assert!(route_patterns.contains(&"/go/chi-method"));
    assert!(route_patterns.contains(&"/go/gorilla"));
    assert!(route_patterns.contains(&"/go/http"));
    assert!(route_patterns.contains(&"/py/users"));
    assert!(route_patterns.contains(&"/py/fastapi"));
    assert!(route_patterns.contains(&"/js/users"));
    assert!(route_patterns.contains(&"/ruby/users"));
    for framework in ["gin", "chi", "gorilla", "net/http"] {
        assert!(
            routes.iter().any(|route| route["framework"] == framework),
            "missing Go framework {framework}"
        );
    }
    assert_eq!(json["reliability"]["level"], "parser_fact");

    let filtered = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "ruby", "--framework", "rails", "--method", "GET"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let filtered_json: Value = serde_json::from_slice(&filtered).unwrap();
    assert_eq!(filtered_json["results"].as_array().unwrap().len(), 1);
    assert_eq!(filtered_json["results"][0]["framework"], "rails");

    let filtered_go = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "--framework", "gin", "--method", "GET"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let filtered_go_json: Value = serde_json::from_slice(&filtered_go).unwrap();
    assert_eq!(filtered_go_json["results"].as_array().unwrap().len(), 1);
    assert_eq!(filtered_go_json["results"][0]["routePattern"], "/go/gin");

    let filtered_fastapi = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "--framework", "fastapi", "--method", "GET"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let filtered_fastapi_json: Value = serde_json::from_slice(&filtered_fastapi).unwrap();
    assert_eq!(
        filtered_fastapi_json["results"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        filtered_fastapi_json["results"][0]["routePattern"],
        "/py/fastapi"
    );
}

#[test]
fn routes_search_matches_route_metadata_and_supports_regex() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("modules/ruoyi-quartz/config")).unwrap();
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(
        dir.path().join("modules/ruoyi-quartz/config/routes.rb"),
        "get \"/monitor/job\", to: \"jobs#index\"\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("config/routes.rb"),
        "get \"/health\", to: \"health#show\"\n",
    )
    .unwrap();

    let by_path = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "quartz"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let by_path_json: Value = serde_json::from_slice(&by_path).unwrap();
    let by_path_results = by_path_json["results"].as_array().unwrap();

    assert_eq!(by_path_json["query"]["mode"], "literal");
    assert_eq!(by_path_results.len(), 1);
    assert_eq!(by_path_results[0]["routePattern"], "/monitor/job");
    assert!(by_path_results[0]["path"]
        .as_str()
        .unwrap()
        .contains("ruoyi-quartz"));

    let by_handler_regex = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "jobs#.*", "--mode", "regex"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let by_handler_json: Value = serde_json::from_slice(&by_handler_regex).unwrap();
    let by_handler_results = by_handler_json["results"].as_array().unwrap();

    assert_eq!(by_handler_json["query"]["mode"], "regex");
    assert_eq!(by_handler_results.len(), 1);
    assert_eq!(by_handler_results[0]["handler"], "jobs#index");
}

#[test]
fn routes_scans_vapor_swift_routes() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Sources/App")).unwrap();
    fs::write(
        dir.path().join("Package.swift"),
        "// swift-tools-version: 6.0\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("Sources/App/routes.swift"),
        r#"
import Vapor

func routes(_ app: Application) throws {
    app.get("swift", "users", use: listUsers)
    app.post("swift", "users") { req in "ok" }
    let grouped = app.grouped("api")
    grouped.delete("users", ":id", use: deleteUser)
}
"#,
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "--framework", "vapor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let routes = json["results"].as_array().unwrap();
    let patterns = routes
        .iter()
        .map(|route| route["routePattern"].as_str().unwrap())
        .collect::<Vec<_>>();

    assert!(patterns.contains(&"/swift/users"));
    assert!(patterns.contains(&"/api/users/:id"));
    assert!(routes.iter().all(|route| route["language"] == "swift"));
    assert!(routes.iter().all(|route| route["framework"] == "vapor"));
    assert_eq!(json["reliability"]["level"], "parser_fact");
}

#[test]
fn routes_handles_utf8_near_java_annotation_window() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    let padding = "a".repeat(699);
    fs::write(
        dir.path().join("src/main/java/example/DemoController.java"),
        format!(
            r#"
@RestController
@RequestMapping("/demo")
class DemoController {{
  @GetMapping("/window")
  /* {padding}é marker */
  public String window() {{ return ""; }}
}}
"#
        ),
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "--framework", "spring", "--method", "GET"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"][0]["routePattern"], "/demo/window");
    assert_eq!(json["results"][0]["reliability"], "parser_fact");
}

#[test]
fn routes_skips_public_java_class_request_mapping() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path()
            .join("src/main/java/example/AdminController.java"),
        r#"
@RestController
@RequestMapping("/admin")
public class AdminController {
  @GetMapping("/users")
  public String users() { return ""; }
}
"#,
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "--framework", "spring"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let route_patterns = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|route| route["routePattern"].as_str().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(route_patterns, vec!["/admin/users"]);
}

#[test]
fn routes_text_output_shows_method_route_and_location() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(
        dir.path().join("config/routes.rb"),
        "get \"/health\", to: \"health#show\"\n",
    )
    .unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "--framework", "rails"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert!(text.contains("GET"));
    assert!(text.contains("/health"));
    assert!(text.contains("config/routes.rb:1"));
    assert!(text.contains("rails"));
    assert!(text.contains("handler=health#show"));
    assert_ne!(text.trim(), "config/routes.rb:1");
}

#[test]
fn routes_text_output_shows_next_page_cursor_when_limited() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(
        dir.path().join("config/routes.rb"),
        "get \"/one\", to: \"one#show\"\nget \"/two\", to: \"two#show\"\nget \"/three\", to: \"three#show\"\n",
    )
    .unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["routes", "--framework", "rails", "--limit", "2"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert!(text.contains("/one"));
    assert!(text.contains("/two"));
    assert!(!text.contains("/three"));
    assert!(text.contains("more: showing first 2 results"));
    assert!(text.contains("use --cursor "));
    assert!(text.contains("increase --limit"));
}

#[test]
fn routes_saved_query_replays_scope_and_filters() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(
        dir.path().join("config/routes.rb"),
        "get \"/health\", to: \"health#show\"\n",
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--save-query")
        .arg("rails-routes")
        .args([
            "routes",
            "health|missing",
            "--mode",
            "regex",
            "--framework",
            "rails",
        ])
        .assert()
        .success();

    let saved_path = dir.path().join(".codetrail/queries/rails-routes.json");
    let saved_file: Value = serde_json::from_slice(&fs::read(&saved_path).unwrap()).unwrap();
    assert_eq!(saved_file["query"]["mode"], "regex");

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "replay", "rails-routes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["command"], "routes");
    assert_eq!(json["query"]["mode"], "regex");
    assert_eq!(json["results"][0]["routePattern"], "/health");
    assert_eq!(json["results"][0]["framework"], "rails");
}

#[test]
fn ruby_parser_fallback_extracts_symbols_and_calls() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("app")).unwrap();
    fs::write(
        dir.path().join("Gemfile"),
        "source \"https://rubygems.org\"\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("app/widget.rb"),
        r#"
class Widget
  def run
    helper()
  end

  def self.build
  end

  def helper
  end
end
"#,
    )
    .unwrap();

    let symbols = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--lang")
        .arg("ruby")
        .args(["symbols", "Widget"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let symbols_json: Value = serde_json::from_slice(&symbols).unwrap();
    assert_eq!(symbols_json["results"][0]["language"], "ruby");
    assert_eq!(symbols_json["results"][0]["kind"], "class");

    let callers = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--lang")
        .arg("ruby")
        .args(["callers", "helper"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert_eq!(callers_json["reliability"]["level"], "inferred_candidate");
    assert!(callers_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| result["enclosingSymbol"] == "run"));
}

#[test]
fn swift_language_mapping_is_visible_to_files_and_index_status() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Sources/App")).unwrap();
    fs::create_dir_all(dir.path().join("ios/App.xcodeproj")).unwrap();
    fs::create_dir_all(dir.path().join("ios/Sources")).unwrap();
    fs::write(
        dir.path().join("Package.swift"),
        "// swift-tools-version: 6.0\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("Sources/App/App.swift"),
        "public struct App { public func run() {} }\n",
    )
    .unwrap();
    fs::write(dir.path().join("ios/App.xcodeproj/project.pbxproj"), "{}\n").unwrap();
    fs::write(
        dir.path().join("ios/Sources/ViewController.swift"),
        "final class ViewController {}\n",
    )
    .unwrap();

    let files = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--lang")
        .arg("swift")
        .args(["files", "App.swift"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let files_json: Value = serde_json::from_slice(&files).unwrap();
    assert_eq!(files_json["results"][0]["path"], "Sources/App/App.swift");
    assert_eq!(files_json["results"][0]["language"], "swift");

    raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build", "--no-semantic"])
        .assert()
        .success();

    let status = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("json")
        .env("PATH", tempdir().unwrap().path())
        .env_remove("CODETRAIL_LSP_SWIFT")
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_json: Value = serde_json::from_slice(&status).unwrap();
    let status = &status_json["results"][0];
    assert!(status["indexedLanguages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|language| language["language"] == "swift"
            && language["fileCount"].as_u64().unwrap_or(0) >= 1));
    let swift_server = status["semanticStatus"]["semanticProviders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["language"] == "swift")
        .expect("swift provider status");
    assert_eq!(swift_server["status"], "missing");
    assert_eq!(swift_server["defaultCommand"], "sourcekit-lsp");
    assert_eq!(swift_server["envKey"], "CODETRAIL_LSP_SWIFT");

    let roots = status["semanticStatus"]["roots"].as_array().unwrap();
    let swiftpm_root = roots
        .iter()
        .find(|root| root["kind"] == "swift_package")
        .expect("swift package root");
    assert_eq!(swiftpm_root["swiftConfig"]["status"], "configured");
    let xcode_root = roots
        .iter()
        .find(|root| root["kind"] == "swift_xcode_project")
        .expect("xcode root");
    assert_eq!(xcode_root["swiftConfig"]["ready"], false);
    assert_eq!(xcode_root["swiftConfig"]["status"], "missing_config");
}

#[test]
fn kotlin_language_parser_status_summary_and_explore_are_bounded() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("settings.gradle.kts"),
        "pluginManagement {}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("build.gradle.kts"),
        "plugins { kotlin(\"jvm\") version \"1.9.0\" }\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src/main/kotlin/okhttp3")).unwrap();
    fs::write(
        dir.path().join("src/main/kotlin/okhttp3/RealCall.kt"),
        r#"package okhttp3

class RealCall(private val chain: RealInterceptorChain) {
  fun getResponseWithInterceptorChain(): Response {
    return chain.proceed(Request())
  }
}

class RealInterceptorChain {
  fun proceed(request: Request): Response {
    return Response()
  }
}

class Request
class Response
"#,
    )
    .unwrap();

    let files = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--lang")
        .arg("kotlin")
        .args(["files", "RealCall.kt"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let files_json: Value = serde_json::from_slice(&files).unwrap();
    assert_eq!(files_json["results"][0]["language"], "kotlin");

    let kts_files = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--lang")
        .arg("kotlin")
        .args(["files", "build.gradle.kts"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let kts_json: Value = serde_json::from_slice(&kts_files).unwrap();
    assert_eq!(kts_json["results"][0]["language"], "kotlin");

    let symbols = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["symbols", "RealCall"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let symbols_json: Value = serde_json::from_slice(&symbols).unwrap();
    assert_eq!(symbols_json["reliability"]["level"], "parser_fact");
    assert!(symbols_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| {
            result["language"] == "kotlin"
                && result["name"] == "RealCall"
                && result["kind"] == "class"
        }));

    let defs = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "getResponseWithInterceptorChain"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();
    assert_eq!(defs_json["reliability"]["level"], "parser_fact");
    assert!(defs_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| {
            result["language"] == "kotlin" && result["name"] == "getResponseWithInterceptorChain"
        }));

    let proceed_defs = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "proceed"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let proceed_defs_json: Value = serde_json::from_slice(&proceed_defs).unwrap();
    assert!(proceed_defs_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| result["language"] == "kotlin" && result["name"] == "proceed"));

    let callers = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "proceed"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert!(callers_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| {
            result["language"] == "kotlin"
                && result["enclosingSymbol"] == "getResponseWithInterceptorChain"
        }));

    raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build", "--no-semantic"])
        .assert()
        .success();

    let summary = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "index", "status", "--summary"])
        .env("PATH", tempdir().unwrap().path())
        .env_remove("CODETRAIL_SCIP_KOTLIN")
        .env_remove("CODETRAIL_SCIP_JAVA")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let summary_json: Value = serde_json::from_slice(&summary).unwrap();
    let summary = &summary_json["results"][0];
    assert!(summary["indexedLanguages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|language| language["language"] == "kotlin"));
    let kotlin_coverage = summary["semanticStatus"]["languageCoverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|coverage| coverage["language"] == "kotlin")
        .expect("kotlin coverage");
    assert_eq!(summary["semanticStatus"]["queryMode"], "parser_fallback");
    assert_eq!(
        summary["semanticStatus"]["fallbackReason"],
        "scip_index_not_generated"
    );
    assert_eq!(kotlin_coverage["provider"], "scip-java");
    assert_eq!(kotlin_coverage["precise"], "manual_required");
    assert_eq!(kotlin_coverage["mode"], "parser_fallback");
    assert_eq!(kotlin_coverage["fallback"], "tree_sitter_parser");
    assert!(summary["manifest"].is_null());
    assert!(summary["semanticManifests"].is_null());

    let explore = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "explore",
            "node",
            "RealCall",
            "--max-candidates",
            "5",
            "--snippet-lines",
            "6",
            "--relation-limit",
            "4",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let explore_json: Value = serde_json::from_slice(&explore).unwrap();
    let first = &explore_json["results"][0];
    assert_eq!(first["language"], "kotlin");
    assert_eq!(first["name"], "RealCall");
    assert!(first["snippet"]
        .as_str()
        .unwrap()
        .contains("class RealCall"));
    assert!(first["source"].is_null());
    assert!(first["relations"]["calls"].as_array().unwrap().len() <= 4);
    assert!(first["relations"]["callers"].as_array().unwrap().len() <= 4);

    let compact_explore = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "explore",
            "node",
            "RealCall",
            "--compact",
            "--max-candidates",
            "5",
            "--snippet-lines",
            "24",
            "--relation-limit",
            "8",
            "--max-bytes",
            "5000",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        compact_explore.len() < 8_000,
        "compact explore output too large: {} bytes",
        compact_explore.len()
    );
    let compact_json: Value = serde_json::from_slice(&compact_explore).unwrap();
    assert!(compact_json["results"].as_array().unwrap().len() <= 2);
    assert!(compact_json["results"][0]["citeTarget"]
        .as_str()
        .unwrap()
        .contains("RealCall.kt:"));
}

#[test]
fn changed_scope_searches_only_git_changed_files() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.name", "Test"])
        .output()
        .unwrap();
    fs::write(
        dir.path().join("src/clean.rs"),
        "fn clean() { /* needle */ }\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "src/clean.rs"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();
    fs::write(
        dir.path().join("src/changed.rs"),
        "fn changed() { /* needle */ }\n",
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--changed")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let paths: Vec<&str> = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["path"].as_str())
        .collect();

    assert_eq!(json["query"]["scope"]["changed"], true);
    assert_eq!(paths, vec!["src/changed.rs"]);
}

#[test]
fn changed_output_distinguishes_staged_unstaged_and_untracked() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.name", "Test"])
        .output()
        .unwrap();
    fs::write(dir.path().join("src/staged.rs"), "old staged\n").unwrap();
    fs::write(dir.path().join("src/unstaged.rs"), "old unstaged\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "src"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    fs::write(dir.path().join("src/staged.rs"), "new staged\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "src/staged.rs"])
        .output()
        .unwrap();
    fs::write(dir.path().join("src/unstaged.rs"), "new unstaged\n").unwrap();
    fs::write(dir.path().join("src/untracked.rs"), "new untracked\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["changed"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let results = json["results"].as_array().unwrap();
    let staged = results
        .iter()
        .find(|result| result["path"] == "src/staged.rs")
        .unwrap();
    let unstaged = results
        .iter()
        .find(|result| result["path"] == "src/unstaged.rs")
        .unwrap();
    let untracked = results
        .iter()
        .find(|result| result["path"] == "src/untracked.rs")
        .unwrap();

    assert_eq!(staged["changeKind"], "staged");
    assert_eq!(staged["staged"], true);
    assert_eq!(unstaged["changeKind"], "unstaged");
    assert_eq!(unstaged["unstaged"], true);
    assert_eq!(untracked["changeKind"], "untracked");
    assert_eq!(untracked["untracked"], true);
    assert_eq!(json["summary"]["changed"]["stagedCount"], 1);
    assert_eq!(json["summary"]["changed"]["unstagedCount"], 1);
    assert_eq!(json["summary"]["changed"]["untrackedCount"], 1);
    assert!(json["summary"]["changed"]["head"].as_str().is_some());
    assert!(json["summary"]["changed"]["worktree"]
        .as_str()
        .unwrap()
        .starts_with("worktree:"));
}

#[test]
fn empty_changed_scope_returns_noop_warning_without_full_workspace_fallback() {
    let dir = tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.name", "Test"])
        .output()
        .unwrap();
    fs::write(dir.path().join("clean.rs"), "needle\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "clean.rs"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--changed")
        .args(["find", "needle"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["query"]["scope"]["changed"], true);
    assert!(json["results"].as_array().unwrap().is_empty());
    assert_eq!(
        json["warnings"][0]["code"],
        "changed_scope_is_empty_no_full_workspace_fallback_was_used"
    );
}

#[test]
fn cursor_paginates_stably_and_reports_facets() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    for path in ["src/a.rs", "src/b.rs", "src/c.rs"] {
        fs::write(dir.path().join(path), "needle\n").unwrap();
    }

    let first_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first: Value = serde_json::from_slice(&first_output).unwrap();
    let cursor = first["nextCursor"].as_str().unwrap().to_string();

    assert_eq!(first["truncated"], true);
    assert_eq!(first["summary"]["resultCount"], 1);
    assert_eq!(first["results"][0]["path"], "src/a.rs");
    assert!(first["summary"]["facets"]["language"]
        .as_array()
        .unwrap()
        .iter()
        .any(|facet| facet["value"] == "rust" && facet["count"] == 3));

    let second_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .arg("--cursor")
        .arg(cursor)
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second: Value = serde_json::from_slice(&second_output).unwrap();

    assert_eq!(second["results"][0]["path"], "src/b.rs");
    assert_ne!(first["results"][0]["path"], second["results"][0]["path"]);
    assert_eq!(second["truncated"], true);
    assert!(second["nextCursor"].as_str().is_some());
}

#[test]
fn cursor_rejects_query_scope_mismatch() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "needle\nother\n").unwrap();
    fs::write(dir.path().join("b.txt"), "needle\n").unwrap();

    let first_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first: Value = serde_json::from_slice(&first_output).unwrap();
    let cursor = first["nextCursor"].as_str().unwrap();

    let mismatch_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .arg("--cursor")
        .arg(cursor)
        .args(["find", "other"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&mismatch_output).unwrap();

    assert_eq!(json["error"]["code"], "cursor_does_not_match_query_scope");
}

#[test]
fn cursor_rejects_snapshot_mismatch_after_worktree_changes() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.name", "Test"])
        .output()
        .unwrap();
    fs::write(dir.path().join("src/a.rs"), "needle\n").unwrap();
    fs::write(dir.path().join("src/b.rs"), "needle\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "src"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    let first_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first: Value = serde_json::from_slice(&first_output).unwrap();
    let cursor = first["nextCursor"].as_str().unwrap();

    fs::write(dir.path().join("src/aa.rs"), "needle\n").unwrap();

    let mismatch_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .arg("--cursor")
        .arg(cursor)
        .args(["find", "needle"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&mismatch_output).unwrap();

    assert_eq!(json["error"]["code"], "cursor_does_not_match_query_scope");
}

#[test]
fn cursor_rejects_dirty_worktree_result_set_changes() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.name", "Test"])
        .output()
        .unwrap();
    fs::write(dir.path().join("README.md"), "base\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "README.md"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();
    fs::write(dir.path().join("src/a.rs"), "needle\n").unwrap();
    fs::write(dir.path().join("src/b.rs"), "needle\n").unwrap();

    let first_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first: Value = serde_json::from_slice(&first_output).unwrap();
    let cursor = first["nextCursor"].as_str().unwrap();
    let first_snapshot = first["snapshot_id"].as_str().unwrap().to_string();

    fs::write(dir.path().join("src/aa.rs"), "needle\n").unwrap();

    let mismatch_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .arg("--cursor")
        .arg(cursor)
        .args(["find", "needle"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&mismatch_output).unwrap();

    assert!(first_snapshot.starts_with("worktree:"));
    assert_eq!(json["error"]["code"], "cursor_does_not_match_query_scope");
}

#[test]
fn saved_query_replay_matches_direct_query_and_can_be_deleted() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
    fs::write(dir.path().join("b.txt"), "needle\n").unwrap();

    let saved_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--save-query")
        .arg("needles")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let saved_json: Value = serde_json::from_slice(&saved_output).unwrap();
    assert_eq!(saved_json["savedQuery"]["name"], "needles");

    let saved_path = dir.path().join(".codetrail/queries/needles.json");
    let saved_file: Value = serde_json::from_slice(&fs::read(&saved_path).unwrap()).unwrap();
    assert_eq!(saved_file["command"], "find");
    assert_eq!(saved_file["query"]["pattern"], "needle");
    assert_eq!(saved_file["query"]["scope"]["limit"], 100);
    assert!(saved_file.get("results").is_none());

    let direct_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let direct_json: Value = serde_json::from_slice(&direct_output).unwrap();

    let replay_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "replay", "needles"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let replay_json: Value = serde_json::from_slice(&replay_output).unwrap();
    assert_eq!(replay_json["query"], direct_json["query"]);
    assert_eq!(replay_json["results"], direct_json["results"]);
    assert_eq!(replay_json["savedQuery"]["snapshotMatch"], true);

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "delete", "needles"])
        .assert()
        .success();
    assert!(!saved_path.exists());
}

#[test]
fn saved_query_replay_continues_from_saved_next_cursor() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
    fs::write(dir.path().join("b.txt"), "needle\n").unwrap();

    let first_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .arg("--save-query")
        .arg("page")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first_json: Value = serde_json::from_slice(&first_output).unwrap();
    let saved_cursor = first_json["nextCursor"].as_str().unwrap().to_string();
    assert_eq!(first_json["results"][0]["path"], "a.txt");

    let replay_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "replay", "page"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let replay_json: Value = serde_json::from_slice(&replay_output).unwrap();

    assert_eq!(replay_json["query"]["scope"]["cursor"], saved_cursor);
    assert_eq!(replay_json["results"][0]["path"], "b.txt");
}

#[test]
fn saved_query_replay_warns_when_snapshot_changes() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());
    fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "a.txt"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--save-query")
        .arg("stable")
        .args(["find", "needle"])
        .assert()
        .success();
    fs::write(dir.path().join("b.txt"), "needle\n").unwrap();

    let replay_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "replay", "stable"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let replay_json: Value = serde_json::from_slice(&replay_output).unwrap();

    assert_eq!(replay_json["savedQuery"]["snapshotMatch"], false);
    assert!(replay_json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "saved_query_snapshot_mismatch"));

    let saved_snapshot_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "replay", "stable", "--snapshot", "saved"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let saved_snapshot_json: Value = serde_json::from_slice(&saved_snapshot_output).unwrap();
    assert_eq!(
        saved_snapshot_json["error"]["code"],
        "saved_query_snapshot_mismatch"
    );
}

#[test]
fn saved_query_replay_drops_saved_cursor_when_snapshot_changes_to_current() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());
    fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
    fs::write(dir.path().join("b.txt"), "needle\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "."])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    let first_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("1")
        .arg("--save-query")
        .arg("page")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first_json: Value = serde_json::from_slice(&first_output).unwrap();
    assert!(first_json["nextCursor"].as_str().is_some());

    fs::write(dir.path().join("aa.txt"), "needle\n").unwrap();
    let replay_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "replay", "page"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let replay_json: Value = serde_json::from_slice(&replay_output).unwrap();

    assert_eq!(replay_json["savedQuery"]["snapshotMatch"], false);
    assert_eq!(replay_json["query"]["scope"]["cursor"], Value::Null);
    assert_eq!(replay_json["results"][0]["path"], "a.txt");
    assert!(replay_json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "saved_query_snapshot_mismatch"));
}

#[test]
fn saved_query_replay_preserves_symbol_scope() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/a")).unwrap();
    fs::create_dir_all(dir.path().join("src/b")).unwrap();
    fs::write(dir.path().join("src/a/mod.rs"), "fn needle() {}\n").unwrap();
    fs::write(dir.path().join("src/b/mod.rs"), "fn needle() {}\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--include")
        .arg("src/a")
        .arg("--save-query")
        .arg("defs-a")
        .args(["defs", "needle"])
        .assert()
        .success();

    let replay_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "replay", "defs-a"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let replay_json: Value = serde_json::from_slice(&replay_output).unwrap();

    assert_eq!(replay_json["query"]["scope"]["include"], json!(["src/a"]));
    assert_eq!(replay_json["results"].as_array().unwrap().len(), 1);
    assert_eq!(replay_json["results"][0]["path"], "src/a/mod.rs");
}

#[test]
fn saved_query_replay_preserves_include_code_options() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\nfn beta() {}\n",
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--save-query")
        .arg("alpha-code")
        .args([
            "symbols",
            "alpha",
            "--include-code",
            "--code-context",
            "3",
            "--code-max-lines",
            "8",
        ])
        .assert()
        .success();

    let saved_path = dir.path().join(".codetrail/queries/alpha-code.json");
    let saved_file: Value = serde_json::from_slice(&fs::read(&saved_path).unwrap()).unwrap();
    assert_eq!(saved_file["query"]["includeCode"], true);
    assert_eq!(saved_file["query"]["codeContext"], 3);
    assert_eq!(saved_file["query"]["codeMaxLines"], 8);

    let replay_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["query", "replay", "alpha-code"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let replay_json: Value = serde_json::from_slice(&replay_output).unwrap();
    assert_eq!(replay_json["query"]["includeCode"], true);
    assert!(replay_json["results"][0]["source"]["content"]
        .as_str()
        .unwrap()
        .contains("beta();"));
    assert!(replay_json["results"][0]["relations"]["calls"]
        .as_array()
        .unwrap()
        .iter()
        .any(|call| call["target"] == "beta"));
}

#[test]
fn jsonl_summary_includes_cursor_and_facets() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.rs"), "needle\n").unwrap();
    fs::write(dir.path().join("b.rs"), "needle\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("jsonl")
        .arg("--limit")
        .arg("1")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let lines = String::from_utf8(output).unwrap();
    let summary: Value = serde_json::from_str(lines.lines().last().unwrap()).unwrap();

    assert_eq!(summary["event"], "page");
    assert_eq!(summary["page"]["truncated"], false);
    assert!(summary["page"]["nextCursor"].as_str().is_some());
}

#[test]
fn small_workspace_uses_generous_output_budget() {
    let dir = tempdir().unwrap();
    let preview = format!("needle {}\n", "a".repeat(180));
    fs::write(dir.path().join("small.rs"), preview).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["budget"]["tier"], "small");
    assert_eq!(json["budget"]["maxResults"], 100);
    assert_eq!(json["budget"]["maxPreviewChars"], 240);
    assert_eq!(json["budget"]["maxContextLines"], 0);
    assert_eq!(json["results"][0]["previewTruncated"], false);
    assert_eq!(json["summary"]["truncatedCount"], 0);
}

#[test]
fn medium_workspace_truncates_preview_with_reason() {
    let dir = tempdir().unwrap();
    for idx in 0..35 {
        fs::write(
            dir.path().join(format!("file{idx}.rs")),
            format!("needle {}\n", "m".repeat(220)),
        )
        .unwrap();
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["budget"]["tier"], "medium");
    assert_eq!(json["budget"]["maxPreviewChars"], 160);
    assert_eq!(json["results"][0]["truncated"], true);
    assert_eq!(
        json["results"][0]["truncatedReason"],
        "output_budget_preview"
    );
    assert_eq!(json["results"][0]["previewTruncated"], true);
    assert_eq!(
        json["results"][0]["previewTruncatedReason"],
        "output_budget_preview"
    );
    assert!(!json["suggestedReads"].as_array().unwrap().is_empty());
    assert_eq!(json["summary"]["truncatedCount"], 35);
}

#[test]
fn large_high_hit_workspace_reduces_preview_and_context_budget() {
    let dir = tempdir().unwrap();
    for idx in 0..220 {
        fs::write(
            dir.path().join(format!("file{idx}.rs")),
            format!(
                "alpha {idx}\nbeta {idx}\ngamma {idx}\nneedle {}\ndelta {idx}\nepsilon {idx}\nzeta {idx}\n",
                "l".repeat(260)
            ),
        )
        .unwrap();
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--context")
        .arg("3")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let first_context = json["results"][0]["context"].as_array().unwrap();

    assert_eq!(json["budget"]["tier"], "large");
    assert_eq!(json["budget"]["maxPreviewChars"], 96);
    assert_eq!(json["budget"]["maxContextLines"], 3);
    assert!(
        json["results"][0]["preview"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= 99
    );
    assert_eq!(
        json["results"][0]["previewTruncatedReason"],
        "output_budget_preview"
    );
    assert_eq!(first_context.len(), 7);
    assert_eq!(
        json["results"][0]["contextTruncatedReason"],
        "output_budget_context"
    );
    assert_eq!(json["results"][0]["truncated"], true);
    assert!(!json["suggestedReads"].as_array().unwrap().is_empty());
    assert_eq!(json["truncated"], true);
}

#[test]
fn broad_find_returns_guarded_summary_samples() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::create_dir_all(dir.path().join("src/app")).unwrap();
    for idx in 0..8 {
        fs::write(
            dir.path()
                .join(format!("src/main/java/example/Public{idx}.java")),
            "public class Sample {}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join(format!("src/app/public{idx}.ts")),
            "export publicFunction = 'public';\n",
        )
        .unwrap();
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "public"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["guard"]["triggered"], true);
    assert_eq!(json["guard"]["reason"], "broad_literal_pattern");
    assert!(json["guard"]["estimatedMatches"].as_u64().unwrap() > 5);
    assert!(json["results"].as_array().unwrap().len() <= 5);
    assert!(json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "broad_query_guard_triggered"));
    assert!(json["summary"]["facets"]["language"]
        .as_array()
        .unwrap()
        .iter()
        .any(|facet| facet["value"] == "java"));
}

#[test]
fn broad_grep_regex_is_guarded_by_default() {
    let dir = tempdir().unwrap();
    for idx in 0..10 {
        fs::write(dir.path().join(format!("file{idx}.txt")), "anything\n").unwrap();
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["grep", ".*"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["guard"]["triggered"], true);
    assert_eq!(json["guard"]["reason"], "broad_regex_pattern");
    assert_eq!(json["nextCursor"], Value::Null);
    assert!(json["results"].as_array().unwrap().len() <= 5);
}

#[test]
fn broad_files_star_returns_summary_samples() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    for idx in 0..9 {
        fs::write(
            dir.path().join(format!("src/file{idx}.rs")),
            "fn main() {}\n",
        )
        .unwrap();
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["files", "*"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["guard"]["triggered"], true);
    assert_eq!(json["guard"]["reason"], "broad_path_pattern");
    assert_eq!(json["guard"]["estimatedMatches"], 9);
    assert_eq!(json["results"].as_array().unwrap().len(), 5);
    assert!(json["summary"]["facets"]["topDir"]
        .as_array()
        .unwrap()
        .iter()
        .any(|facet| facet["value"] == "src" && facet["count"] == 9));
}

#[test]
fn public_broad_guard_uses_page_truncation_without_caveat() {
    let dir = tempdir().unwrap();
    for idx in 0..8 {
        fs::write(
            dir.path().join(format!("file{idx}.java")),
            "public class Sample {}\n",
        )
        .unwrap();
    }

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "find", "public"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_no_public_caveats(&json);
    assert_eq!(json["page"]["truncated"], true);
    assert!(json["page"]["nextCursor"].is_null());
}

#[test]
fn allow_broad_expands_with_limit_and_cursor() {
    let dir = tempdir().unwrap();
    for idx in 0..3 {
        fs::write(dir.path().join(format!("file{idx}.txt")), "content\n").unwrap();
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--allow-broad")
        .arg("--limit")
        .arg("2")
        .args(["files", "*"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert!(json.get("guard").is_none());
    assert_eq!(json["results"].as_array().unwrap().len(), 2);
    assert_eq!(json["truncated"], true);
    assert!(json["nextCursor"].as_str().is_some());
}

#[test]
fn public_allow_broad_limited_page_uses_cursor_without_truncated_caveat() {
    let dir = tempdir().unwrap();
    for idx in 0..6 {
        fs::write(dir.path().join(format!("file{idx}.txt")), "content\n").unwrap();
    }

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--output",
            "json",
            "--allow-broad",
            "--limit",
            "2",
            "files",
            "*",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"].as_array().unwrap().len(), 2);
    assert_eq!(json["page"]["truncated"], false);
    assert!(json["page"]["nextCursor"].as_str().is_some());
    assert_no_public_caveats(&json);
}

#[test]
fn limit_does_not_bypass_broad_query_guard() {
    let dir = tempdir().unwrap();
    for idx in 0..8 {
        fs::write(
            dir.path().join(format!("file{idx}.java")),
            "public class Sample {}\n",
        )
        .unwrap();
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--limit")
        .arg("20")
        .args(["find", "public"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["budget"]["maxResults"], 20);
    assert_eq!(json["guard"]["triggered"], true);
    assert_eq!(json["results"].as_array().unwrap().len(), 5);
    assert_eq!(json["nextCursor"], Value::Null);
}

#[test]
fn small_broad_literal_match_does_not_trigger_guard() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("file.txt"), "x\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "x"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert!(json.get("guard").is_none());
    assert_eq!(json["results"].as_array().unwrap().len(), 1);
    assert!(!json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "broad_query_guard_triggered"));
}

#[test]
fn text_output_reports_broad_guard_warning() {
    let dir = tempdir().unwrap();
    for idx in 0..6 {
        fs::write(dir.path().join(format!("file{idx}.txt")), "anything\n").unwrap();
    }

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("text")
        .args(["grep", ".*"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert!(text.contains("warning: broad query guard triggered"));
}

#[test]
fn text_output_regular_search_stays_path_line_focused() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() {\n let needle = 1;\n}\n",
    )
    .unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert_eq!(text.trim(), "src/main.rs:2  let needle = 1;");
}

#[test]
fn text_output_symbols_keep_location_on_result_line() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn beta() {}\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["symbols", "beta"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    let mut lines = text.lines();

    assert_eq!(lines.next(), Some("function     fn beta()  src/lib.rs:1"));
    assert_ne!(lines.next(), Some("  src/lib.rs:1"));
}

#[test]
fn text_output_call_graph_keeps_location_on_result_line() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\n\nfn beta() {}\n",
    )
    .unwrap();

    for args in [["calls", "alpha"], ["callers", "beta"]] {
        let output = raw_codetrail()
            .arg("--path")
            .arg(dir.path())
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("alpha -> beta  src/lib.rs:2"));
        assert!(!text.contains("\n  src/lib.rs:2\n"));
    }
}

#[test]
fn text_output_no_match_shows_hint_and_exit_code_two() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("text")
        .args(["find", "MissingThing"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert!(text.contains("no matches for find"));
    assert!(!text.contains("try:"));
}

#[test]
fn text_output_broad_query_shows_summary_facets_and_next_action() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/java")).unwrap();
    for idx in 0..8 {
        fs::write(
            dir.path().join(format!("src/java/Public{idx}.java")),
            "public class Sample {}\n",
        )
        .unwrap();
    }

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("text")
        .args(["find", "public"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert!(text.contains("summary:"));
    assert!(text.contains("estimated matches: 8"));
    assert!(text.contains("top languages: java=8"));
    assert!(!text.contains("next:"));
}

#[test]
fn text_output_omits_parser_fallback_caveat() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn helper() {}\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("text")
        .args(["defs", "helper"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert!(!text.contains("caveat:"));
    assert!(!text.contains("precise_scip_index_unavailable"));
    assert!(text.contains("src/lib.rs:1"));
}

#[test]
fn text_output_error_is_single_readable_line() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("text")
        .args(["grep", "["])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert!(text.starts_with("error:"));
    assert!(!text.contains("\"schemaVersion\""));
}

#[test]
fn text_output_parse_error_is_single_readable_line() {
    let output = raw_codetrail()
        .args(["--output", "text", "--definitely-not-an-option"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();

    assert_eq!(text.lines().count(), 1);
    assert!(text.starts_with("error:"));
    assert!(!text.contains("Usage:"));
}

#[test]
fn json_output_includes_source_targets_and_next_actions() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() {\n    println!(\"needle\");\n}\n",
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"][0]["sourceTarget"], "src/main.rs");
    assert!(json["results"][0].get("readCommand").is_none());
    assert!(json["results"][0].get("readCommandArgv").is_none());
    assert_eq!(json["suggestedReads"][0], "src/main.rs");
    assert_eq!(json["nextActions"][0]["kind"], "source_read");
    assert_eq!(json["nextActions"][0]["target"], "src/main.rs");
    assert_eq!(json["truncated"], false);
    assert!(json["nextCursor"].is_null());
}

#[test]
fn small_file_read_suggestions_prefer_one_full_file_read() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn first() {\n    println!(\"needle\");\n}\nfn second() {\n    println!(\"needle\");\n}\n",
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"].as_array().unwrap().len(), 2);
    for result in json["results"].as_array().unwrap() {
        assert_eq!(result["sourceTarget"], "src/main.rs");
    }
    assert_eq!(json["suggestedReads"].as_array().unwrap().len(), 1);
    assert_eq!(json["nextActions"].as_array().unwrap().len(), 1);
    assert_eq!(json["suggestedReads"][0], "src/main.rs");
    assert_eq!(json["nextActions"][0]["target"], "src/main.rs");

    let source = fs::read_to_string(dir.path().join("src/main.rs")).unwrap();
    assert_eq!(
        source,
        "fn first() {\n    println!(\"needle\");\n}\nfn second() {\n    println!(\"needle\");\n}\n"
    );
}

#[test]
fn large_file_read_suggestions_keep_precise_ranges() {
    let dir = tempdir().unwrap();
    let content = (0..8000)
        .map(|idx| {
            if idx == 7000 {
                "needle in a large file\n".to_string()
            } else {
                format!("line {idx:04} filler text\n")
            }
        })
        .collect::<String>();
    fs::write(dir.path().join("large.txt"), content).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    let source_target = json["results"][0]["sourceTarget"].as_str().unwrap();
    assert!(source_target.starts_with("large.txt:"));
    assert_ne!(source_target, "large.txt");
    assert_eq!(json["suggestedReads"].as_array().unwrap().len(), 1);
}

#[test]
fn source_targets_preserve_paths_with_spaces() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src dir")).unwrap();
    fs::write(
        dir.path().join("src dir/a b.rs"),
        "fn main() { /* needle */ }\n",
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"][0]["sourceTarget"], "src dir/a b.rs");
    let source = fs::read_to_string(dir.path().join("src dir/a b.rs")).unwrap();
    assert_eq!(source, "fn main() { /* needle */ }\n");
}

#[test]
fn source_targets_preserve_paths_that_look_like_flags() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("--odd.txt"), "needle\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["results"][0]["sourceTarget"], "--odd.txt");
    assert_eq!(
        fs::read_to_string(dir.path().join("--odd.txt")).unwrap(),
        "needle\n"
    );
}

#[test]
fn deleted_changed_files_do_not_emit_read_next_actions() {
    let dir = tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    fs::write(dir.path().join("gone.txt"), "removed\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "gone.txt"])
        .output()
        .unwrap();
    fs::remove_file(dir.path().join("gone.txt")).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["changed"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["results"][0]["path"], "gone.txt");
    assert_eq!(json["results"][0]["worktreeStatus"], "D");
    assert!(json["results"][0].get("sourceTarget").is_none());
    assert!(json["suggestedReads"].as_array().unwrap().is_empty());
    assert!(json["nextActions"].as_array().unwrap().is_empty());
}

#[test]
fn index_status_metadata_does_not_emit_read_next_actions() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert!(json["results"][0].get("sourceTarget").is_none());
    assert!(json["suggestedReads"].as_array().unwrap().is_empty());
    assert!(json["nextActions"].as_array().unwrap().is_empty());
}

#[test]
fn index_doctor_reports_semantic_frontend_readiness() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn alpha() {}\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "doctor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let doctor = &json["results"][0];

    assert_eq!(doctor["mode"], "scip_index_frontend");
    assert!(doctor["preciseIndex"]["usable"].as_bool().is_some());
    assert_eq!(doctor["nativeFallback"]["tool"], "ripgrep");
    assert_eq!(
        doctor["nativeFallback"]["reason"],
        "CodeTrail no longer wraps text, path, read, or git workflows."
    );
}

#[test]
fn index_status_reports_semantic_status_languages_and_missing_servers() {
    let dir = tempdir().unwrap();
    let path_dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(dir.path().join("pom.xml"), "<project />\n").unwrap();
    fs::write(
        dir.path().join("src/main/java/example/App.java"),
        "package example;\npublic class App {}\n",
    )
    .unwrap();

    raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .env("PATH", path_dir.path())
        .env_remove("CODETRAIL_SCIP_JAVA")
        .args(["index", "build", "--no-semantic"])
        .assert()
        .success();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("json")
        .env("PATH", path_dir.path())
        .env_remove("CODETRAIL_SCIP_JAVA")
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let status = &json["results"][0];

    assert!(status["indexedLanguages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|language| language["language"] == "java" && language["fileCount"] == 1));
    assert_eq!(status["semanticStatus"]["scipIndex"]["enabled"], false);
    assert_eq!(status["semanticStatus"]["scipIndex"]["usable"], false);
    assert_eq!(
        status["semanticStatus"]["scipIndex"]["state"],
        "not_generated"
    );
    let java_server = status["semanticStatus"]["semanticProviders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["language"] == "java")
        .expect("java provider status");
    assert_eq!(java_server["status"], "missing");
    assert_eq!(java_server["provider"], "scip-java");
    assert_eq!(java_server["envKey"], "CODETRAIL_SCIP_JAVA");
    assert_eq!(java_server["fallback"], "tree_sitter_parser");
    assert_eq!(java_server["missingDependencies"][0], "scip-java");

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("text")
        .env("PATH", path_dir.path())
        .env_remove("CODETRAIL_SCIP_JAVA")
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Indexed languages:"));
    assert!(text.contains("java=1"));
    assert!(text.contains("SCIP index: not_generated (enabled: false, usable: false)"));
    assert!(text.contains("java: missing (scip-java; missing: scip-java)"));
    assert!(text.contains("Install:"));
    assert!(text.contains("scip-java_2.13"));
    assert!(text.contains("$HOME/.local/bin/scip-java"));
    assert!(!text.contains("-o scip-java "));
    assert!(text.contains("Command: scip-java index"));
    assert!(text.contains("Override: CODETRAIL_SCIP_JAVA"));
    assert!(text.contains("Fallback: tree-sitter parser"));
}

#[test]
fn index_status_reports_ruby_scip_provider_missing() {
    let dir = tempdir().unwrap();
    let path_dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("app/models")).unwrap();
    fs::write(
        dir.path().join("Gemfile"),
        "source \"https://rubygems.org\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("app/models/user.rb"), "class User\nend\n").unwrap();

    raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .env("PATH", path_dir.path())
        .env_remove("CODETRAIL_SCIP_RUBY")
        .args(["index", "build", "--no-semantic"])
        .assert()
        .success();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("json")
        .env("PATH", path_dir.path())
        .env_remove("CODETRAIL_SCIP_RUBY")
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let status = &json["results"][0];

    assert!(status["indexedLanguages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|language| language["language"] == "ruby"));
    let ruby_server = status["semanticStatus"]["semanticProviders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["language"] == "ruby")
        .expect("ruby provider status");
    assert_eq!(ruby_server["status"], "missing");
    assert_eq!(ruby_server["provider"], "scip-ruby");
    assert_eq!(ruby_server["defaultCommand"], "scip-ruby");
    assert_eq!(ruby_server["defaultArgs"][0], ".");
    assert_eq!(ruby_server["envKey"], "CODETRAIL_SCIP_RUBY");
    assert_eq!(ruby_server["fallback"], "tree_sitter_parser");
    assert_eq!(ruby_server["missingDependencies"][0], "scip-ruby");
}

#[test]
fn index_provider_install_uses_global_lang_filter_when_language_argument_is_absent() {
    let dir = tempdir().unwrap();
    let path_dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"mixed\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(
        dir.path().join("go.mod"),
        "module example.com/mixed\n\ngo 1.22\n",
    )
    .unwrap();
    fs::write(dir.path().join("main.go"), "package main\nfunc main() {}\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("json")
        .env("PATH", path_dir.path())
        .env_remove("CODETRAIL_SCIP_GO")
        .env_remove("CODETRAIL_SCIP_RUST")
        .args(["index-provider", "install", "--lang", "rust", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["language"], "rust");
    assert_eq!(results[0]["provider"], "rust-analyzer-scip");
    assert_eq!(results[0]["status"], "planned");
    assert!(results[0]["installCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line
            .as_str()
            .unwrap()
            .contains("rustup component add rust-analyzer")));
}

#[test]
fn index_provider_install_dry_run_reports_user_level_commands() {
    let dir = tempdir().unwrap();
    let path_dir = tempdir().unwrap();
    fs::write(dir.path().join("pom.xml"), "<project />\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("json")
        .env("PATH", path_dir.path())
        .env_remove("CODETRAIL_SCIP_JAVA")
        .args(["index-provider", "install", "java", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let result = &json["results"][0];
    assert_eq!(result["language"], "java");
    assert_eq!(result["provider"], "scip-java");
    assert_eq!(result["status"], "planned");
    assert_eq!(result["dryRun"], true);
    assert!(result["installCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line
            .as_str()
            .unwrap()
            .contains("$HOME/.local/bin/scip-java")));
    assert!(result["installCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line
            .as_str()
            .unwrap()
            .contains("bootstrap --standalone -f -o")));
    assert!(!result["installCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line.as_str().unwrap().contains("-o scip-java ")));

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("text")
        .env("PATH", path_dir.path())
        .env_remove("CODETRAIL_SCIP_JAVA")
        .args(["index-provider", "install", "java", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("java: scip-java (planned)"));
    assert!(text.contains("$HOME/.local/bin/scip-java"));
}

#[cfg(unix)]
#[test]
fn index_provider_install_json_keeps_child_output_on_stderr() {
    let dir = tempdir().unwrap();
    let path_dir = tempdir().unwrap();
    let bundle = path_dir.path().join("bundle");
    fs::write(
        &bundle,
        "#!/bin/sh\necho child-stdout\necho child-stderr >&2\nexit 0\n",
    )
    .unwrap();
    make_executable(&bundle);

    let path_value = format!("{}:/bin:/usr/bin", path_dir.path().display());
    let assert = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("json")
        .env("PATH", path_value)
        .env_remove("CODETRAIL_SCIP_RUBY")
        .args(["index-provider", "install", "ruby", "--force"])
        .assert()
        .failure();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(!stdout.contains("child-stdout"));
    assert!(!stdout.contains("child-stderr"));

    let json: Value = serde_json::from_str(&stdout).unwrap();
    let result = &json["results"][0];
    assert_eq!(result["language"], "ruby");
    assert_eq!(result["provider"], "scip-ruby");
    assert_eq!(result["status"], "installed_not_on_path");
    assert_eq!(result["steps"][0]["status"], "ok");

    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(stderr.contains("child-stdout"));
    assert!(stderr.contains("child-stderr"));
}

#[cfg(unix)]
#[test]
fn index_provider_install_text_output_suppresses_progress_when_stderr_is_not_tty() {
    let dir = tempdir().unwrap();
    let path_dir = tempdir().unwrap();
    let bundle = path_dir.path().join("bundle");
    fs::write(
        &bundle,
        "#!/bin/sh\necho child-stdout\necho child-stderr >&2\nexit 0\n",
    )
    .unwrap();
    make_executable(&bundle);

    let path_value = format!("{}:/bin:/usr/bin", path_dir.path().display());
    let assert = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .env("PATH", path_value)
        .env_remove("CODETRAIL_SCIP_RUBY")
        .args(["index-provider", "install", "ruby", "--force"])
        .assert()
        .failure();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(stdout.contains("ruby: scip-ruby (installed_not_on_path)"));

    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(stderr.contains("child-stdout"));
    assert!(stderr.contains("child-stderr"));
    assert!(!stderr.contains("Installing ruby provider"));
}

#[test]
fn skill_install_supports_project_scope_and_dry_run() {
    let dir = tempdir().unwrap();
    let project = tempdir().unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("json")
        .args([
            "skill",
            "install",
            "codex",
            "--scope",
            "project",
            "--path",
            project.path().to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let result = &json["results"][0];
    assert_eq!(result["target"], "codex");
    assert_eq!(result["scope"], "project");
    assert_eq!(result["dryRun"], true);
    assert_eq!(result["changed"], false);
    assert_eq!(result["files"].as_array().unwrap().len(), 1);
    assert_eq!(
        project
            .path()
            .join(".codex/skills/codetrail/SKILL.md")
            .exists(),
        false
    );

    raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "skill",
            "install",
            "codex",
            "--scope",
            "project",
            "--path",
            project.path().to_str().unwrap(),
            "--force",
        ])
        .assert()
        .success();
    assert!(project
        .path()
        .join(".codex/skills/codetrail/SKILL.md")
        .exists());
    assert!(!project
        .path()
        .join(".codex/agents/codetrail-evidence.toml")
        .exists());
}

#[test]
fn skill_install_requires_target_without_interactive_terminal() {
    let dir = tempdir().unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--output")
        .arg("json")
        .args(["skill", "install", "--dry-run"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let error = &json["error"];
    let message = error["message"].as_str().unwrap();
    assert_eq!(json["results"].as_array().unwrap().len(), 0);
    assert_eq!(
        error["code"],
        "skill_target_is_required_in_non_interactive_mode_pass_one_of"
    );
    assert_no_public_caveats(&json);
    assert!(message.contains("skill target is required in non-interactive mode"));
    assert!(message.contains("codex"));
    assert!(message.contains("roo"));
    assert!(!message.contains("opencode"));
    assert!(!message.contains("openai"));
}

#[test]
fn error_envelopes_keep_stable_output_fields() {
    let dir = tempdir().unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["read", "missing.txt"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["schemaVersion"], "1.0");
    assert_eq!(json["truncated"], false);
    assert!(json["nextCursor"].is_null());
    assert!(json["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn jsonl_parse_errors_are_error_events() {
    let output = raw_codetrail()
        .args(["--output", "jsonl", "definitely-not-a-command"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "error");
    assert_eq!(lines[0]["error"]["code"], "cli_usage_error");
    assert!(lines[0].get("caveats").is_none());
    assert!(lines[0].get("page").is_none());
}

#[test]
fn compact_json_omits_internal_source_target() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "before\nneedle here\nafter\n",
    )
    .unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "--output",
            "compact-json",
            "--context",
            "1",
            "find",
            "needle",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"][0]["path"], "src/main.rs");
    assert!(json["results"][0].get("readCommand").is_none());
    assert!(json["results"][0].get("readCommandArgv").is_none());
    assert!(json["results"][0].get("sourceTarget").is_none());
    assert!(json.get("schemaVersion").is_none());
}

#[test]
fn jsonl_output_streams_result_events_and_summary() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle one\nneedle two\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "jsonl", "find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert_eq!(lines[0]["event"], "result");
    assert_eq!(lines[0]["result"]["path"], "sample.txt");
    assert_eq!(lines[2]["event"], "page");
    assert_eq!(lines[2]["page"]["truncated"], false);
    assert!(lines[2].get("caveats").is_none());
    assert!(lines[2].get("schemaVersion").is_none());
}

#[test]
fn cli_parse_errors_use_json_error_schema() {
    let output = codetrail()
        .args(["definitely-not-a-command"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["schemaVersion"], "1.0");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "cli_usage_error");
}

#[test]
fn removed_subcommands_share_cli_usage_error_code() {
    let dir = tempdir().unwrap();

    for args in [
        &["read", "missing-one.txt"][..],
        &["read", "missing-two.txt"][..],
        &["list", "missing-one"][..],
        &["tree", "missing-two"][..],
    ] {
        let output = codetrail()
            .arg("--path")
            .arg(dir.path())
            .args(args)
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["error"]["code"], "cli_usage_error");
    }
}

#[test]
fn dynamic_warning_details_do_not_change_warning_code() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/broken.rs"), "fn broken( {\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["symbols", "broken"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["warnings"][0]["code"], "partial_parse_syntax_errors");
}

#[test]
fn find_truncates_very_long_preview_and_summarizes_it() {
    let dir = tempdir().unwrap();
    let long_line = format!("prefix needle {}\n", "x".repeat(2000));
    fs::write(dir.path().join("long.txt"), long_line).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--context", "1", "find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let preview = json["results"][0]["preview"].as_str().unwrap();
    assert!(preview.len() < 400);
    assert_eq!(json["results"][0]["previewTruncated"], true);
    assert_eq!(json["summary"]["truncatedCount"], 1);
}

#[test]
fn generated_directories_are_default_excluded_but_explicitly_searchable() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("target/generated")).unwrap();
    fs::write(dir.path().join("target/generated/out.rs"), "needle\n").unwrap();
    fs::write(dir.path().join("src.rs"), "needle\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert!(json["summary"]["skippedCount"].as_u64().unwrap() >= 1);
    assert!(json["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result["path"] != "target/generated/out.rs"));

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--no-ignore")
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert!(json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| result["path"] == "target/generated/out.rs"));
}

#[test]
fn fresh_index_reports_generated_skips_in_summary() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("target/generated")).unwrap();
    fs::write(dir.path().join("target/generated/out.rs"), "needle\n").unwrap();
    fs::write(dir.path().join("src.rs"), "needle\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
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
    assert!(json["summary"]["skippedCount"].as_u64().unwrap() >= 1);
    assert!(json["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result["path"] != "target/generated/out.rs"));
}

#[test]
fn jsonl_summary_includes_large_content_summary_counts() {
    let dir = tempdir().unwrap();
    let long_line = format!("prefix needle {}\n", "x".repeat(2000));
    fs::write(dir.path().join("long.txt"), long_line).unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "jsonl", "--context", "1", "find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let summary = lines.last().unwrap();
    assert_eq!(summary["event"], "page");
    assert_eq!(summary["page"]["truncated"], true);
    assert!(summary.get("caveats").is_none());
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

    let defs = codetrail()
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

    let callers = codetrail()
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
fn parser_fallback_outputs_candidate_layer_without_precise_facts() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\n\nfn beta() {}\n",
    )
    .unwrap();

    let defs = codetrail()
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
    assert_ne!(defs_json["reliability"]["level"], "precise_fact");
    assert_eq!(defs_json["results"][0]["layer"], "parser_fact");
    assert_eq!(defs_json["results"][0]["rootId"], "rust:.");
    assert!(defs_json["results"][0]["bodyHash"]
        .as_str()
        .unwrap()
        .starts_with("blake3:"));
    assert!(defs_json["results"][0].get("symbol").is_none());

    let callers = codetrail()
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
    assert_ne!(callers_json["reliability"]["level"], "precise_fact");
    assert_eq!(callers_json["results"][0]["layer"], "inferred_candidate");
    assert_eq!(callers_json["results"][0]["enclosingSymbol"], "alpha");
    assert!(callers_json["results"][0]["bodyHash"]
        .as_str()
        .unwrap()
        .starts_with("blake3:"));
}

#[test]
fn parser_candidate_budget_is_public_page_truncation() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    let mut source = String::new();
    for idx in 0..1005 {
        source.push_str(&format!("fn needle_{idx}() {{}}\n"));
    }
    fs::write(dir.path().join("src/lib.rs"), source).unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "--limit", "0", "symbols", "needle_"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["results"].as_array().unwrap().len(), 1000);
    assert_eq!(json["page"]["truncated"], true);
    assert!(json["page"]["nextCursor"].is_null());
    assert_no_public_caveats(&json);
}

#[test]
fn index_verify_detects_stale_files() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "one\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "verify"])
        .assert()
        .success();

    fs::write(dir.path().join("sample.txt"), "one\ntwo\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "verify"])
        .assert()
        .code(6)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let stale = &json["results"][0]["freshness"]["staleFiles"][0];
    assert_eq!(stale["path"], "sample.txt");
    assert_eq!(stale["reason"], "file_hash_mismatch");
}

#[test]
fn index_status_marks_legacy_parquet_snapshot_stale_without_error() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "one\n").unwrap();

    let codetrail_dir = dir.path().join(".codetrail");
    let snapshot_key = "commit_legacy";
    let snapshot_id = "commit:legacy";
    fs::create_dir_all(codetrail_dir.join("working")).unwrap();
    fs::create_dir_all(codetrail_dir.join("snapshots").join(snapshot_key)).unwrap();
    fs::write(
        codetrail_dir
            .join("snapshots")
            .join(snapshot_key)
            .join("files.parquet"),
        b"PAR1legacy",
    )
    .unwrap();
    fs::write(
        codetrail_dir.join("working").join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "toolVersion": "legacy",
            "repoRoot": dir.path().to_string_lossy(),
            "snapshotId": snapshot_id,
            "snapshotKey": snapshot_key,
            "source": "working_tree",
            "head": null,
            "dirty": false,
            "fileCount": 1,
            "scanOptions": {
                "include": [],
                "exclude": [],
                "hidden": false,
                "noIgnore": false,
                "lang": [],
                "changed": false
            },
            "createdAtEpochMs": 0
        }))
        .unwrap(),
    )
    .unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let status = &json["results"][0];
    assert_eq!(status["exists"], true);
    assert_eq!(status["fresh"], false);
    assert_eq!(
        status["freshness"]["staleFiles"][0]["reason"],
        "legacy_snapshot_format"
    );

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "verify"])
        .assert()
        .code(6)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        json["results"][0]["freshness"]["staleFiles"][0]["reason"],
        "legacy_snapshot_format"
    );
}

#[test]
fn index_verify_ignores_missing_best_effort_graph_artifact() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "one\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    fs::remove_dir_all(dir.path().join(".codetrail/graph")).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "verify"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["results"][0]["fresh"], true);
    assert!(json["results"][0]["graphFresh"].is_null());
}

#[test]
fn index_verify_checks_graph_against_active_manifest_snapshot() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    init_git_repo(dir.path());
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\n\nfn beta() {}\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "src/lib.rs"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "verify"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let result = &json["results"][0];
    assert_eq!(result["fresh"], true);
    assert!(result["manifest"]["snapshotId"]
        .as_str()
        .unwrap()
        .starts_with("commit:"));
    assert_eq!(result["graphFresh"], true);
}

#[test]
fn git_dirty_index_status_uses_active_manifest_for_per_file_freshness() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());
    fs::write(dir.path().join("sample.txt"), "one\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "sample.txt"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    fs::write(dir.path().join("sample.txt"), "one\ntwo\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let status = &json["results"][0];
    assert_eq!(status["exists"], true);
    assert_eq!(status["fresh"], false);
    assert!(status["manifest"]["snapshotId"]
        .as_str()
        .unwrap()
        .starts_with("commit:"));
    assert_eq!(
        status["freshness"]["staleFiles"][0]["reason"],
        "file_hash_mismatch"
    );
}

#[test]
fn index_build_writes_lancedb_only_storage() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let codetrail_dir = dir.path().join(".codetrail");
    // LanceDB store is the primary storage backend
    assert!(codetrail_dir.join("index.lance").is_dir());
    // Old JSON/.idx artifacts are no longer written
    assert!(!codetrail_dir.join("snapshots").exists());
    assert!(!codetrail_dir.join("text").exists());
    // working/manifest.json is written for pack/unpack compatibility

    // Build output declares lancedb backend
    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["results"][0]["index"]["used"], true);
    assert_eq!(json["results"][0]["index"]["storageBackend"], "lancedb");
}

#[test]
fn index_skipped_reports_files_skipped_by_last_build() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();
    fs::write(dir.path().join("blob.bin"), b"prefix\0suffix").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "skipped"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let result = &json["results"][0];
    assert_eq!(result["exists"], true);
    assert_eq!(result["count"], 1);
    assert_eq!(result["items"][0]["path"], "blob.bin");
    assert_eq!(result["items"][0]["reason"], "binary");
    assert_eq!(result["items"][0]["stage"], "catalog");

    let public_output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "index", "skipped"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let public_json: Value = serde_json::from_slice(&public_output).unwrap();
    assert_eq!(public_json["results"][0]["items"][0]["path"], "blob.bin");

    let text = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "skipped"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(text).unwrap();
    assert!(text.contains("Skipped files: 1"));
    assert!(text.contains("blob.bin"));
    assert!(text.contains("binary"));
}

#[test]
fn index_build_changed_limits_catalog_to_changed_files() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    init_git_repo(dir.path());
    fs::write(dir.path().join("src/stable.txt"), "stable\n").unwrap();
    fs::write(dir.path().join("src/changed.txt"), "old\n").unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "src"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    fs::write(dir.path().join("src/changed.txt"), "new\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build", "--changed"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).unwrap();
    let index = &json["results"][0]["index"];
    assert_eq!(index["changedOnly"], true);
    assert_eq!(index["fileCount"], 1);
}

#[test]
fn find_uses_fresh_text_index_for_candidates() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
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
fn path_queries_use_fresh_index_catalog() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.path().join("README.md"), "hello\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let files_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["files", "main"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let files_json: Value = serde_json::from_slice(&files_output).unwrap();
    assert_eq!(files_json["index"]["used"], true);
    assert_eq!(files_json["index"]["fresh"], true);
    assert_eq!(
        files_json["results"][0]["producer"],
        "text_index_file_catalog"
    );
    assert_eq!(files_json["results"][0]["sourceReason"], "indexed_fresh");

    let glob_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["glob", "**/*.rs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let glob_json: Value = serde_json::from_slice(&glob_output).unwrap();
    assert_eq!(glob_json["index"]["used"], true);
    assert_eq!(glob_json["results"][0]["path"], "src/main.rs");
    assert_eq!(
        glob_json["results"][0]["producer"],
        "text_index_file_catalog"
    );
}

#[test]
fn dirty_worktree_uses_index_for_fresh_files_and_live_overlay_for_changed_files() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    init_git_repo(dir.path());
    fs::write(
        dir.path().join("src/stable.rs"),
        "fn stable() { /* needle */ }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/changed.rs"),
        "fn changed() { /* needle old */ }\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "src"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    fs::write(
        dir.path().join("src/changed.rs"),
        "fn changed() { /* needle new */ }\n",
    )
    .unwrap();

    let output = codetrail()
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
    assert_eq!(json["index"]["fresh"], false);
    assert_eq!(json["index"]["fallback"], true);
    assert_eq!(json["index"]["reason"], "partial_live_overlay");
    assert_eq!(json["index"]["staleCount"], 1);

    let results = json["results"].as_array().unwrap();
    let stable = results
        .iter()
        .find(|result| result["path"] == "src/stable.rs")
        .unwrap();
    assert_eq!(stable["producer"], "text_index_live_text_search");
    assert_eq!(stable["indexFresh"], true);
    assert_eq!(stable["sourceReason"], "indexed_fresh");

    let changed = results
        .iter()
        .find(|result| result["path"] == "src/changed.rs")
        .unwrap();
    assert_eq!(changed["producer"], "live_text_search");
    assert_eq!(changed["indexFresh"], false);
    assert_eq!(changed["sourceReason"], "per_file_live_overlay");
    assert_eq!(changed["matchText"], "needle");
}

#[test]
fn non_git_added_file_uses_live_overlay_after_index_build() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("stable.txt"), "needle stable\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    fs::write(dir.path().join("added.txt"), "needle added\n").unwrap();

    let output = codetrail()
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
    assert_eq!(json["index"]["fresh"], false);
    assert_eq!(json["index"]["addedCount"], 1);
    let added = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["path"] == "added.txt")
        .unwrap();
    assert_eq!(added["producer"], "live_text_search");
    assert_eq!(added["sourceReason"], "per_file_live_overlay");
}

#[test]
fn added_files_outside_index_scope_do_not_dirty_scoped_index() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("docs")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn main() {}\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--include", "src", "index", "build"])
        .assert()
        .success();

    fs::write(dir.path().join("docs/new.md"), "outside scope\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let status = &json["results"][0];
    assert_eq!(status["fresh"], true);
    assert!(status["freshness"].get("addedFiles").is_none());
}

#[test]
fn find_uses_lancedb_gram_prefilter_for_candidates() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("hit.txt"),
        "this file contains needle_rare_literal\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("miss.txt"),
        "this file contains many words but not the target\n",
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle_rare_literal"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["index"]["used"], true);
    assert_eq!(json["index"]["fresh"], true);
    assert_eq!(json["index"]["prefilter"], "trigram");
    assert_eq!(json["index"]["candidateCount"], 1);
    assert_eq!(json["results"][0]["path"], "hit.txt");
}

#[test]
fn regex_search_reports_prefilter_plan_when_using_index_catalog() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle_123\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["grep", "needle_[0-9]+"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["index"]["used"], true);
    assert_eq!(json["index"]["prefilter"], "none");
    assert_eq!(
        json["index"]["prefilterReason"],
        "regex_prefilter_not_supported"
    );
    assert_eq!(
        json["results"][0]["producer"],
        "text_index_live_text_search"
    );
}

#[test]
fn index_update_noops_when_index_is_fresh() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "update"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let result = &json["results"][0];

    assert_eq!(result["updated"], false);
    assert_eq!(result["reason"], "index_fresh");
    assert_eq!(result["index"]["fresh"], true);
}

#[test]
fn index_update_replaces_stale_gram_postings() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "alpha oldtoken\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    fs::write(dir.path().join("sample.txt"), "alpha newtoken\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "update"])
        .assert()
        .success();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "oldtoken"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["index"]["used"], true);
    assert_eq!(json["index"]["fresh"], true);
    assert_eq!(json["index"]["candidateCount"], 0);
    assert_eq!(json["results"].as_array().unwrap().len(), 0);
}

#[test]
fn files_live_scan_uses_catalog_without_content_hash() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["files", "sample"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["index"]["used"], false);
    assert_eq!(json["results"][0]["producer"], "live_file_catalog");
    assert!(json["results"][0]["hash"].is_null());
}

#[test]
fn query_falls_back_when_scan_options_do_not_match_index() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".hidden.txt"), "needle\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
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
    let output = codetrail()
        .args(["--path", "/definitely/missing", "completions", "bash"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let script = String::from_utf8(output).unwrap();

    assert!(script.contains("complete -F _codetrail codetrail"));
    assert!(script.contains("refs symbols defs calls callers call-hierarchy index completions"));
    assert!(script.contains("build status doctor"));
    assert!(!script.contains("find grep files"));
    assert!(!script.contains("build update status skipped verify clean pack unpack"));
    assert!(!script.contains("generate-scip"));
    assert!(!script.contains("import-scip"));
}

#[test]
fn zsh_completions_include_allow_broad_option() {
    let output = codetrail()
        .args(["--path", "/definitely/missing", "completions", "zsh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let script = String::from_utf8(output).unwrap();

    assert!(script.contains("--allow-broad"));
}

#[test]
fn prebuilt_json_scip_index_drives_precise_defs_refs_and_symbols() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn needle() {}\nfn main() { needle(); }\n",
    )
    .unwrap();
    let scip_path = dir.path().join("index.scip.json");
    write_minimal_scip_json(&scip_path);

    let workspace = codetrail::workspace::Workspace::discover(dir.path()).unwrap();
    let import_json = codetrail::scip_index::import_scip_json(&workspace, &scip_path).unwrap();
    assert_eq!(import_json["index"]["storageBackend"], "lancedb");
    assert!(dir.path().join(".codetrail/index.lance").is_dir());

    let defs = codetrail()
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
    assert_eq!(defs_json["results"][0]["symbolName"], "needle");
    assert_eq!(defs_json["results"][0]["role"], "definition");
    assert_eq!(defs_json["results"][0]["range"]["start"]["line"], 1);
    assert_eq!(defs_json["results"][0]["sourceTarget"], "src/lib.rs");
    let source = fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(source.contains("fn needle()"));

    let refs = codetrail()
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
    assert_eq!(refs_json["results"][0]["symbolName"], "needle");
    assert_eq!(refs_json["results"][0]["role"], "reference");
    assert_eq!(refs_json["results"][0]["range"]["start"]["line"], 2);
    assert_eq!(refs_json["results"][0]["sourceTarget"], "src/lib.rs");
    assert!(source.contains("needle();"));

    let symbols = codetrail()
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
    assert_eq!(symbols_json["results"][0]["symbolName"], "needle");
    assert_eq!(symbols_json["results"][0]["role"], "definition");
    assert_eq!(symbols_json["results"][0]["range"]["start"]["line"], 1);
    assert_eq!(symbols_json["results"][0]["sourceTarget"], "src/lib.rs");

    let public_defs = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "defs", "needle", "--include-code"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let public_defs_json: Value = serde_json::from_slice(&public_defs).unwrap();
    assert_eq!(public_defs_json["results"][0]["symbolName"], "needle");
    assert_eq!(
        public_defs_json["results"][0]["source"]["rangeKind"],
        "body"
    );
    assert!(public_defs_json["results"][0]["source"]["content"]
        .as_str()
        .unwrap()
        .contains("fn needle()"));
    assert!(public_defs_json["results"][0].get("producer").is_none());
    assert!(public_defs_json["results"][0].get("sourceTarget").is_none());
    assert_no_public_caveats(&public_defs_json);
}

#[test]
fn defs_falls_back_to_parser_after_plain_index_build_without_scip() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn needle() {}\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let defs = codetrail()
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
    assert_eq!(defs_json["results"][0]["symbolName"], "needle");
    assert_eq!(defs_json["results"][0]["role"], "definition");
    assert_eq!(
        defs_json["results"][0]["fallbackReason"],
        "precise_scip_index_unavailable"
    );
    assert_eq!(defs_json["results"][0]["range"]["start"]["line"], 1);
    assert_eq!(defs_json["results"][0]["sourceTarget"], "src/lib.rs");
}

#[test]
fn defs_falls_back_to_parser_for_java_classes() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/SampleService.java"),
        "package example;\n\npublic class SampleService {\n    public void run() {}\n}\n",
    )
    .unwrap();

    let defs = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "SampleService"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();

    assert_eq!(defs_json["reliability"]["level"], "parser_fact");
    assert_eq!(defs_json["results"][0]["name"], "SampleService");
    assert_eq!(defs_json["results"][0]["symbolName"], "SampleService");
    assert_eq!(defs_json["results"][0]["kind"], "class");
    assert_eq!(defs_json["results"][0]["language"], "java");
    assert_eq!(defs_json["results"][0]["role"], "definition");
    assert_eq!(
        defs_json["results"][0]["fallbackReason"],
        "precise_scip_index_unavailable"
    );
    assert_eq!(
        defs_json["results"][0]["path"],
        "src/main/java/example/SampleService.java"
    );
    assert_eq!(defs_json["results"][0]["range"]["start"]["line"], 3);
    assert_eq!(
        defs_json["results"][0]["sourceTarget"],
        "src/main/java/example/SampleService.java"
    );
    let source =
        fs::read_to_string(dir.path().join("src/main/java/example/SampleService.java")).unwrap();
    assert!(source.contains("public class SampleService"));
    assert!(defs_json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "precise_scip_index_unavailable"));
}

#[test]
fn parser_defs_read_closure_covers_python_typescript_and_javascript() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/app.py"),
        "def py_target():\n    pass\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/app.ts"),
        "function tsTarget() { return 1; }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/app.js"),
        "function jsTarget() { return 1; }\n",
    )
    .unwrap();

    for (identifier, language, path, line) in [
        ("py_target", "python", "src/app.py", 1),
        ("tsTarget", "typescript", "src/app.ts", 1),
        ("jsTarget", "javascript", "src/app.js", 1),
    ] {
        let output = codetrail()
            .arg("--path")
            .arg(dir.path())
            .args(["defs", identifier])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["reliability"]["level"], "parser_fact");
        assert_eq!(json["results"][0]["symbolName"], identifier);
        assert_eq!(json["results"][0]["role"], "definition");
        assert_eq!(json["results"][0]["language"], language);
        assert_eq!(json["results"][0]["range"]["start"]["line"], line);
        assert_eq!(json["results"][0]["sourceTarget"], path);
    }
}

#[test]
fn defs_ambiguous_symbol_results_include_grouped_hints() {
    let dir = tempdir().unwrap();
    for module in ["api", "db", "web"] {
        let path = dir.path().join(format!("src/main/java/{module}"));
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("User.java"),
            format!("package {module};\n\npublic class User {{}}\n"),
        )
        .unwrap();
    }

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "User"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ambiguity"]["triggered"], true);
    assert_eq!(json["ambiguity"]["reason"], "multiple_symbol_candidates");
    assert_eq!(json["ambiguity"]["candidateCount"], 3);
    assert!(json["ambiguity"]["groups"]["kind"]
        .as_array()
        .unwrap()
        .iter()
        .any(|group| group["value"] == "class" && group["count"] == 3));
    assert!(json["nextActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["kind"] == "narrow_scope"
            && action["command"].as_str().unwrap().contains("--include")
            && action["command"].as_str().unwrap().contains("--path")));
}

#[test]
fn parser_fallback_supports_java_methods_and_callers() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/SampleService.java"),
        "package example;\n\npublic class SampleService {\n    public void run() {}\n\n    public void start() {\n        run();\n    }\n}\n",
    )
    .unwrap();

    let defs = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();
    assert_eq!(defs_json["results"][0]["name"], "run");
    assert_eq!(defs_json["results"][0]["kind"], "function");
    assert_eq!(defs_json["results"][0]["language"], "java");

    let callers = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert_eq!(callers_json["results"][0]["target"], "run");
    assert_eq!(callers_json["results"][0]["enclosingSymbol"], "start");
    assert_eq!(callers_json["results"][0]["language"], "java");
}

#[test]
fn java_semantic_index_supports_call_hierarchy_and_lombok_overlay() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/SampleService.java"),
        r#"package example;

import lombok.Builder;
import lombok.Data;

public class SampleService {
    @Data
    @Builder
    static class Payload {
        private String name;
    }

    public void run() {}

    public void start() {
        Payload payload = new Payload();
        run();
        payload.getName();
        Payload.builder();
    }
}
"#,
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let callers = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "getName"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert_eq!(callers_json["index"]["source"], "java_semantic");
    assert_eq!(
        callers_json["results"][0]["producer"],
        "java_semantic_resolver"
    );
    assert_eq!(callers_json["results"][0]["target"], "getName");
    assert_eq!(callers_json["results"][0]["enclosingSymbol"], "start");

    let outgoing = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "call-hierarchy",
            "start",
            "--direction",
            "outgoing",
            "--include-overrides",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let outgoing_json: Value = serde_json::from_slice(&outgoing).unwrap();
    assert_eq!(outgoing_json["index"]["source"], "java_semantic");
    let outgoing_calls = outgoing_json["results"][0]["outgoingCalls"]
        .as_array()
        .unwrap();
    assert!(outgoing_calls
        .iter()
        .any(|call| call["to"]["name"] == "run"));
    assert!(outgoing_calls
        .iter()
        .any(|call| call["to"]["name"] == "getName"));
    assert!(outgoing_calls
        .iter()
        .any(|call| call["to"]["name"] == "builder"));

    let incoming = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["call-hierarchy", "getName", "--direction", "incoming"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let incoming_json: Value = serde_json::from_slice(&incoming).unwrap();
    let incoming_calls = incoming_json["results"][0]["incomingCalls"]
        .as_array()
        .unwrap();
    assert!(incoming_calls
        .iter()
        .any(|call| call["from"]["name"] == "start"));
    assert_no_public_caveats(&incoming_json);

    let calls_text = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["calls", "start"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let calls_text = String::from_utf8(calls_text).unwrap();
    assert!(calls_text.contains(
        "SampleService.start() -> SampleService.run()  src/main/java/example/SampleService.java:17"
    ));

    let text = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["call-hierarchy", "start", "--direction", "outgoing"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(text).unwrap();
    assert!(text.contains("Call hierarchy for \"start\""));
    assert!(text.contains(
        "SampleService.start() -> SampleService.run()  src/main/java/example/SampleService.java:17"
    ));
    assert!(text.contains(
        "SampleService.start() -> SampleService.Payload.getName()  src/main/java/example/SampleService.java:18"
    ));
    assert!(
        !text.trim_start().starts_with('{'),
        "call-hierarchy text output must not be raw JSON: {text}"
    );
}

#[test]
fn java_call_hierarchy_incoming_include_overrides_uses_possible_callees() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/p")).unwrap();
    fs::write(
        dir.path().join("src/main/java/p/I.java"),
        "package p;\npublic interface I { void m(); }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/main/java/p/A.java"),
        "package p;\npublic class A implements I { public void m() {} }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/main/java/p/C.java"),
        "package p;\npublic class C { public void call() { I i = new A(); i.m(); } }\n",
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args([
            "call-hierarchy",
            "p.A.m()",
            "--direction",
            "incoming",
            "--include-overrides",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let results = json["results"].as_array().unwrap();
    let a_root = results
        .iter()
        .find(|result| result["root"]["path"] == "src/main/java/p/A.java")
        .expect("expected A.m root");
    let a_incoming = a_root["incomingCalls"].as_array().unwrap();
    assert_eq!(a_incoming.len(), 1, "{a_incoming:?}");
    assert_eq!(a_incoming[0]["from"]["name"], "call");

    let i_root = results
        .iter()
        .find(|result| result["root"]["path"] == "src/main/java/p/I.java")
        .expect("expected I.m root");
    assert_eq!(
        i_root["incomingCalls"].as_array().unwrap().len(),
        1,
        "declared and possible callee entries should not duplicate the same call site"
    );
}

#[test]
fn java_semantic_index_includes_module_generated_sources() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("app/src/main/java/p")).unwrap();
    fs::create_dir_all(
        dir.path()
            .join("app/target/generated-sources/annotations/p/gen"),
    )
    .unwrap();
    fs::write(
        dir.path().join("app/src/main/java/p/C.java"),
        r#"package p;

import p.gen.G;

public class C {
    public void call() {
        new G().m();
    }
}
"#,
    )
    .unwrap();
    fs::write(
        dir.path()
            .join("app/target/generated-sources/annotations/p/gen/G.java"),
        r#"package p.gen;

public class G {
    public void m() {}
}
"#,
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "m"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["index"]["source"], "java_semantic");
    assert_eq!(json["results"][0]["resolveStatus"], "Resolved");
    assert_eq!(json["results"][0]["targetSignature"], "p.gen.G.m()");

    let text = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "m"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(text).unwrap();
    assert!(
        text.contains("C.call() -> p.gen.G.m()  app/src/main/java/p/C.java:7"),
        "{text}"
    );
}

#[test]
fn java_semantic_index_uses_sqlite_not_json_artifacts() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/Sample.java"),
        r#"package example;

public class Sample {
    public void target() {}
    public void caller() {
        target();
    }
}
"#,
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let codetrail_dir = dir.path().join(".codetrail");
    assert!(codetrail_dir.join("java-semantic.sqlite").exists());
    assert!(
        !codetrail_dir.join("java-semantic").exists(),
        "Java semantic must not write legacy artifact directories"
    );

    let callers = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "target"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert!(callers_json["index"]["path"]
        .as_str()
        .unwrap()
        .ends_with(".codetrail/java-semantic.sqlite"));
    assert_eq!(
        callers_json["results"][0]["targetSignature"],
        "Sample.target()"
    );
}

#[test]
fn parser_fallback_supports_swift_symbols_defs_and_callers() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Sources/App")).unwrap();
    fs::write(
        dir.path().join("Package.swift"),
        "// swift-tools-version: 6.0\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("Sources/App/App.swift"),
        r#"
import Vapor

protocol Runnable {
    func run()
}

struct Worker: Runnable {
    func run() {
        helper()
        UserController()
    }
}

final class UserController {
    init() {}
}

func helper() {}
"#,
    )
    .unwrap();

    let symbols = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["symbols", "Worker"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let symbols_json: Value = serde_json::from_slice(&symbols).unwrap();
    assert_eq!(symbols_json["reliability"]["level"], "parser_fact");
    assert_eq!(symbols_json["results"][0]["language"], "swift");
    assert_eq!(symbols_json["results"][0]["symbolName"], "Worker");

    let defs = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "helper"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();
    assert_eq!(defs_json["results"][0]["language"], "swift");
    assert_eq!(defs_json["results"][0]["name"], "helper");
    assert_eq!(defs_json["results"][0]["kind"], "function");

    let callers = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "helper"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert_eq!(callers_json["reliability"]["level"], "inferred_candidate");
    assert!(callers_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| result["language"] == "swift" && result["enclosingSymbol"] == "run"));
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

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let calls = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["calls", "alpha"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let calls_json: Value = serde_json::from_slice(&calls).unwrap();
    // graph backend exists now (petgraph), so index is used
    assert_eq!(calls_json["index"]["used"], true);
    assert_eq!(calls_json["reliability"]["level"], "inferred_candidate");
    assert_eq!(calls_json["results"][0]["target"], "beta");
    // producer reflects the graph source (tree-sitter heuristic inside graph)
    let producer = calls_json["results"][0]["producer"].as_str().unwrap_or("");
    assert!(producer.starts_with("graph:"));

    let callers = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "beta"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    // producer reflects the graph source
    let cproducer = callers_json["results"][0]["producer"]
        .as_str()
        .unwrap_or("");
    assert!(cproducer.starts_with("graph:"));
    assert_eq!(callers_json["results"][0]["enclosingSymbol"], "alpha");
}

#[test]
fn callers_after_index_build_matches_qualified_method_target_by_simple_name() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "struct Widget;\n\nimpl Widget {\n    fn run(&self) {\n        self.helper();\n    }\n\n    fn helper(&self) {}\n}\n",
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let callers = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["callers", "helper"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let callers_json: Value = serde_json::from_slice(&callers).unwrap();
    assert_eq!(callers_json["results"][0]["enclosingSymbol"], "run");
    assert!(callers_json["results"][0]["target"]
        .as_str()
        .unwrap()
        .ends_with("helper"));
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

fn write_jvm_scip_index(path: &std::path::Path, include_java: bool, include_kotlin: bool) {
    use codetrail::scip_proto::proto;
    use prost::Message;

    let mut documents = Vec::new();
    if include_java {
        documents.push(proto::Document {
            language: "java".to_string(),
            relative_path: "src/main/java/example/App.java".to_string(),
            occurrences: vec![proto::Occurrence {
                range: vec![1, 13, 1, 16],
                symbol: "local java-app".to_string(),
                symbol_roles: 1,
                ..Default::default()
            }],
            symbols: vec![proto::SymbolInformation {
                symbol: "local java-app".to_string(),
                kind: proto::symbol_information::Kind::Class as i32,
                display_name: "App".to_string(),
                ..Default::default()
            }],
            position_encoding: proto::PositionEncoding::Utf8CodeUnitOffsetFromLineStart as i32,
            ..Default::default()
        });
    }
    if include_kotlin {
        documents.push(proto::Document {
            language: "kotlin".to_string(),
            relative_path: "src/main/kotlin/okhttp3/RealCall.kt".to_string(),
            occurrences: vec![
                proto::Occurrence {
                    range: vec![2, 6, 2, 14],
                    symbol: "local kotlin-realcall".to_string(),
                    symbol_roles: 1,
                    ..Default::default()
                },
                proto::Occurrence {
                    range: vec![3, 6, 3, 37],
                    symbol: "local kotlin-get-response".to_string(),
                    symbol_roles: 1,
                    ..Default::default()
                },
            ],
            symbols: vec![
                proto::SymbolInformation {
                    symbol: "local kotlin-realcall".to_string(),
                    kind: proto::symbol_information::Kind::Class as i32,
                    display_name: "RealCall".to_string(),
                    ..Default::default()
                },
                proto::SymbolInformation {
                    symbol: "local kotlin-get-response".to_string(),
                    kind: proto::symbol_information::Kind::Method as i32,
                    display_name: "getResponseWithInterceptorChain".to_string(),
                    ..Default::default()
                },
            ],
            position_encoding: proto::PositionEncoding::Utf8CodeUnitOffsetFromLineStart as i32,
            ..Default::default()
        });
    }

    let index = proto::Index {
        metadata: Some(proto::Metadata {
            version: proto::ProtocolVersion::UnspecifiedProtocolVersion as i32,
            tool_info: Some(proto::ToolInfo {
                name: "test-jvm-indexer".to_string(),
                version: "0.1.0".to_string(),
                arguments: vec![],
            }),
            project_root: "file:///test".to_string(),
            text_document_encoding: proto::TextEncoding::Utf8 as i32,
        }),
        documents,
        ..Default::default()
    };

    let mut buf = Vec::new();
    index.encode(&mut buf).unwrap();
    fs::write(path, buf).unwrap();
}

fn write_java_scip_index_for_paths(path: &std::path::Path, rel_paths: &[&str]) {
    use codetrail::scip_proto::proto;
    use prost::Message;

    let documents = rel_paths
        .iter()
        .enumerate()
        .map(|(index, rel_path)| {
            let symbol = format!("local java-app-{index}");
            proto::Document {
                language: "java".to_string(),
                relative_path: (*rel_path).to_string(),
                occurrences: vec![proto::Occurrence {
                    range: vec![1, 13, 1, 16],
                    symbol: symbol.clone(),
                    symbol_roles: 1,
                    ..Default::default()
                }],
                symbols: vec![proto::SymbolInformation {
                    symbol,
                    kind: proto::symbol_information::Kind::Class as i32,
                    display_name: "App".to_string(),
                    ..Default::default()
                }],
                position_encoding: proto::PositionEncoding::Utf8CodeUnitOffsetFromLineStart as i32,
                ..Default::default()
            }
        })
        .collect();
    let index = proto::Index {
        metadata: Some(proto::Metadata {
            version: proto::ProtocolVersion::UnspecifiedProtocolVersion as i32,
            tool_info: Some(proto::ToolInfo {
                name: "test-jvm-indexer".to_string(),
                version: "0.1.0".to_string(),
                arguments: vec![],
            }),
            project_root: "file:///test".to_string(),
            text_document_encoding: proto::TextEncoding::Utf8 as i32,
        }),
        documents,
        ..Default::default()
    };

    let mut buf = Vec::new();
    index.encode(&mut buf).unwrap();
    fs::write(path, buf).unwrap();
}

fn write_java_mapper_scip_index(path: &std::path::Path) {
    use codetrail::scip_proto::proto;
    use prost::Message;

    let index = proto::Index {
        metadata: Some(proto::Metadata {
            version: proto::ProtocolVersion::UnspecifiedProtocolVersion as i32,
            tool_info: Some(proto::ToolInfo {
                name: "test-jvm-indexer".to_string(),
                version: "0.1.0".to_string(),
                arguments: vec![],
            }),
            project_root: "file:///test".to_string(),
            text_document_encoding: proto::TextEncoding::Utf8 as i32,
        }),
        documents: vec![proto::Document {
            language: "java".to_string(),
            relative_path: "src/main/java/com/example/SysUserMapper.java".to_string(),
            occurrences: vec![
                proto::Occurrence {
                    range: vec![2, 12, 2, 33],
                    symbol: "local java-select-user-by-login-name".to_string(),
                    symbol_roles: 1,
                    ..Default::default()
                },
                proto::Occurrence {
                    range: vec![4, 18, 4, 31],
                    symbol: "local java-sys-user-result".to_string(),
                    symbol_roles: 0,
                    ..Default::default()
                },
            ],
            symbols: vec![
                proto::SymbolInformation {
                    symbol: "local java-select-user-by-login-name".to_string(),
                    kind: proto::symbol_information::Kind::Method as i32,
                    display_name: "selectUserByLoginName".to_string(),
                    ..Default::default()
                },
                proto::SymbolInformation {
                    symbol: "local java-sys-user-result".to_string(),
                    kind: proto::symbol_information::Kind::Class as i32,
                    display_name: "SysUserResult".to_string(),
                    ..Default::default()
                },
            ],
            position_encoding: proto::PositionEncoding::Utf8CodeUnitOffsetFromLineStart as i32,
            ..Default::default()
        }],
        ..Default::default()
    };

    let mut buf = Vec::new();
    index.encode(&mut buf).unwrap();
    fs::write(path, buf).unwrap();
}

fn build_native_scip_db_from_file(
    root: &std::path::Path,
    scip_path: &std::path::Path,
) -> std::path::PathBuf {
    let workspace = codetrail::workspace::Workspace::discover(root).unwrap();
    let scip_index = codetrail::scip::parse_native_scip(scip_path).unwrap();
    let db_path = codetrail::scip_index::native_db_path(&workspace);
    codetrail::scip::build_occurrences_db(
        &scip_index,
        &db_path,
        &workspace.snapshot_id,
        &workspace.root,
    )
    .unwrap();
    db_path
}

#[test]
fn manual_scip_commands_are_not_registered() {
    let dir = tempdir().unwrap();

    for subcommand in ["import-scip", "generate-scip"] {
        let output = raw_codetrail()
            .arg("--path")
            .arg(dir.path())
            .args(["--output", "json"])
            .args(["index", subcommand])
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(json["results"], json!([]));
        assert_eq!(json["error"]["code"], "cli_usage_error");
        assert_no_public_caveats(&json);
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains(subcommand));
    }
}

#[test]
fn prebuilt_native_scip_db_drives_precise_defs_refs_and_symbols() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn needle() {}\nfn main() { needle(); }\n",
    )
    .unwrap();

    let scip_path = dir.path().join("index.scip");
    codetrail::scip::write_minimal_test_index(&scip_path).unwrap();

    let db_path = build_native_scip_db_from_file(dir.path(), &scip_path);
    assert!(db_path.is_file());

    // defs
    let defs = codetrail()
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
    assert_eq!(defs_json["index"]["source"], "scip_native");

    // refs
    let refs = codetrail()
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
    assert_eq!(refs_json["index"]["source"], "scip_native");

    let missing_refs = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["refs", "missing"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let missing_refs_json: Value = serde_json::from_slice(&missing_refs).unwrap();
    assert_eq!(missing_refs_json["reliability"]["level"], "precise_fact");
    assert!(missing_refs_json["results"].as_array().unwrap().is_empty());
    assert!(!missing_refs_json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "precise_scip_index_unavailable"));

    // symbols
    let symbols = codetrail()
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
fn prebuilt_native_scip_db_preserves_kotlin_language_for_precise_results() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/main/kotlin/okhttp3")).unwrap();
    fs::write(
        dir.path().join("src/main/kotlin/okhttp3/RealCall.kt"),
        "package okhttp3\n\nclass RealCall {\n  fun getResponseWithInterceptorChain() {}\n}\n",
    )
    .unwrap();

    let scip_path = dir.path().join("index.scip");
    write_jvm_scip_index(&scip_path, false, true);
    let db_path = build_native_scip_db_from_file(dir.path(), &scip_path);
    assert!(db_path.is_file());

    let symbols = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["symbols", "RealCall"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let symbols_json: Value = serde_json::from_slice(&symbols).unwrap();
    assert_eq!(symbols_json["reliability"]["level"], "precise_fact");
    assert_eq!(symbols_json["results"][0]["language"], "kotlin");
    assert_eq!(symbols_json["results"][0]["name"], "RealCall");

    let defs = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "getResponseWithInterceptorChain"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();
    assert_eq!(defs_json["reliability"]["level"], "precise_fact");
    assert_eq!(defs_json["results"][0]["language"], "kotlin");
    assert_eq!(
        defs_json["results"][0]["name"],
        "getResponseWithInterceptorChain"
    );
}

#[cfg(unix)]
#[test]
fn mixed_java_kotlin_gradle_root_runs_scip_java_once_for_both_languages() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("settings.gradle.kts"),
        "pluginManagement {}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("build.gradle.kts"),
        "plugins { kotlin(\"jvm\") version \"1.9.0\" }\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src/main/java/example")).unwrap();
    fs::write(
        dir.path().join("src/main/java/example/App.java"),
        "package example;\npublic class App {}\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src/main/kotlin/okhttp3")).unwrap();
    fs::write(
        dir.path().join("src/main/kotlin/okhttp3/RealCall.kt"),
        "package okhttp3\nclass RealCall { fun getResponseWithInterceptorChain() {} }\n",
    )
    .unwrap();

    let fixture = dir.path().join("fixture.scip");
    write_jvm_scip_index(&fixture, true, true);
    let count_file = dir.path().join("provider-count.txt");
    let provider = dir.path().join("fake-scip-java");
    fs::write(
        &provider,
        r#"#!/bin/sh
set -eu
echo run >> "$CODETRAIL_TEST_PROVIDER_COUNT"
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
    shift
    out="$1"
  fi
  shift || true
done
cp "$CODETRAIL_TEST_SCIP_FIXTURE" "$out"
"#,
    )
    .unwrap();
    make_executable(&provider);

    raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "index", "build"])
        .env(
            "CODETRAIL_SCIP_KOTLIN",
            provider.to_string_lossy().to_string(),
        )
        .env(
            "CODETRAIL_TEST_SCIP_FIXTURE",
            fixture.to_string_lossy().to_string(),
        )
        .env(
            "CODETRAIL_TEST_PROVIDER_COUNT",
            count_file.to_string_lossy().to_string(),
        )
        .assert()
        .success();

    let count = fs::read_to_string(&count_file).unwrap();
    assert_eq!(count.lines().count(), 1, "{count}");

    let status = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "index", "status"])
        .env(
            "CODETRAIL_SCIP_KOTLIN",
            provider.to_string_lossy().to_string(),
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_json: Value = serde_json::from_slice(&status).unwrap();
    let manifests = status_json["results"][0]["semanticManifests"]
        .as_array()
        .unwrap();
    assert!(manifests.iter().any(|manifest| {
        manifest["language"] == "java"
            && manifest["providerName"] == "scip-java"
            && manifest["state"] == "fresh"
    }));
    assert!(manifests.iter().any(|manifest| {
        manifest["language"] == "kotlin"
            && manifest["providerName"] == "scip-java"
            && manifest["state"] == "fresh"
    }));
}

#[cfg(unix)]
#[test]
fn maven_reactor_roots_run_scip_java_once_at_aggregator_root() {
    let dir = tempdir().unwrap();
    let modules = [
        "ruoyi-admin",
        "ruoyi-framework",
        "ruoyi-system",
        "ruoyi-generator",
        "ruoyi-quartz",
        "ruoyi-common",
    ];
    let module_xml = modules
        .iter()
        .map(|module| format!("    <module>{module}</module>\n"))
        .collect::<String>();
    fs::write(
        dir.path().join("pom.xml"),
        format!("<project><modules>\n{module_xml}</modules></project>\n"),
    )
    .unwrap();
    let mut scip_paths = Vec::new();
    for module in modules {
        fs::create_dir_all(dir.path().join(module).join("src/main/java/com/ruoyi")).unwrap();
        fs::write(
            dir.path().join(module).join("pom.xml"),
            "<project><artifactId>module</artifactId></project>\n",
        )
        .unwrap();
        fs::write(
            dir.path()
                .join(module)
                .join("src/main/java/com/ruoyi/App.java"),
            "package com.ruoyi;\npublic class App {}\n",
        )
        .unwrap();
        scip_paths.push(format!("{module}/src/main/java/com/ruoyi/App.java"));
    }

    let fixture = dir.path().join("reactor.scip");
    let scip_refs = scip_paths.iter().map(String::as_str).collect::<Vec<_>>();
    write_java_scip_index_for_paths(&fixture, &scip_refs);
    let log_file = dir.path().join("provider.log");
    let provider = dir.path().join("fake-scip-java");
    fs::write(
        &provider,
        r#"#!/bin/sh
set -eu
{
  echo "cwd=$PWD"
  printf 'args='
  printf '%s ' "$@"
  printf '\n'
} >> "$CODETRAIL_TEST_PROVIDER_LOG"
echo "reactor stdout"
echo "reactor stderr" >&2
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
    shift
    out="$1"
  fi
  shift || true
done
cp "$CODETRAIL_TEST_SCIP_FIXTURE" "$out"
"#,
    )
    .unwrap();
    make_executable(&provider);

    raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "index", "build"])
        .env(
            "CODETRAIL_SCIP_JAVA",
            provider.to_string_lossy().to_string(),
        )
        .env(
            "CODETRAIL_TEST_SCIP_FIXTURE",
            fixture.to_string_lossy().to_string(),
        )
        .env(
            "CODETRAIL_TEST_PROVIDER_LOG",
            log_file.to_string_lossy().to_string(),
        )
        .assert()
        .success();

    let provider_log = fs::read_to_string(&log_file).unwrap();
    assert_eq!(
        provider_log
            .lines()
            .filter(|line| line.starts_with("cwd="))
            .count(),
        1,
        "{provider_log}"
    );
    let canonical_dir = fs::canonicalize(dir.path()).unwrap();
    assert!(
        provider_log.contains(&format!("cwd={}", canonical_dir.display())),
        "{provider_log}"
    );
    assert!(provider_log.contains("--build-tool Maven"));
    assert!(provider_log.contains("--targetroot "));
    assert!(provider_log.contains("-- --batch-mode clean verify -DskipTests -DskipITs"));

    let status = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "index", "status"])
        .env(
            "CODETRAIL_SCIP_JAVA",
            provider.to_string_lossy().to_string(),
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_json: Value = serde_json::from_slice(&status).unwrap();
    let manifests = status_json["results"][0]["semanticManifests"]
        .as_array()
        .unwrap();
    let fresh_java_roots = manifests
        .iter()
        .filter(|manifest| {
            manifest["language"] == "java"
                && manifest["providerName"] == "scip-java"
                && manifest["state"] == "fresh"
        })
        .count();
    assert_eq!(fresh_java_roots, 6, "{manifests:#?}");
    let roots = status_json["results"][0]["semanticStatus"]["roots"]
        .as_array()
        .unwrap();
    assert!(
        roots.iter().all(|root| root["rootId"] != "java:."),
        "{roots:#?}"
    );

    let summary = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "index", "status", "--summary"])
        .env(
            "CODETRAIL_SCIP_JAVA",
            provider.to_string_lossy().to_string(),
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let summary_json: Value = serde_json::from_slice(&summary).unwrap();
    let coverage = summary_json["results"][0]["semanticStatus"]["languageCoverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|coverage| coverage["language"] == "java")
        .unwrap();
    assert_eq!(coverage["precise"], "fresh");
    assert_eq!(coverage["mode"], "precise");
    assert_eq!(coverage["rootCount"], 6);

    let provider_output = dir
        .path()
        .join(".codetrail/scip/worktree_non-git/provider-output");
    assert!(provider_output
        .join("java-root-scip-java.stdout.log")
        .exists());
    assert!(provider_output
        .join("java-root-scip-java.stderr.log")
        .exists());
    let command_json: Value = serde_json::from_slice(
        &fs::read(provider_output.join("java-root-scip-java.command.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        command_json["cwd"].as_str(),
        Some(canonical_dir.to_string_lossy().as_ref())
    );
    assert_eq!(command_json["exitCode"], 0);
}

#[test]
fn native_scip_precise_results_respect_hidden_and_no_ignore() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".hidden")).unwrap();
    fs::create_dir_all(dir.path().join("target/generated")).unwrap();
    let source = "fn needle() {}\nfn main() { needle(); }\n";
    fs::write(dir.path().join(".hidden/lib.rs"), source).unwrap();
    fs::write(dir.path().join("target/generated/lib.rs"), source).unwrap();

    let scip_path = dir.path().join("index.scip");
    write_scip_index_for_paths(&scip_path, &[".hidden/lib.rs", "target/generated/lib.rs"]);

    build_native_scip_db_from_file(dir.path(), &scip_path);

    let default_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["refs", "needle"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let default_json: Value = serde_json::from_slice(&default_output).unwrap();
    assert_eq!(default_json["index"]["source"], "scip_native");
    assert!(default_json["results"].as_array().unwrap().is_empty());

    let hidden_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--hidden")
        .args(["refs", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let hidden_json: Value = serde_json::from_slice(&hidden_output).unwrap();
    let hidden_paths: Vec<&str> = hidden_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["path"].as_str())
        .collect();
    assert_eq!(hidden_paths, vec![".hidden/lib.rs"]);

    let expanded_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--hidden")
        .arg("--no-ignore")
        .args(["refs", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let expanded_json: Value = serde_json::from_slice(&expanded_output).unwrap();
    let expanded_paths: Vec<&str> = expanded_json["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["path"].as_str())
        .collect();
    assert!(expanded_paths.contains(&".hidden/lib.rs"));
    assert!(expanded_paths.contains(&"target/generated/lib.rs"));
    assert_eq!(expanded_paths.len(), 2);
}

#[test]
fn native_scip_stale_detection_simulates_staleness_by_db_removal() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn needle() {}\nfn main() { needle(); }\n",
    )
    .unwrap();

    let scip_path = dir.path().join("index.scip");
    codetrail::scip::write_minimal_test_index(&scip_path).unwrap();

    let db_path = build_native_scip_db_from_file(dir.path(), &scip_path);
    assert!(db_path.is_file());
    fs::remove_file(&db_path).unwrap();

    // After DB removal, queries MUST fall back to tree-sitter,
    // and tree-sitter results are NEVER marked as precise
    let defs = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["defs", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let defs_json: Value = serde_json::from_slice(&defs).unwrap();
    assert_ne!(defs_json["reliability"]["level"], "precise_fact");
    assert_eq!(defs_json["reliability"]["level"], "parser_fact");
    assert_eq!(defs_json["reliability"]["exact"], false);
    assert_eq!(defs_json["results"][0]["producer"], "tree_sitter_parser");
}

fn write_scip_index_for_paths(path: &std::path::Path, rel_paths: &[&str]) {
    use codetrail::scip_proto::proto;
    use prost::Message;

    let documents = rel_paths
        .iter()
        .map(|rel_path| proto::Document {
            language: "rust".to_string(),
            relative_path: (*rel_path).to_string(),
            occurrences: vec![
                proto::Occurrence {
                    range: vec![0, 3, 0, 9],
                    symbol: "local 1".to_string(),
                    symbol_roles: 1,
                    ..Default::default()
                },
                proto::Occurrence {
                    range: vec![1, 12, 1, 18],
                    symbol: "local 1".to_string(),
                    symbol_roles: 0,
                    ..Default::default()
                },
            ],
            symbols: vec![proto::SymbolInformation {
                symbol: "local 1".to_string(),
                kind: proto::symbol_information::Kind::Function as i32,
                display_name: "needle".to_string(),
                ..Default::default()
            }],
            position_encoding: proto::PositionEncoding::Utf8CodeUnitOffsetFromLineStart as i32,
            ..Default::default()
        })
        .collect();

    let index = proto::Index {
        metadata: Some(proto::Metadata {
            version: proto::ProtocolVersion::UnspecifiedProtocolVersion as i32,
            tool_info: Some(proto::ToolInfo {
                name: "test-indexer".to_string(),
                version: "0.1.0".to_string(),
                arguments: vec![],
            }),
            project_root: "file:///test".to_string(),
            text_document_encoding: proto::TextEncoding::Utf8 as i32,
        }),
        documents,
        ..Default::default()
    };

    let mut buf = Vec::new();
    index.encode(&mut buf).unwrap();
    fs::write(path, &buf).unwrap();
}

#[test]
fn watch_once_reconcile_detects_file_changes() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    // Build an index first to create a snapshot
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // run watch --once to check reconcile
    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["watch", "--once"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "watch");
    // Should have results containing reconcile info
    let results = json["results"].as_array().unwrap();
    assert!(!results.is_empty());
    let reconcile = &results[0];
    assert_eq!(
        reconcile["stale"], false,
        "fresh after build should not be stale"
    );
    assert_eq!(reconcile["addedFiles"].as_array().unwrap().len(), 0);
    assert_eq!(reconcile["deletedFiles"].as_array().unwrap().len(), 0);

    // Modify the file and run watch --once again
    fs::write(dir.path().join("sample.txt"), "hello\nworld\n").unwrap();

    let output2 = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["watch", "--once"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json2: Value = serde_json::from_slice(&output2).unwrap();

    assert_eq!(json2["ok"], true);
    let results2 = json2["results"].as_array().unwrap();
    let reconcile2 = &results2[0];
    assert!(
        reconcile2["stale"].as_bool().unwrap(),
        "modified file should be detected as stale"
    );
    let dirty = reconcile2["dirtyFiles"].as_array().unwrap();
    assert!(!dirty.is_empty());
    assert_eq!(dirty[0]["path"], "sample.txt");
    assert_eq!(dirty[0]["reason"], "file_hash_mismatch");
}

#[test]
fn watch_status_output_format() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["watch", "--status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "watch");
    let results = json["results"].as_array().unwrap();
    // results[0] IS the watcher status object directly
    let watcher = &results[0];
    assert!(watcher.is_object());
    assert_eq!(watcher["running"], false);
    assert_eq!(watcher["state"], "idle");
    assert!(watcher["root"].is_string());
    assert!(watcher["queueLength"].is_number());
    assert!(watcher["stale"].is_boolean());
    // lastEventAt should be null (no events collected)
    assert!(watcher["lastEventAt"].is_null());
    // lastReconcileAt should be null (--status doesn't run reconcile)
    assert!(watcher["lastReconcileAt"].is_null());
    assert_eq!(watcher["mode"], "reconcile_on_demand");
    // Should have overlay sub-object
    let overlay = &watcher["overlay"];
    assert!(overlay.is_object());
    assert!(overlay["dirtyFiles"].is_array());
    assert!(overlay["addedFiles"].is_array());
    assert!(overlay["deletedFiles"].is_array());
}

#[test]
fn watcher_does_not_modify_git_staged_state() {
    let dir = tempdir().unwrap();
    // Initialize a git repo
    std::process::Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["config", "user.name", "Test"])
        .output()
        .unwrap();

    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    // Stage the file
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "sample.txt"])
        .output()
        .unwrap();

    // Run watch --once
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["watch", "--once"])
        .assert()
        .success();

    // Verify git staged state is still as expected — file should still be staged
    let status_output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    // sample.txt should still show as staged (A or M in index)
    assert!(
        status_str.contains("sample.txt"),
        "git status should still show sample.txt"
    );
    // The file should not be unstaged by watcher
}

#[test]
fn watch_run_once_returns_reconcile_info_without_modifying_files() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "content before watch\n").unwrap();

    let original_content = fs::read_to_string(dir.path().join("sample.txt")).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["watch", "--once"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], true);

    // Verify file content is unchanged
    let after_content = fs::read_to_string(dir.path().join("sample.txt")).unwrap();
    assert_eq!(
        original_content, after_content,
        "watch should not modify file content"
    );

    // Verify the response has reconcile information
    let results = json["results"].as_array().unwrap();
    let reconcile = &results[0];
    assert!(reconcile["totalFilesScanned"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn serve_no_watch_returns_service_status() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["serve", "--no-watch"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "serve");
    let results = json["results"].as_array().unwrap();
    let service = &results[0]["service"];
    assert!(service.is_object());
    assert_eq!(service["running"], false);
    assert_eq!(service["watchEnabled"], false);
    assert_eq!(service["mode"], "cli_query_service");
    assert!(service["root"].is_string());
    assert!(service["snapshot"].is_string());
    assert!(json["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn public_serve_no_watch_returns_note_without_caveat() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    let output = raw_codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--output", "json", "serve", "--no-watch"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_no_public_caveats(&json);
    assert!(json["results"][0]["service"]["note"]
        .as_str()
        .unwrap()
        .contains("HTTP/MCP adapters"));
}

#[test]
fn serve_with_watch_includes_watcher_status() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["serve"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["ok"], true);
    let results = json["results"].as_array().unwrap();
    let service = &results[0]["service"];
    assert_eq!(service["watchEnabled"], true);
    // When watch is enabled, watcher status should be included
    // but watcher might fail to init, so it's optional
    if let Some(watcher) = service.get("watcher") {
        assert!(watcher.is_object());
        assert!(watcher["root"].is_string());
    }
}

// ---------------------------------------------------------------------------
// MCP integration tests
// ---------------------------------------------------------------------------

#[test]
fn mcp_subcommand_is_hidden_from_help() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(output).unwrap();
    assert!(!help
        .lines()
        .any(|line| line.trim_start().starts_with("mcp")));
}

#[test]
fn call_graph_help_uses_user_facing_boundary_language() {
    for command in ["calls", "callers", "call-hierarchy"] {
        let output = raw_codetrail()
            .args([command, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let help = String::from_utf8(output).unwrap();

        assert!(
            !help.contains("inferred_candidate"),
            "{command} help: {help}"
        );
        assert!(
            help.contains("navigation evidence"),
            "{command} help: {help}"
        );
        assert!(help.contains("may be incomplete"), "{command} help: {help}");
        assert!(help.contains("verify call sites"), "{command} help: {help}");
    }
}

#[test]
fn mcp_stdio_legacy_find_returns_tool_error() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() {\n    let needle = 42;\n}\n",
    )
    .unwrap();

    let find_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "codetrail_find",
            "arguments": { "text": "needle" }
        }
    });
    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("mcp")
        .write_stdin(format!("{find_request}\n"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    let lines: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert_eq!(lines[0]["result"]["isError"], true);
    let mcp_find: Value =
        serde_json::from_str(lines[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(mcp_find["error"]["code"], "unknown_tool");
    assert_no_public_caveats(&mcp_find);
}

#[test]
fn mcp_stdio_explore_node_is_not_registered_and_returns_tool_error() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn alpha() {\n    beta();\n}\n\nfn beta() {}\n",
    )
    .unwrap();

    let list_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list"
    });
    let explore_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "codetrail_explore_node",
            "arguments": {
                "query": "alpha",
                "maxCandidates": 5,
                "snippetLines": 4,
                "relationLimit": 4
            }
        }
    });
    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .arg("mcp")
        .write_stdin(format!("{list_request}\n{explore_request}\n"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    let lines: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    let tools = lines[0]["result"]["tools"].as_array().unwrap();
    assert!(!tools
        .iter()
        .any(|tool| tool["name"] == "codetrail_explore_node"));
    assert!(!tools
        .iter()
        .any(|tool| tool["name"] == "codetrail_explore_flow"));

    assert_eq!(lines[1]["result"]["isError"], true);
    let explore: Value =
        serde_json::from_str(lines[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(explore.get("results").is_some());
    assert!(explore.get("page").is_some());
    assert!(explore.get("error").is_some());
    assert_eq!(explore.as_object().unwrap().len(), 3);
    assert!(explore["results"].as_array().unwrap().is_empty());
    assert_eq!(explore["error"]["code"], "unknown_tool");
    assert_no_public_caveats(&explore);
}

// ---------------------------------------------------------------------------
// MR-08 Remote/Pack mode tests
// ---------------------------------------------------------------------------

#[test]
fn index_pack_produces_valid_archive_with_checksums() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    // Build index first
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Pack
    let archive_path = dir.path().join("output.tar.gz");
    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "pack", "--output"])
        .arg(&archive_path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).unwrap();
    let packed = &json["results"][0];
    assert_eq!(packed["packed"], true);
    assert!(packed["archiveSize"].as_u64().unwrap() > 0);
    assert_eq!(packed["source"], "packed_remote");

    // Verify archive exists
    assert!(archive_path.exists());
    assert!(archive_path.metadata().unwrap().len() > 0);

    // Verify it's a valid gzip file (magic bytes 1f 8b)
    let archive_bytes = fs::read(&archive_path).unwrap();
    assert_eq!(&archive_bytes[0..2], &[0x1f, 0x8b]);
}

#[test]
fn index_unpack_extracts_to_remote_dir_does_not_touch_working_or_staged() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    // Build index
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Pack
    let archive_path = dir.path().join("output.tar.gz");
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "pack", "--output"])
        .arg(&archive_path)
        .assert()
        .success();

    let codetrail_dir = dir.path().join(".codetrail");
    // Clean local index to simulate fresh workspace without local index
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "clean"])
        .assert()
        .success();

    // Unpack
    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).unwrap();
    let unpacked = &json["results"][0];
    assert_eq!(unpacked["unpacked"], true);
    assert_eq!(unpacked["source"], "remote_unpacked");

    // Verify remote dir exists
    let remote_dir = codetrail_dir.join("remote");
    assert!(remote_dir.exists());

    // snapshots may or may not exist after clean, but remote must be separate
    let remote_entries: Vec<_> = remote_dir
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!remote_entries.is_empty(), "remote dir should have content");

    // Verify provenance.json exists
    for entry in &remote_entries {
        let path = entry.path();
        if path.is_dir() {
            let prov = path.join("provenance.json");
            if prov.exists() {
                let prov_content = fs::read_to_string(&prov).unwrap();
                assert!(prov_content.contains("remote_unpacked"));
                assert!(path.join("files.parquet").exists());
                assert!(path.join("text/docs.idx").exists());
                assert!(path.join("text/grams.idx").exists());
                return;
            }
        }
    }
    panic!("provenance.json not found in remote directory");
}

#[test]
fn remote_snapshot_never_overrides_local_when_local_is_fresh() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    // Build local index
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Pack
    let archive_path = dir.path().join("output.tar.gz");
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "pack", "--output"])
        .arg(&archive_path)
        .assert()
        .success();

    // Unpack to create remote snapshot
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .success();

    // Local snapshot should still be active (not the remote one)
    let status_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&status_output).unwrap();
    let status = &json["results"][0];
    // Local snapshot exists and is fresh
    assert_eq!(status["exists"], true);
    assert!(status["fresh"].as_bool().unwrap_or(false));
    // Remote should be listed but separate
    if let Some(remote) = status.get("remote") {
        assert!(remote.is_array());
    }
}

#[test]
fn remote_query_is_used_when_local_is_clean_missing() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { let _ = \"needle\"; }\n",
    )
    .unwrap();

    // Build and pack
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let archive_path = dir.path().join("output.tar.gz");
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "pack", "--output"])
        .arg(&archive_path)
        .assert()
        .success();

    // Clean local index
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "clean"])
        .assert()
        .success();

    // Unpack remote
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .success();

    // Now find should use remote index (since local is missing)
    let find_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&find_output).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["index"]["used"], true);
    assert_eq!(json["index"]["source"], "text_index:remote");
    // Should find the file even with local index deleted (via remote)
    assert!(!json["results"].as_array().unwrap().is_empty());
    assert_eq!(json["results"][0]["path"], "src/main.rs");
}

#[test]
fn remote_fallback_respects_packed_scan_scope() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("docs")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { /* srctoken */ }\n",
    )
    .unwrap();
    fs::write(dir.path().join("docs/guide.md"), "needle docs\n").unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--include", "src", "index", "build"])
        .assert()
        .success();

    let archive_path = dir.path().join("output.tar.gz");
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "pack", "--output"])
        .arg(&archive_path)
        .assert()
        .success();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "clean"])
        .assert()
        .success();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .success();

    let unscoped_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["find", "needle"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let unscoped_json: Value = serde_json::from_slice(&unscoped_output).unwrap();
    assert_eq!(unscoped_json["index"]["used"], false);
    assert_eq!(unscoped_json["results"][0]["path"], "docs/guide.md");

    let scoped_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["--include", "src", "find", "srctoken"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let scoped_json: Value = serde_json::from_slice(&scoped_output).unwrap();
    assert_eq!(scoped_json["index"]["used"], true);
    assert_eq!(scoped_json["index"]["source"], "text_index:remote");
    assert_eq!(scoped_json["results"][0]["path"], "src/main.rs");
}

#[test]
fn remote_mismatch_labels_results_as_unverified() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { let _ = \"needle\"; }\n",
    )
    .unwrap();

    // Build index
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    // Pack
    let archive_path = dir.path().join("output.tar.gz");
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "pack", "--output"])
        .arg(&archive_path)
        .assert()
        .success();

    // Modify local file so remote won't match
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { let _ = \"changed\"; }\n",
    )
    .unwrap();

    // Clean and unpack remote
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "clean"])
        .assert()
        .success();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .success();

    // Query should still work via remote but should indicate remote_unverified
    // (the remote records won't match changed local files)
    let status = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&status).unwrap();
    let status_val = &json["results"][0];
    // Remote should have remoteVerified: false
    if let Some(remote) = status_val.get("remote") {
        if let Some(arr) = remote.as_array() {
            if let Some(first) = arr.first() {
                // remoteVerified should be false since file hashes don't match
                assert_eq!(first["remoteVerified"], json!(false));
                let files_output = codetrail()
                    .arg("--path")
                    .arg(dir.path())
                    .args(["files", "main"])
                    .assert()
                    .success()
                    .get_output()
                    .stdout
                    .clone();
                let files_json: Value = serde_json::from_slice(&files_output).unwrap();
                assert_eq!(files_json["index"]["source"], "text_index:remote");
                assert_eq!(files_json["index"]["remote_verified"], false);
                assert_eq!(files_json["results"][0]["indexFresh"], false);
                assert_eq!(
                    files_json["results"][0]["sourceReason"],
                    "indexed_unverified"
                );
                return;
            }
        }
    }
    panic!("remote snapshot not found or remoteVerified not present");
}

#[test]
fn legacy_parquet_remote_snapshot_is_unverified_without_error() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { let _ = \"needle\"; }\n",
    )
    .unwrap();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "build"])
        .assert()
        .success();

    let archive_path = dir.path().join("output.tar.gz");
    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "pack", "--output"])
        .arg(&archive_path)
        .assert()
        .success();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "clean"])
        .assert()
        .success();

    codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .success();

    let remote_dir = fs::read_dir(dir.path().join(".codetrail").join("remote"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().is_dir())
        .expect("remote snapshot exists")
        .path();
    fs::write(remote_dir.join("files.parquet"), b"PAR1legacy").unwrap();

    let status = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&status).unwrap();
    let remote = json["results"][0]["remote"].as_array().unwrap();
    assert_eq!(remote[0]["remoteVerified"], json!(false));

    let files_output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["files", "main"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let files_json: Value = serde_json::from_slice(&files_output).unwrap();
    assert_eq!(files_json["index"]["source"], "text_index:remote");
    assert_eq!(files_json["index"]["remote_verified"], false);
    assert_eq!(
        files_json["results"][0]["sourceReason"],
        "indexed_unverified"
    );
}

// ---------------------------------------------------------------------------
// Security regression tests – issue #164
// ---------------------------------------------------------------------------

#[test]
fn index_unpack_rejects_dotdot_path_traversal() {
    let dir = tempdir().unwrap();

    let archive_data = build_raw_tar_gz(&[("../../escape-repo.txt", b"pwned")]);
    let archive_path = dir.path().join("malicious.tar.gz");
    fs::write(&archive_path, &archive_data).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).unwrap();
    let msg = json["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("..") || msg.contains("escape") || msg.contains("traversal"),
        "expected path-traversal error, got: {msg}"
    );

    // Confirm no file was written outside the temp dir
    assert!(
        !dir.path().join("escape-repo.txt").exists(),
        "traversal file must not have been created"
    );
    assert!(
        !dir.path()
            .parent()
            .unwrap()
            .join("escape-repo.txt")
            .exists(),
        "traversal file must not have been created in parent"
    );
}

#[test]
fn index_unpack_rejects_absolute_path_in_archive() {
    let dir = tempdir().unwrap();

    // absolute path: /tmp/injected.txt — written via raw tar bytes to bypass
    // the tar crate's writer-side path safety check
    let archive_data = build_raw_tar_gz(&[("/tmp/injected.txt", b"injected")]);
    let archive_path = dir.path().join("abs-path.tar.gz");
    fs::write(&archive_path, &archive_data).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).unwrap();
    let msg = json["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("absolute") || msg.contains("injected") || !msg.is_empty(),
        "expected absolute-path rejection, got: {msg}"
    );
}

#[test]
fn index_unpack_rejects_too_many_entries() {
    let dir = tempdir().unwrap();

    // Craft an archive with 1001 entries (> MAX_ARCHIVE_ENTRIES = 1000)
    let content = b"x";
    let entries: Vec<(String, &[u8])> = (0..1001)
        .map(|i| (format!("file_{i:04}.bin"), content as &[u8]))
        .collect();
    let entry_refs: Vec<(&str, &[u8])> = entries.iter().map(|(p, c)| (p.as_str(), *c)).collect();

    let archive_data = build_safe_tar_gz(&entry_refs);
    let archive_path = dir.path().join("too-many.tar.gz");
    fs::write(&archive_path, &archive_data).unwrap();

    let output = codetrail()
        .arg("--path")
        .arg(dir.path())
        .args(["index", "unpack"])
        .arg(&archive_path)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).unwrap();
    let msg = json["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("1000") || msg.contains("entries") || msg.contains("more than"),
        "expected entry-count error, got: {msg}"
    );
}
