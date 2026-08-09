//! Daemon-owned AFS sessions: the storage side of `coven.daemon.v1`'s
//! `afs.*` operations.
//!
//! Layout under `<COVEN_HOME>` follows `specs/coven-agent-fs/DESIGN.md` §2:
//!
//! ```text
//! afs/bases/<fingerprint>.db     read-only base snapshots, content-addressed
//! afs/sessions/<id>.db           writable deltas, one per AFS session
//! ```
//!
//! Bases are shared by every session opened against the same fingerprint, so N
//! concurrent agents on one repository cost one base plus N small deltas.
//!
//! Each operation opens the databases it needs and drops them again. SQLite
//! opens are cheap and the daemon serves requests independently, so holding
//! overlay handles across requests would buy contention rather than speed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use coven_afs::{Actor, AgentFs, Change, OverlayFs, SessionBinding, STATE_DISCARDED, STATE_OPEN};

/// Bumped when the ingest filter changes, so bases built under the old rules
/// are not silently reused for sessions expecting the new ones.
const INGEST_FILTER_VERSION: u32 = 1;

/// Directories never ingested into a base. Build output and package trees are
/// the offenders RESEARCH.md called out: they are enormous, regenerable, and
/// they are exactly what makes full-file copy-up expensive.
const INGEST_EXCLUDES: &[&str] = &[".git", "target", "node_modules", ".worktrees"];

/// Largest single file copied into a base, and the largest first-write
/// copy-up a session will absorb.
///
/// Justified by the coven-110 measurements in `MOUNT-SPIKE.md` §2: copy-up is
/// linear at roughly 135–300 MiB/s (1 MiB → 3.3 ms, 8 MiB → 57 ms, 32 MiB →
/// 238 ms). 16 MiB keeps a worst-case first write near ~120 ms, which stays
/// inside a plausible interactive budget; 32 MiB would not.
pub const COPY_UP_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Failures that map onto DESIGN.md §3.4's dotted error codes.
///
/// Only the codes the implemented operations can actually raise live here.
/// `afs.copy_up_too_large`, `afs.path_outside_root`, `afs.base_diverged`, and
/// `afs.commit_conflict` arrive with commit materialization; defining them
/// before anything can return them would advertise a contract the daemon does
/// not yet honour.
#[derive(Debug)]
pub enum AfsError {
    SessionNotFound(String),
    SessionNotOpen { id: String, state: String },
    NameInUse(String),
    ConfirmationRequired,
    Internal(anyhow::Error),
}

impl AfsError {
    /// `(status, code, message)` for the standard error envelope.
    pub fn parts(&self) -> (u16, &'static str, String) {
        match self {
            Self::SessionNotFound(id) => (
                404,
                "afs.session_not_found",
                format!("No AFS session {id}."),
            ),
            Self::SessionNotOpen { id, state } => (
                409,
                "afs.session_not_open",
                format!("AFS session {id} is {state}; this operation requires an open session."),
            ),
            Self::NameInUse(name) => (
                409,
                "afs.name_in_use",
                format!("Another open AFS session already holds the name {name}."),
            ),
            Self::ConfirmationRequired => (
                400,
                "invalid_request",
                "Discard requires \"confirm\": true.".to_string(),
            ),
            Self::Internal(error) => (500, "afs.unavailable", error.to_string()),
        }
    }
}

impl From<anyhow::Error> for AfsError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl From<coven_afs::Error> for AfsError {
    fn from(error: coven_afs::Error) -> Self {
        Self::Internal(anyhow::anyhow!(error))
    }
}

type AfsResult<T> = std::result::Result<T, AfsError>;

