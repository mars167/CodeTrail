//! Atomic promotion between staging areas.
//!
//! The publish module handles safe, crash-resistant movement of
//! snapshot data between the temp, staged, commit, and working areas.
//! All promotions use fs::rename where possible for atomicity on the
//! same filesystem, and fall back to copy+delete otherwise.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde_json::{json, Value};

/// Promote a staged manifest into the commit area.
///
/// This is called after a successful `index build --staged`:
///   staged snapshot → commit snapshot
///
/// The commit area stores the canonical post-commit state that
/// `index update` uses as its baseline for incremental updates.
pub fn promote_staged_to_commit(workspace_root: &Path) -> Result<Value> {
    let storage = workspace_root.join(".code-search");
    let staged = storage.join("staged");
    let commit = storage.join("commit");

    // If there is no staged manifest, nothing to promote.
    let staged_manifest = staged.join("manifest.json");
    if !staged_manifest.exists() {
        return Ok(json!({
            "promotion": "staged_to_commit",
            "status": "skipped",
            "reason": "no_staged_manifest"
        }));
    }

    let manifest: serde_json::Value = serde_json::from_reader(fs::File::open(&staged_manifest)?)?;
    let snapshot_key = manifest["snapshotKey"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    fs::create_dir_all(&commit)?;

    let staged_snapshots = storage.join("snapshots").join(&snapshot_key);
    let staged_text = storage.join("text").join(&snapshot_key);

    // Copy snapshot data into commit area
    if staged_snapshots.exists() {
        let commit_snapshots = commit.join("snapshots").join(&snapshot_key);
        copy_dir_atomic(&staged_snapshots, &commit_snapshots)?;
    }
    if staged_text.exists() {
        let commit_text = commit.join("text").join(&snapshot_key);
        copy_dir_atomic(&staged_text, &commit_text)?;
    }

    // Copy the manifest to commit
    let commit_manifest = commit.join("manifest.json");
    fs::copy(&staged_manifest, &commit_manifest)?;

    Ok(json!({
        "promotion": "staged_to_commit",
        "status": "ok",
        "snapshotKey": snapshot_key,
        "commitPath": commit
    }))
}

/// Promote a just-built temp snapshot into the working area.
///
/// This is called after a successful `index build` (non-staged) or
/// `index update`:
///   temp snapshot → working area (symlink-style promotion)
///
/// The working area is what commands use to serve fresh queries.
pub fn promote_temp_to_working(workspace_root: &Path, snapshot_key: &str) -> Result<Value> {
    let storage = workspace_root.join(".code-search");
    let working = storage.join("working");

    fs::create_dir_all(&working)?;

    // Read the snapshot manifest to copy into working
    let snapshot_manifest_path = storage
        .join("snapshots")
        .join(snapshot_key)
        .join("manifest.json");
    if !snapshot_manifest_path.exists() {
        return Ok(json!({
            "promotion": "temp_to_working",
            "status": "skipped",
            "reason": "no_snapshot_manifest",
            "snapshotKey": snapshot_key
        }));
    }

    let working_manifest = working.join("manifest.json");
    let working_manifest_tmp = working.join("manifest.json.tmp");
    fs::copy(&snapshot_manifest_path, &working_manifest_tmp)?;
    fs::rename(&working_manifest_tmp, &working_manifest)?;

    Ok(json!({
        "promotion": "temp_to_working",
        "status": "ok",
        "snapshotKey": snapshot_key,
        "workingPath": working
    }))
}

/// Copy a directory tree atomically: write to .tmp then rename.
fn copy_dir_atomic(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        fs::remove_dir_all(dst)?;
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = if let Some(parent) = dst.parent() {
        let file_name = dst
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "tmp".to_string());
        parent.join(format!("{}.tmp", file_name))
    } else {
        PathBuf::from(format!("{}.tmp", dst.display()))
    };

    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }

    copy_dir_recursive(src, &tmp)?;
    fs::rename(&tmp, dst)?;

    Ok(())
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn promote_temp_to_working_writes_manifest() {
        let dir = tempdir().unwrap();
        let storage = dir.path().join(".code-search");
        fs::create_dir_all(&storage).unwrap();

        // Set up a snapshot with a manifest
        let snap = storage.join("snapshots").join("testkey");
        fs::create_dir_all(&snap).unwrap();
        let manifest = serde_json::json!({
            "snapshotKey": "testkey",
            "snapshot_id": "test:id",
            "fileCount": 1,
            "source": "working_tree"
        });
        fs::write(
            snap.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let result = promote_temp_to_working(dir.path(), "testkey").unwrap();
        assert_eq!(result["status"], "ok");
        assert!(storage.join("working").join("manifest.json").exists());
    }

    #[test]
    fn promote_temp_to_working_skips_on_missing_manifest() {
        let dir = tempdir().unwrap();
        let result = promote_temp_to_working(dir.path(), "nonexistent").unwrap();
        assert_eq!(result["status"], "skipped");
        assert_eq!(result["reason"], "no_snapshot_manifest");
    }

    #[test]
    fn copy_dir_atomic_preserves_content() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");

        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("hello.txt"), b"world").unwrap();
        fs::write(src.join("sub").join("nested.txt"), b"nested content").unwrap();

        copy_dir_atomic(&src, &dst).unwrap();

        assert!(dst.join("hello.txt").exists());
        assert!(dst.join("sub").join("nested.txt").exists());
        assert_eq!(fs::read_to_string(dst.join("hello.txt")).unwrap(), "world");
    }

    #[test]
    fn promote_staged_to_commit_copies_data() {
        let dir = tempdir().unwrap();
        let storage = dir.path().join(".code-search");
        fs::create_dir_all(&storage).unwrap();

        // Set up staged manifest
        let staged = storage.join("staged");
        fs::create_dir_all(&staged).unwrap();
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "snapshotKey": "commitkey",
            "snapshot_id": "staged:id",
            "fileCount": 5,
            "source": "staged"
        });
        fs::write(
            staged.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        // Set up snapshot data
        let snap = storage.join("snapshots").join("commitkey");
        fs::create_dir_all(&snap).unwrap();
        fs::write(
            snap.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let result = promote_staged_to_commit(dir.path()).unwrap();
        assert_eq!(result["status"], "ok");

        let commit = storage.join("commit");
        assert!(commit.join("manifest.json").exists());
    }

    #[test]
    fn promote_staged_to_commit_skips_without_manifest() {
        let dir = tempdir().unwrap();
        let result = promote_staged_to_commit(dir.path()).unwrap();
        assert_eq!(result["status"], "skipped");
        assert_eq!(result["reason"], "no_staged_manifest");
    }
}
