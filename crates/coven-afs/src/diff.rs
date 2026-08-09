//! Change set between an overlay's delta and its read-only base.
//!
//! Computed from `fs_dentry`, `fs_origin`, and `fs_whiteout` — never from
//! `afs_provenance`. DESIGN.md §4.4 is explicit about why: writes can arrive
//! without a bound actor (upstream `agentfs` tooling, a manual `sqlite3` edit,
//! a mount write from a process the daemon did not launch), and those produce
//! no provenance row. The filesystem tables cannot lie about what the
//! filesystem contains; the provenance log can be incomplete. A diff derived
//! from provenance would silently omit exactly the changes nobody can account
//! for, which are the ones worth seeing.

use crate::fs::Metadata;
use crate::{AgentFs, OverlayFs, Result};

/// How a path differs from the base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

impl Change {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// One changed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEntry {
    pub path: String,
    pub change: Change,
    pub bytes: i64,
    pub ino: Option<i64>,
    pub base_ino: Option<i64>,
    pub mode: Option<u32>,
}

/// The full change set, with counts callers would otherwise recompute.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeSet {
    pub entries: Vec<ChangeEntry>,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub bytes: i64,
}

impl ChangeSet {
    fn from_entries(mut entries: Vec<ChangeEntry>) -> Self {
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let mut set = Self {
            added: entries.iter().filter(|e| e.change == Change::Added).count(),
            modified: entries
                .iter()
                .filter(|e| e.change == Change::Modified)
                .count(),
            deleted: entries
                .iter()
                .filter(|e| e.change == Change::Deleted)
                .count(),
            bytes: entries.iter().map(|e| e.bytes).sum(),
            entries,
        };
        set.entries.dedup_by(|a, b| a.path == b.path);
        set
    }
}

/// Walk every path present in a filesystem, depth-first, skipping the root.
fn walk(fs: &AgentFs, ino: i64, prefix: &str, out: &mut Vec<(String, Metadata)>) -> Result<()> {
    for entry in fs.readdir_ino(ino, 0, i64::MAX as usize)? {
        let path = format!("{prefix}/{}", entry.name);
        let is_dir = entry.meta.is_dir();
        let child = entry.meta.ino;
        out.push((path.clone(), entry.meta));
        if is_dir {
            walk(fs, child, &path, out)?;
        }
    }
    Ok(())
}

impl OverlayFs {
    /// Compute the change set of this overlay against its base.
    ///
    /// A path in the delta is `Added` when the base does not have it and
    /// `Modified` when it does — which is exactly what `fs_origin` records
    /// after a copy-up, and what a same-path entry in the base means
    /// otherwise. Whiteouts are `Deleted`. Directories are reported only when
    /// they are new, because an unchanged directory holding a changed file is
    /// not itself a change.
    pub fn change_set(&self) -> Result<ChangeSet> {
        let mut entries = Vec::new();

        let mut delta_paths = Vec::new();
        walk(self.delta(), crate::ROOT_INO, "", &mut delta_paths)?;
        for (path, meta) in delta_paths {
            let base_ino = self.origin_ino(meta.ino)?;
            let in_base = self.base().resolve(&path)?;
            if meta.is_dir() && in_base.is_some() {
                continue;
            }
            let change = if base_ino.is_some() || in_base.is_some() {
                Change::Modified
            } else {
                Change::Added
            };
            entries.push(ChangeEntry {
                path,
                change,
                bytes: meta.size,
                ino: Some(meta.ino),
                base_ino: base_ino.or(in_base),
                mode: Some(meta.mode),
            });
        }

        let mut stmt = self
            .delta()
            .conn
            .prepare("SELECT path FROM fs_whiteout ORDER BY path ASC")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let path = row?;
            let base_ino = self.base().resolve(&path)?;
            entries.push(ChangeEntry {
                path,
                change: Change::Deleted,
                bytes: 0,
                ino: None,
                base_ino,
                mode: None,
            });
        }

        Ok(ChangeSet::from_entries(entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(dir: &std::path::Path) -> OverlayFs {
        let base_path = dir.join("base.db");
        {
            let mut base = AgentFs::create(&base_path).unwrap();
            base.write_file("/keep.txt", b"unchanged").unwrap();
            base.write_file("/edit.txt", b"original").unwrap();
            base.write_file("/drop.txt", b"doomed").unwrap();
            base.write_file("/dir/nested.txt", b"nested").unwrap();
        }
        OverlayFs::open(dir.join("delta.db"), &base_path).unwrap()
    }

    #[test]
    fn change_set_reports_added_modified_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut overlay = overlay(dir.path());
        overlay.write_file("/new.txt", b"brand new").unwrap();
        overlay.write_file("/edit.txt", b"changed").unwrap();
        overlay.remove_file("/drop.txt").unwrap();

        let set = overlay.change_set().unwrap();
        let by_path = |path: &str| {
            set.entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap_or_else(|| panic!("{path} missing from change set"))
                .change
        };
        assert_eq!(by_path("/new.txt"), Change::Added);
        assert_eq!(by_path("/edit.txt"), Change::Modified);
        assert_eq!(by_path("/drop.txt"), Change::Deleted);
        assert!(
            !set.entries.iter().any(|entry| entry.path == "/keep.txt"),
            "untouched base files must not appear"
        );
        assert_eq!(set.added, 1);
        assert_eq!(set.modified, 1);
        assert_eq!(set.deleted, 1);
    }

    #[test]
    fn a_write_with_no_provenance_still_appears() {
        // The case DESIGN.md 4.4 exists for: a mutation nobody recorded must
        // not be invisible just because the provenance log missed it.
        let dir = tempfile::tempdir().unwrap();
        let mut overlay = overlay(dir.path());
        overlay
            .write_file("/unattributed.txt", b"who wrote this")
            .unwrap();
        assert!(overlay.delta().provenance_since(0, 10).unwrap().is_empty());

        let set = overlay.change_set().unwrap();
        assert!(set
            .entries
            .iter()
            .any(|entry| entry.path == "/unattributed.txt" && entry.change == Change::Added));
    }

    #[test]
    fn unchanged_overlay_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let overlay = overlay(dir.path());
        let set = overlay.change_set().unwrap();
        assert!(set.entries.is_empty());
        assert_eq!(set.bytes, 0);
    }

    #[test]
    fn new_directories_are_reported_but_traversed_ones_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut overlay = overlay(dir.path());
        overlay
            .write_file("/dir/added.txt", b"in an existing dir")
            .unwrap();
        overlay
            .write_file("/fresh/deep/file.txt", b"in a new dir")
            .unwrap();

        let set = overlay.change_set().unwrap();
        let paths: Vec<&str> = set.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"/dir/added.txt"));
        assert!(paths.contains(&"/fresh/deep/file.txt"));
        assert!(
            !paths.contains(&"/dir"),
            "a directory that already exists in the base is not itself a change"
        );
        assert!(paths.contains(&"/fresh"), "a new directory is a change");
    }
}