// ---- wire shapes --------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    pub project_root: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub familiar_id: Option<String>,
    #[serde(default)]
    pub bead_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseView {
    pub fingerprint: String,
    pub commit: Option<String>,
    pub files: i64,
    pub skipped: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingView {
    pub session_id: Option<String>,
    pub familiar_id: Option<String>,
    pub bead_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeCounts {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: String,
    pub name: Option<String>,
    pub state: String,
    pub base: BaseView,
    pub binding: BindingView,
    pub mount: Option<String>,
    pub changes: ChangeCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeView {
    pub path: String,
    pub change: String,
    pub bytes: i64,
    pub ino: Option<i64>,
    pub base_ino: Option<i64>,
    pub mode: Option<u32>,
    /// `"recorded"` when provenance accounts for this path, `"unknown"` when
    /// it does not. Never hidden: DESIGN.md §4.4 requires a diff entry nobody
    /// can explain to be visible as such.
    pub attribution: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffView {
    pub changes: Vec<ChangeView>,
    pub counts: ChangeCounts,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    pub seq: i64,
    pub op: String,
    pub path: String,
    pub to_path: Option<String>,
    pub bytes: i64,
    pub at: i64,
    pub session_id: Option<String>,
    pub familiar_id: Option<String>,
    pub bead_id: Option<String>,
    pub turn: Option<i64>,
    pub tool_call_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineView {
    pub entries: Vec<TimelineEntry>,
    pub next_cursor: Option<i64>,
    pub has_more: bool,
}

// ---- store --------------------------------------------------------------

/// The daemon's AFS session store, rooted at `<COVEN_HOME>/afs`.
pub struct AfsStore {
    root: PathBuf,
}

impl AfsStore {
    pub fn new(coven_home: &Path) -> Self {
        Self {
            root: coven_home.join("afs"),
        }
    }

    fn bases_dir(&self) -> PathBuf {
        self.root.join("bases")
    }

    fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn delta_path(&self, id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{id}.db"))
    }

    fn base_path(&self, fingerprint: &str) -> PathBuf {
        self.bases_dir().join(format!("{fingerprint}.db"))
    }

    /// Open a session's delta with write-ahead logging enabled.
    ///
    /// WAL is not the crate default because it costs the single-file property
    /// a portable delta depends on. A daemon-owned session delta is scratch
    /// state whose durability story is "discard or commit", and the coven-110
    /// benchmarks put whole-file writes at 8.84x the host filesystem without
    /// it versus 1.29x with it, so the daemon always turns it on.
    fn open_delta(&self, id: &str) -> AfsResult<AgentFs> {
        let path = self.delta_path(id);
        if !path.exists() {
            return Err(AfsError::SessionNotFound(id.to_string()));
        }
        let fs = AgentFs::create(&path).map_err(AfsError::from)?;
        fs.enable_wal().map_err(AfsError::from)?;
        Ok(fs)
    }

    fn open_overlay(&self, id: &str, binding: &SessionBinding) -> AfsResult<OverlayFs> {
        let fingerprint = binding.base_fingerprint.clone().unwrap_or_default();
        let overlay = OverlayFs::open(self.delta_path(id), self.base_path(&fingerprint))
            .map_err(AfsError::from)?;
        overlay.delta().enable_wal().map_err(AfsError::from)?;
        Ok(overlay)
    }

    fn binding(&self, id: &str) -> AfsResult<SessionBinding> {
        let delta = self.open_delta(id)?;
        delta
            .session_binding()
            .map_err(AfsError::from)?
            .ok_or_else(|| AfsError::SessionNotFound(id.to_string()))
    }

    fn require_open(&self, binding: &SessionBinding) -> AfsResult<()> {
        if binding.state == STATE_OPEN {
            Ok(())
        } else {
            Err(AfsError::SessionNotOpen {
                id: binding.id.clone(),
                state: binding.state.clone(),
            })
        }
    }

    /// `afs.session.create`.
    pub fn create(&self, request: &CreateRequest) -> AfsResult<SessionView> {
        let project_root = std::fs::canonicalize(&request.project_root)
            .with_context(|| format!("project root {} is unreadable", request.project_root))
            .map_err(AfsError::from)?;

        if let Some(name) = &request.name {
            for existing in self.list()? {
                if existing.name.as_deref() == Some(name.as_str()) && existing.state == STATE_OPEN {
                    return Err(AfsError::NameInUse(name.clone()));
                }
            }
        }

        let commit = git_head(&project_root);
        let fingerprint = base_fingerprint(&project_root, commit.as_deref());
        std::fs::create_dir_all(self.bases_dir())
            .and_then(|_| std::fs::create_dir_all(self.sessions_dir()))
            .context("failed to create the AFS store layout")
            .map_err(AfsError::from)?;

        let base_path = self.base_path(&fingerprint);
        let (files, skipped) = if base_path.exists() {
            base_stats(&base_path).unwrap_or((0, 0))
        } else {
            ingest_base(&project_root, &base_path)?
        };

        let id = format!("afs-{}", uuid::Uuid::new_v4());
        let delta = AgentFs::create(self.delta_path(&id)).map_err(AfsError::from)?;
        delta.enable_wal().map_err(AfsError::from)?;
        let binding = SessionBinding {
            id: id.clone(),
            name: request.name.clone(),
            state: STATE_OPEN.to_string(),
            base_fingerprint: Some(fingerprint.clone()),
            base_commit: commit.clone(),
            project_root: Some(project_root.to_string_lossy().into_owned()),
            coven_session_id: request.session_id.clone(),
            familiar_id: request.familiar_id.clone(),
            bead_id: request.bead_id.clone(),
            ..Default::default()
        };
        delta.bind_session(&binding).map_err(AfsError::from)?;
        drop(delta);

        Ok(SessionView {
            id,
            name: binding.name.clone(),
            state: STATE_OPEN.to_string(),
            base: BaseView {
                fingerprint,
                commit,
                files,
                skipped,
            },
            binding: BindingView {
                session_id: binding.coven_session_id.clone(),
                familiar_id: binding.familiar_id.clone(),
                bead_id: binding.bead_id.clone(),
            },
            mount: None,
            changes: ChangeCounts {
                added: 0,
                modified: 0,
                deleted: 0,
                bytes: 0,
            },
        })
    }

    /// `afs.session.list`.
    pub fn list(&self) -> AfsResult<Vec<SessionView>> {
        let dir = self.sessions_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&dir)
            .with_context(|| format!("failed to list {}", dir.display()))
            .map_err(AfsError::from)?;
        for entry in entries {
            let entry = entry.context("failed to read an AFS session entry")?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("db") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Ok(view) = self.get(id) {
                out.push(view);
            }
        }
        out.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(out)
    }

    /// `afs.session.get`.
    pub fn get(&self, id: &str) -> AfsResult<SessionView> {
        let binding = self.binding(id)?;
        let counts = match self.open_overlay(id, &binding) {
            Ok(overlay) => {
                let set = overlay.change_set().map_err(AfsError::from)?;
                ChangeCounts {
                    added: set.added,
                    modified: set.modified,
                    deleted: set.deleted,
                    bytes: set.bytes,
                }
            }
            // A discarded or base-less session still lists; it just has no
            // change set to report.
            Err(_) => ChangeCounts {
                added: 0,
                modified: 0,
                deleted: 0,
                bytes: 0,
            },
        };
        Ok(SessionView {
            id: binding.id.clone(),
            name: binding.name.clone(),
            state: binding.state.clone(),
            base: BaseView {
                fingerprint: binding.base_fingerprint.clone().unwrap_or_default(),
                commit: binding.base_commit.clone(),
                files: 0,
                skipped: 0,
            },
            binding: BindingView {
                session_id: binding.coven_session_id.clone(),
                familiar_id: binding.familiar_id.clone(),
                bead_id: binding.bead_id.clone(),
            },
            mount: None,
            changes: counts,
        })
    }

    /// `afs.session.join` — attach a second actor to an existing delta.
    ///
    /// Recorded as an operation rather than folded into the binding, because
    /// the acting identity is a property of each operation once more than one
    /// actor can write (DESIGN.md §4.2).
    pub fn join(&self, id: &str, actor: &Actor) -> AfsResult<SessionView> {
        let binding = self.binding(id)?;
        self.require_open(&binding)?;
        let delta = self.open_delta(id)?;
        delta
            .record_operation("join", "/", None, None, None, 0, actor)
            .map_err(AfsError::from)?;
        drop(delta);
        self.get(id)
    }

    /// `afs.session.diff`.
    pub fn diff(&self, id: &str) -> AfsResult<DiffView> {
        let binding = self.binding(id)?;
        let overlay = self.open_overlay(id, &binding)?;
        let set = overlay.change_set().map_err(AfsError::from)?;
        let attributed: HashSet<String> =
            overlay.delta().attributed_paths().map_err(AfsError::from)?;
        let counts = ChangeCounts {
            added: set.added,
            modified: set.modified,
            deleted: set.deleted,
            bytes: set.bytes,
        };
        let changes = set
            .entries
            .into_iter()
            .map(|entry| ChangeView {
                attribution: if attributed.contains(&entry.path) {
                    "recorded".to_string()
                } else {
                    "unknown".to_string()
                },
                path: entry.path,
                change: match entry.change {
                    Change::Added => "added",
                    Change::Modified => "modified",
                    Change::Deleted => "deleted",
                }
                .to_string(),
                bytes: entry.bytes,
                ino: entry.ino,
                base_ino: entry.base_ino,
                mode: entry.mode,
            })
            .collect();
        Ok(DiffView {
            changes,
            counts,
            truncated: false,
        })
    }

    /// `afs.timeline` — cursor-paginated on `afs_provenance.seq`, matching the
    /// daemon's existing `eventCursor: "sequence"` idiom.
    pub fn timeline(&self, id: &str, since: i64, limit: usize) -> AfsResult<TimelineView> {
        let delta = self.open_delta(id)?;
        let records = delta
            .provenance_since(since, limit + 1)
            .map_err(AfsError::from)?;
        let has_more = records.len() > limit;
        let entries: Vec<TimelineEntry> = records
            .into_iter()
            .take(limit)
            .map(|record| TimelineEntry {
                seq: record.seq,
                op: record.op,
                path: record.path,
                to_path: record.to_path,
                bytes: record.bytes,
                at: record.at,
                session_id: record.actor.coven_session_id,
                familiar_id: record.actor.familiar_id,
                bead_id: record.actor.bead_id,
                turn: record.actor.turn,
                tool_call_id: record.actor.tool_call_id,
            })
            .collect();
        Ok(TimelineView {
            next_cursor: entries.last().map(|entry| entry.seq),
            has_more,
            entries,
        })
    }

    /// `afs.session.discard` — unmount and delete the delta.
    ///
    /// A POST with an explicit `confirm` rather than a DELETE, because it is
    /// destructive and must be distinguishable in logs from idle cleanup.
    /// `retain_audit` keeps the database and marks it discarded instead.
    pub fn discard(&self, id: &str, confirm: bool, retain_audit: bool) -> AfsResult<()> {
        if !confirm {
            return Err(AfsError::ConfirmationRequired);
        }
        let binding = self.binding(id)?;
        if retain_audit {
            let delta = self.open_delta(id)?;
            delta
                .set_session_state(&binding.id, STATE_DISCARDED)
                .map_err(AfsError::from)?;
            return Ok(());
        }
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{suffix}", self.delta_path(id).display()));
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("failed to remove {}", path.display()))
                    .map_err(AfsError::from)?;
            }
        }
        Ok(())
    }
}

// ---- base ingest --------------------------------------------------------

fn git_head(project_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

/// Content identity of a base: the inputs that decide what it holds.
fn base_fingerprint(project_root: &Path, commit: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(project_root.to_string_lossy().as_bytes());
    digest.update([0]);
    digest.update(commit.unwrap_or("no-commit").as_bytes());
    digest.update([0]);
    digest.update(INGEST_FILTER_VERSION.to_be_bytes());
    let digest = digest.finalize();
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn base_stats(base_path: &Path) -> Result<(i64, i64)> {
    let fs = AgentFs::open_read_only(base_path).map_err(|e| anyhow::anyhow!(e))?;
    let count = fs
        .file_count()
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to count base files")?;
    Ok((count, 0))
}

/// Copy a project root into a fresh base database. Returns `(files, skipped)`.
fn ingest_base(project_root: &Path, base_path: &Path) -> AfsResult<(i64, i64)> {
    let staging = base_path.with_extension("partial");
    let _ = std::fs::remove_file(&staging);
    let mut fs = AgentFs::create(&staging).map_err(AfsError::from)?;
    fs.enable_wal().map_err(AfsError::from)?;
    let mut files = 0_i64;
    let mut skipped = 0_i64;
    ingest_dir(&mut fs, project_root, "", &mut files, &mut skipped)?;
    // A base is copied and opened read-only by every session that shares it,
    // so publish it as the single file that promise depends on rather than a
    // database trailing WAL sidecars.
    fs.checkpoint_to_single_file().map_err(AfsError::from)?;
    drop(fs);
    // Publish atomically so a crashed ingest never leaves a half-built base
    // that a later session would happily reuse.
    std::fs::rename(&staging, base_path)
        .with_context(|| format!("failed to publish base {}", base_path.display()))
        .map_err(AfsError::from)?;
    Ok((files, skipped))
}

fn ingest_dir(
    fs: &mut AgentFs,
    dir: &Path,
    prefix: &str,
    files: &mut i64,
    skipped: &mut i64,
) -> AfsResult<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))
        .map_err(AfsError::from)?;
    for entry in entries {
        let entry = entry.context("failed to read a directory entry")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if INGEST_EXCLUDES.contains(&name.as_str()) {
            *skipped += 1;
            continue;
        }
        let kind = entry.file_type().context("failed to stat an entry")?;
        let child = format!("{prefix}/{name}");
        if kind.is_dir() {
            ingest_dir(fs, &entry.path(), &child, files, skipped)?;
        } else if kind.is_file() {
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            if size > COPY_UP_MAX_BYTES {
                // Recorded, not silently dropped: a base that quietly omits
                // files would make a diff look complete when it is not.
                *skipped += 1;
                continue;
            }
            let data = std::fs::read(entry.path())
                .with_context(|| format!("failed to read {}", entry.path().display()))
                .map_err(AfsError::from)?;
            fs.write_file(&child, &data).map_err(AfsError::from)?;
            *files += 1;
        }
        // Symlinks are skipped: a base is a content snapshot, and a link
        // pointing outside the root would escape it.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(dir: &Path) -> PathBuf {
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("README.md"), b"# project").unwrap();
        std::fs::write(root.join("target/huge.bin"), b"build output").unwrap();
        root
    }

    fn store(dir: &Path) -> AfsStore {
        AfsStore::new(dir)
    }

    fn create(store: &AfsStore, root: &Path) -> SessionView {
        store
            .create(&CreateRequest {
                project_root: root.to_string_lossy().into_owned(),
                bead_id: Some("coven-5kt".into()),
                ..Default::default()
            })
            .unwrap()
    }

    #[test]
    fn create_ingests_the_project_and_excludes_build_output() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);

        assert_eq!(view.state, STATE_OPEN);
        assert_eq!(view.base.files, 2, "src/main.rs and README.md only");
        assert!(view.base.skipped >= 1, "target/ must be excluded");
        assert_eq!(view.changes.added, 0);
        assert_eq!(view.binding.bead_id.as_deref(), Some("coven-5kt"));
    }

    #[test]
    fn two_sessions_on_one_root_share_a_base() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let first = create(&store, &root);
        let second = create(&store, &root);

        assert_ne!(first.id, second.id);
        assert_eq!(first.base.fingerprint, second.base.fingerprint);
        let bases = std::fs::read_dir(dir.path().join("afs/bases"))
            .unwrap()
            .count();
        assert_eq!(bases, 1, "one base for one project root");
    }

    #[test]
    fn diff_reports_changes_and_marks_unattributed_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);

        // Write through the overlay without recording provenance, which is
        // what an unbound writer does.
        {
            let binding = store.binding(&view.id).unwrap();
            let mut overlay = store.open_overlay(&view.id, &binding).unwrap();
            overlay
                .write_file("/src/main.rs", b"fn main() { changed }")
                .unwrap();
            overlay.write_file("/new.txt", b"added").unwrap();
        }

        let diff = store.diff(&view.id).unwrap();
        assert_eq!(diff.counts.modified, 1);
        assert_eq!(diff.counts.added, 1);
        assert!(
            diff.changes.iter().all(|c| c.attribution == "unknown"),
            "writes with no provenance must be visible AND marked unknown"
        );
    }

    #[test]
    fn recorded_operations_appear_on_the_timeline_and_mark_attribution() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);
        {
            let binding = store.binding(&view.id).unwrap();
            let mut overlay = store.open_overlay(&view.id, &binding).unwrap();
            overlay.write_file("/src/main.rs", b"edited").unwrap();
            overlay
                .delta()
                .record_operation(
                    "write",
                    "/src/main.rs",
                    None,
                    None,
                    None,
                    6,
                    &Actor {
                        coven_session_id: Some("sess-1".into()),
                        turn: Some(3),
                        ..Default::default()
                    },
                )
                .unwrap();
        }

        let timeline = store.timeline(&view.id, 0, 10).unwrap();
        assert_eq!(timeline.entries.len(), 1);
        assert_eq!(timeline.entries[0].path, "/src/main.rs");
        assert_eq!(timeline.entries[0].turn, Some(3));
        assert!(!timeline.has_more);

        let diff = store.diff(&view.id).unwrap();
        let entry = diff
            .changes
            .iter()
            .find(|c| c.path == "/src/main.rs")
            .unwrap();
        assert_eq!(entry.attribution, "recorded");
    }

    #[test]
    fn timeline_paginates_on_seq() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);
        {
            let delta = store.open_delta(&view.id).unwrap();
            for index in 0..5 {
                delta
                    .record_operation(
                        "write",
                        &format!("/f{index}"),
                        None,
                        None,
                        None,
                        1,
                        &Actor::default(),
                    )
                    .unwrap();
            }
        }
        let first = store.timeline(&view.id, 0, 2).unwrap();
        assert_eq!(first.entries.len(), 2);
        assert!(first.has_more);
        let rest = store
            .timeline(&view.id, first.next_cursor.unwrap(), 10)
            .unwrap();
        assert_eq!(rest.entries.len(), 3);
        assert!(!rest.has_more);
    }

    #[test]
    fn join_records_the_second_actor_per_operation() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);
        store
            .join(
                &view.id,
                &Actor {
                    familiar_id: Some("echo".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let timeline = store.timeline(&view.id, 0, 10).unwrap();
        assert_eq!(timeline.entries[0].op, "join");
        assert_eq!(timeline.entries[0].familiar_id.as_deref(), Some("echo"));
    }

    #[test]
    fn discard_requires_confirmation_and_then_removes_the_delta() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);

        assert!(matches!(
            store.discard(&view.id, false, false),
            Err(AfsError::ConfirmationRequired)
        ));
        assert!(store.get(&view.id).is_ok());

        store.discard(&view.id, true, false).unwrap();
        assert!(matches!(
            store.get(&view.id),
            Err(AfsError::SessionNotFound(_))
        ));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn discard_with_retain_audit_keeps_the_delta_and_marks_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);
        store.discard(&view.id, true, true).unwrap();

        let kept = store.get(&view.id).unwrap();
        assert_eq!(kept.state, STATE_DISCARDED);
    }

    #[test]
    fn a_name_cannot_be_reused_while_a_session_holds_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let request = CreateRequest {
            project_root: root.to_string_lossy().into_owned(),
            name: Some("spike".into()),
            ..Default::default()
        };
        store.create(&request).unwrap();
        assert!(matches!(
            store.create(&request),
            Err(AfsError::NameInUse(name)) if name == "spike"
        ));
    }

    #[test]
    fn operations_on_an_unknown_session_report_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        for result in [
            store.get("afs-missing").err(),
            store.diff("afs-missing").err(),
            store.timeline("afs-missing", 0, 10).err(),
        ] {
            assert!(matches!(result, Some(AfsError::SessionNotFound(_))));
        }
        let (status, code, _) = AfsError::SessionNotFound("x".into()).parts();
        assert_eq!(status, 404);
        assert_eq!(code, "afs.session_not_found");
    }

    #[test]
    fn oversized_files_are_skipped_and_counted_rather_than_dropped_silently() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        std::fs::write(
            root.join("big.bin"),
            vec![0_u8; (COPY_UP_MAX_BYTES + 1) as usize],
        )
        .unwrap();
        let store = store(dir.path());
        let view = create(&store, &root);
        assert_eq!(view.base.files, 2, "the oversized file is not ingested");
        assert!(view.base.skipped >= 2, "and it is counted as skipped");
    }
}
