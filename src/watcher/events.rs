use std::path::Path;

use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeEntry {
    pub path: String,
    pub kind: ChangeKind,
    /// Previous path (for rename events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSet {
    pub added: Vec<ChangeEntry>,
    pub modified: Vec<ChangeEntry>,
    pub deleted: Vec<ChangeEntry>,
    pub renamed: Vec<ChangeEntry>,
    pub total_changes: usize,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.total_changes == 0
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

/// Filter out events on paths we should skip (.git, .codetrail, target, node_modules, etc.)
pub fn should_skip_event(event: &Event) -> bool {
    event.paths.iter().any(|p| should_skip_path(p))
}

pub fn should_skip_path(path: &Path) -> bool {
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        matches!(
            value.as_ref(),
            ".git" | ".codetrail" | "target" | "node_modules" | "dist" | ".next" | ".DS_Store"
        )
    })
}

/// Normalize raw notify events into a ChangeSet.
/// Groups events by file path, deduplicates, and merges sequential modifications.
pub fn normalize_events(events: &[Event], workspace_root: &Path) -> ChangeSet {
    use std::collections::HashMap;

    // Map from file path to the most significant change kind for that path
    // Priority: Renamed > Modified > Deleted > Added (we want the most impactful)
    let mut change_map: HashMap<String, (ChangeKind, Option<String>)> = HashMap::new();

    for event in events {
        if event.paths.is_empty() {
            continue;
        }

        let path = &event.paths[0];
        let rel = relative_path(path, workspace_root);

        match &event.kind {
            EventKind::Create(_) => {
                change_map.entry(rel).or_insert((ChangeKind::Added, None));
            }
            EventKind::Modify(modify_kind) => {
                match modify_kind {
                    ModifyKind::Data(_) | ModifyKind::Any => {
                        // If already Renamed or Deleted, keep that; otherwise mark modified
                        let entry = change_map
                            .entry(rel)
                            .or_insert((ChangeKind::Modified, None));
                        if entry.0 != ChangeKind::Renamed && entry.0 != ChangeKind::Deleted {
                            entry.0 = ChangeKind::Modified;
                        }
                    }
                    ModifyKind::Name(rename_mode) => {
                        match rename_mode {
                            RenameMode::From => {
                                // Source of a rename
                                change_map
                                    .entry(rel.clone())
                                    .or_insert((ChangeKind::Renamed, None));
                            }
                            RenameMode::To => {
                                // Destination of a rename — mark as Added
                                change_map
                                    .entry(rel.clone())
                                    .or_insert((ChangeKind::Added, None));
                            }
                            RenameMode::Both => {
                                // A complete rename event with both old and new paths
                                if event.paths.len() >= 2 {
                                    let old_rel = relative_path(&event.paths[1], workspace_root);
                                    change_map.insert(
                                        rel.clone(),
                                        (ChangeKind::Renamed, Some(old_rel.clone())),
                                    );
                                } else {
                                    change_map.entry(rel).or_insert((ChangeKind::Renamed, None));
                                }
                            }
                            _ => {
                                change_map
                                    .entry(rel)
                                    .or_insert((ChangeKind::Modified, None));
                            }
                        }
                    }
                    _ => {
                        change_map
                            .entry(rel)
                            .or_insert((ChangeKind::Modified, None));
                    }
                }
            }
            EventKind::Remove(_) => {
                // If already Added, just remove the entry (file was created then deleted)
                if let Some(entry) = change_map.get(&rel) {
                    if entry.0 == ChangeKind::Added {
                        change_map.remove(&rel);
                        continue;
                    }
                }
                change_map.entry(rel).or_insert((ChangeKind::Deleted, None));
            }
            _ => {}
        }
    }

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    let mut renamed = Vec::new();

    for (path, (kind, previous_path)) in change_map {
        let entry = ChangeEntry {
            path: path.clone(),
            kind: kind.clone(),
            previous_path,
        };
        match kind {
            ChangeKind::Added => added.push(entry),
            ChangeKind::Modified => modified.push(entry),
            ChangeKind::Deleted => deleted.push(entry),
            ChangeKind::Renamed => renamed.push(entry),
        }
    }

    // Sort each category for deterministic output
    added.sort_by(|a, b| a.path.cmp(&b.path));
    modified.sort_by(|a, b| a.path.cmp(&b.path));
    deleted.sort_by(|a, b| a.path.cmp(&b.path));
    renamed.sort_by(|a, b| a.path.cmp(&b.path));

    let total = added.len() + modified.len() + deleted.len() + renamed.len();

    ChangeSet {
        added,
        modified,
        deleted,
        renamed,
        total_changes: total,
    }
}

fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_skip_dot_git_paths() {
        assert!(should_skip_path(Path::new(".git/config")));
        assert!(should_skip_path(Path::new("src/.git/HEAD")));
        assert!(should_skip_path(Path::new(".codetrail/snapshots/x")));
        assert!(should_skip_path(Path::new("target/debug/main")));
        assert!(should_skip_path(Path::new("node_modules/pkg/index.js")));
        assert!(!should_skip_path(Path::new("src/main.rs")));
    }
}
