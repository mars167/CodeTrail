use std::{collections::HashMap, path::PathBuf};

use serde_json::{json, Value};

use super::reconcile::{DirtyFile, ReconcileResult};

/// Overlay tracks the worktree overlay: which files are dirty compared
/// to the snapshot, and which files have been added or deleted.
///
/// Does NOT execute `git add`, does NOT modify staged state,
/// does NOT generate commit snapshots.
pub struct Overlay {
    #[allow(dead_code)]
    workspace_root: PathBuf,
    /// Map from file path to dirty reason/hash info
    dirty_files: HashMap<String, DirtyFile>,
    /// Files present on disk but not in snapshot
    added_files: Vec<String>,
    /// Files in snapshot but no longer on disk
    deleted_files: Vec<String>,
    stale: bool,
}

impl Overlay {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            dirty_files: HashMap::new(),
            added_files: Vec::new(),
            deleted_files: Vec::new(),
            stale: false,
        }
    }

    /// Update overlay state from a reconcile result.
    pub fn update_from_reconcile(&mut self, result: &ReconcileResult) {
        self.dirty_files.clear();
        for dirty in &result.dirty_files {
            self.dirty_files.insert(dirty.path.clone(), dirty.clone());
        }

        self.added_files = result.added_files.clone();
        self.deleted_files = result.deleted_files.clone();
        self.stale = result.stale;
    }

    /// Check if a specific path is dirty (modified compared to snapshot).
    pub fn is_dirty(&self, path: &str) -> bool {
        self.dirty_files.contains_key(path)
    }

    /// Check if a specific path is newly added (not in snapshot).
    pub fn is_added(&self, path: &str) -> bool {
        self.added_files.iter().any(|p| p == path)
    }

    /// Check if a specific path was deleted (in snapshot but not on disk).
    pub fn is_deleted(&self, path: &str) -> bool {
        self.deleted_files.iter().any(|p| p == path)
    }

    /// Return the dirty file info for a path, if any.
    pub fn dirty_info(&self, path: &str) -> Option<&DirtyFile> {
        self.dirty_files.get(path)
    }

    /// Return all tracked changes as a JSON-serializable structure.
    pub fn status(&self) -> Value {
        let dirty: Vec<Value> = self
            .dirty_files
            .values()
            .map(|d| {
                json!({
                    "path": d.path,
                    "reason": d.reason,
                    "expectedHash": d.expected_hash,
                    "actualHash": d.actual_hash,
                })
            })
            .collect();

        json!({
            "dirtyFiles": dirty,
            "addedFiles": self.added_files,
            "deletedFiles": self.deleted_files,
            "stale": self.stale,
            "dirtyCount": dirty.len(),
            "addedCount": self.added_files.len(),
            "deletedCount": self.deleted_files.len(),
        })
    }

    /// Clear the overlay state (e.g., after a rebuild).
    pub fn clear(&mut self) {
        self.dirty_files.clear();
        self.added_files.clear();
        self.deleted_files.clear();
        self.stale = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_tracks_dirty_files() {
        let mut overlay = Overlay::new(PathBuf::from("/tmp/test"));
        assert!(!overlay.is_dirty("src/main.rs"));

        overlay.dirty_files.insert(
            "src/main.rs".to_string(),
            DirtyFile {
                path: "src/main.rs".to_string(),
                reason: "file_hash_mismatch".to_string(),
                expected_hash: Some("blake3:abc".to_string()),
                actual_hash: Some("blake3:def".to_string()),
            },
        );

        assert!(overlay.is_dirty("src/main.rs"));
        assert!(!overlay.is_dirty("src/lib.rs"));

        let status = overlay.status();
        assert_eq!(status["dirtyCount"], 1);
    }

    #[test]
    fn overlay_tracks_added_and_deleted() {
        let mut overlay = Overlay::new(PathBuf::from("/tmp/test"));
        overlay.added_files = vec!["src/new.rs".to_string()];
        overlay.deleted_files = vec!["src/old.rs".to_string()];

        assert!(overlay.is_added("src/new.rs"));
        assert!(!overlay.is_added("src/main.rs"));
        assert!(overlay.is_deleted("src/old.rs"));
        assert!(!overlay.is_deleted("src/main.rs"));

        let status = overlay.status();
        assert_eq!(status["addedCount"], 1);
        assert_eq!(status["deletedCount"], 1);
    }
}
