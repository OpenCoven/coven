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
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use similar::TextDiff;

use coven_afs::{
    normalize, Actor, AgentFs, Change, ChangeSet, OverlayFs, SessionBinding, STATE_COMMITTED,
    STATE_COMMITTING, STATE_DISCARDED, STATE_OPEN,
};

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
pub const UNIFIED_DIFF_MAX_BYTES: usize = 256 * 1024;

/// Failures that map onto DESIGN.md §3.4's dotted error codes.
///
/// Only the codes the implemented operations can actually raise live here, so
/// the enum never advertises a contract the daemon does not honour. The commit
/// codes arrived with materialization (`coven-fty`).
#[derive(Debug)]
pub enum AfsError {
    SessionNotFound(String),
    PathNotFound(String),
    PathNotFile(String),
    SessionNotOpen {
        id: String,
        state: String,
    },
    NameInUse(String),
    ConfirmationRequired,
    /// The project root moved off `base_commit` while the delta was open.
    BaseDiverged {
        expected: String,
        found: String,
    },
    /// A delta path would materialize outside the repository root, under
    /// `.git/`, or through a symlink whose target escapes.
    PathOutsideRoot {
        path: String,
        reason: String,
    },
    /// A single file exceeds the configured copy-up cap.
    CopyUpTooLarge {
        path: String,
        bytes: i64,
    },
    /// The target branch or worktree path is already taken.
    CommitConflict(String),
    /// Signing is required and unavailable; Coven never falls back to an
    /// unsigned commit to make materialization land.
    CommitUnsigned(String),
    /// No mount backend on this platform or in this build.
    MountUnsupported,
    /// Already mounted, or the mount point is not empty.
    MountBusy(String),
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
            Self::PathNotFound(path) => (404, "afs.path_not_found", format!("No AFS path {path}.")),
            Self::PathNotFile(path) => (
                400,
                "afs.path_not_file",
                format!("AFS path {path} is not a regular file."),
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
            Self::BaseDiverged { expected, found } => (
                409,
                "afs.base_diverged",
                format!(
                    "The project root is at {found}, not the delta's base {expected}. \
                     The delta is preserved; rebase the base or commit onto a fresh worktree."
                ),
            ),
            Self::PathOutsideRoot { path, reason } => (
                400,
                "afs.path_outside_root",
                format!("Refusing to materialize {path}: {reason}."),
            ),
            Self::CopyUpTooLarge { path, bytes } => (
                413,
                "afs.copy_up_too_large",
                format!("{path} is {bytes} bytes, over the {COPY_UP_MAX_BYTES}-byte copy-up cap."),
            ),
            Self::CommitConflict(message) => (409, "afs.commit_conflict", message.clone()),
            Self::CommitUnsigned(message) => (
                500,
                "afs.commit_unsigned",
                format!("Commit signing is required but unavailable: {message}"),
            ),
            Self::MountUnsupported => (
                501,
                "afs.mount_unsupported",
                "No mount backend is available; health advertises afsMount:false.".to_string(),
            ),
            Self::MountBusy(message) => (409, "afs.mount_busy", message.clone()),
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

pub(crate) type AfsResult<T> = std::result::Result<T, AfsError>;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    /// Defaults to `afs/<session-name-or-id>` (DESIGN.md §5 step 3).
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// Extra `Co-authored-by:` trailers, expected in the numeric-id no-reply
    /// form `AGENTS.md` requires.
    #[serde(default)]
    pub co_authors: Vec<String>,
    /// Run every refusal check and report what would happen, without
    /// quiescing the session, creating a worktree, or recording a commit.
    #[serde(default)]
    pub dry_run: bool,
}

/// What a `dryRun` commit reports (bead `coven-fty` follow-up `coven-y7a`).
///
/// `wouldCommit` is only ever `true`: a preview that would be refused returns
/// the refusal itself, as the same dotted error a real commit would raise, so
/// a client reads one contract rather than two.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitPreview {
    pub id: String,
    pub branch: String,
    pub worktree_path: String,
    pub provenance_high_water: i64,
    pub counts: ChangeCounts,
    /// Entries that would be written or removed, after directories and
    /// unmaterializable nodes are dropped — not the same as `counts`.
    pub files: usize,
    pub dry_run: bool,
    pub would_commit: bool,
}

/// Everything a commit needs, resolved and validated, before anything is
/// written. Shared by `commit` and `commit_dry_run` so the two cannot diverge.
struct CommitPlan {
    binding: SessionBinding,
    project_root: PathBuf,
    branch: String,
    worktree_path: PathBuf,
    plan: Vec<Planned>,
    counts: ChangeCounts,
    high_water: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitView {
    pub id: String,
    pub branch: String,
    pub commit: String,
    pub worktree_path: String,
    pub provenance_high_water: i64,
    pub state: String,
    pub counts: ChangeCounts,
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
pub struct FileDiffView {
    pub path: String,
    pub patch: String,
    pub truncated: bool,
    pub binary: bool,
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
        let project_root = strip_verbatim_prefix(
            std::fs::canonicalize(&request.project_root)
                .with_context(|| format!("project root {} is unreadable", request.project_root))
                .map_err(AfsError::from)?,
        );

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
            mount: crate::afs_mount::current(self.coven_home(), id),
            changes: counts,
        })
    }

    /// `afs.mount` — mount the session's filesystem.
    ///
    /// Mounting requires an open session for the same reason writing does: a
    /// committed or discarded delta is a record, and handing back a writable
    /// mount over one would let an agent edit history.
    pub fn mount(&self, id: &str) -> AfsResult<crate::afs_mount::MountView> {
        let binding = self.binding(id)?;
        self.require_open(&binding)?;
        let delta = self.delta_path(id);
        let read_only = self.open_delta(id)?.is_read_only();
        crate::afs_mount::mount(self.coven_home(), id, &delta, read_only)
    }

    /// `afs.mount` DELETE — unmount. Reports whether anything was mounted.
    ///
    /// Unlike mount, this does not require an open session: a session that was
    /// committed while mounted still needs its mount taken down, and refusing
    /// would strand it.
    pub fn unmount(&self, id: &str) -> AfsResult<bool> {
        // Still resolve the binding, so unmounting an unknown session is
        // `afs.session_not_found` rather than a cheerful no-op.
        self.binding(id)?;
        crate::afs_mount::unmount(self.coven_home(), id)
    }

    /// `<COVEN_HOME>`, recovered from the `afs` root this store was built on.
    fn coven_home(&self) -> &Path {
        self.root.parent().unwrap_or(&self.root)
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

    /// `afs.session.diff` for a single file path.
    pub fn file_diff(&self, id: &str, path: &str) -> AfsResult<FileDiffView> {
        let binding = self.binding(id)?;
        let overlay = self.open_overlay(id, &binding)?;
        let path = normalize(path);

        let base_meta = optional_agent_metadata(overlay.base(), &path)?;
        let merged_meta = optional_overlay_metadata(&overlay, &path)?;
        if base_meta.is_none() && merged_meta.is_none() {
            return Err(AfsError::PathNotFound(path));
        }
        if base_meta.as_ref().is_some_and(|meta| !meta.is_file())
            || merged_meta.as_ref().is_some_and(|meta| !meta.is_file())
        {
            return Err(AfsError::PathNotFile(path));
        }

        let base = if base_meta.is_some() {
            Some(overlay.base().read_file(&path).map_err(AfsError::from)?)
        } else {
            None
        };
        let merged = if merged_meta.is_some() {
            Some(overlay.read_file(&path).map_err(AfsError::from)?)
        } else {
            None
        };
        let path_header = diff_header_path(&path);
        let base_header = if base.is_some() {
            path_header.as_str()
        } else {
            "/dev/null"
        };
        let merged_header = if merged.is_some() {
            path_header.as_str()
        } else {
            "/dev/null"
        };
        let (patch, truncated, binary) = match (
            std::str::from_utf8(base.as_deref().unwrap_or(&[])),
            std::str::from_utf8(merged.as_deref().unwrap_or(&[])),
        ) {
            (Ok(""), Ok("")) if base.is_none() != merged.is_none() => {
                let (patch, truncated) = bounded_diff_headers(base_header, merged_header, &path)?;
                (patch, truncated, false)
            }
            (Ok(base), Ok(merged)) => {
                let (patch, truncated) =
                    bounded_unified_diff(base, merged, base_header, merged_header, &path)?;
                (patch, truncated, false)
            }
            _ if matches!((&base, &merged), (Some(base), Some(merged)) if base == merged) => {
                (String::new(), false, true)
            }
            _ => ("Binary files differ\n".to_string(), false, true),
        };

        Ok(FileDiffView {
            path,
            patch,
            truncated,
            binary,
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

    /// `afs.session.commit` with `dryRun` — every refusal a real commit could
    /// raise, and none of its effects.
    ///
    /// This shares [`plan_commit`](Self::plan_commit) with the real thing
    /// rather than re-deriving the checks, so a preview cannot drift from what
    /// commit actually enforces — the drift is the only way a preview could
    /// lie. Nothing is written: no `committing` transition, no worktree, no
    /// `afs_commit` row.
    pub fn commit_dry_run(&self, id: &str, request: &CommitRequest) -> AfsResult<CommitPreview> {
        let plan = self.plan_commit(id, request)?;
        Ok(CommitPreview {
            id: id.to_string(),
            branch: plan.branch,
            worktree_path: plan.worktree_path.to_string_lossy().into_owned(),
            provenance_high_water: plan.high_water,
            counts: plan.counts,
            files: plan.plan.len(),
            dry_run: true,
            would_commit: true,
        })
    }

    /// Resolve and validate everything a commit needs, mutating nothing.
    ///
    /// Every DESIGN.md §5 check that can refuse lives here: base verification
    /// (§5.2), the full change-set resolution and escape rejection (§5.4/§5.5),
    /// and the branch/worktree conflict checks. Reading the provenance high
    /// water is a `SELECT MAX`, so it is safe on this side of the line too.
    fn plan_commit(&self, id: &str, request: &CommitRequest) -> AfsResult<CommitPlan> {
        let binding = self.binding(id)?;
        self.require_open(&binding)?;

        // Sessions bound before the prefix fix still carry a verbatim root, so
        // strip here too rather than trusting what was stored.
        let project_root = binding
            .project_root
            .as_deref()
            .map(PathBuf::from)
            .map(strip_verbatim_prefix)
            .ok_or_else(|| {
                AfsError::Internal(anyhow::anyhow!(
                    "AFS session {id} has no bound project root"
                ))
            })?;

        // §5.2 — verify the base first. Divergence preserves the delta.
        let expected = binding.base_commit.clone().unwrap_or_default();
        let found = git_head(&project_root).unwrap_or_default();
        if expected.is_empty() || expected != found {
            return Err(AfsError::BaseDiverged {
                expected: if expected.is_empty() {
                    "<unrecorded>".to_string()
                } else {
                    expected
                },
                found: if found.is_empty() {
                    "<no HEAD>".to_string()
                } else {
                    found
                },
            });
        }

        // §5.4/§5.5 — resolve and validate the whole change set up front.
        let overlay = self.open_overlay(id, &binding)?;
        let set = overlay.change_set().map_err(AfsError::from)?;
        let counts = ChangeCounts {
            added: set.added,
            modified: set.modified,
            deleted: set.deleted,
            bytes: set.bytes,
        };
        let plan = plan_materialization(&overlay, &set)?;
        drop(overlay);

        let branch = match &request.branch {
            Some(branch) if !branch.trim().is_empty() => branch.trim().to_string(),
            _ => format!(
                "afs/{}",
                binding.name.clone().unwrap_or_else(|| id.to_string())
            ),
        };
        let worktrees_root = validate_worktrees_root(&project_root)?;
        let worktree_path = worktrees_root.join(worktree_slug(&branch));
        if git_branch_exists(&project_root, &branch) {
            return Err(AfsError::CommitConflict(format!(
                "Branch {branch} already exists; pass an explicit branch."
            )));
        }
        if worktree_path.exists() {
            return Err(AfsError::CommitConflict(format!(
                "Worktree path {} already exists.",
                worktree_path.display()
            )));
        }

        // A read, not a write — safe on the no-effects side of the line.
        let high_water = self
            .open_delta(id)?
            .provenance_high_water()
            .map_err(AfsError::from)?;

        Ok(CommitPlan {
            binding,
            project_root,
            branch,
            worktree_path,
            plan,
            counts,
            high_water,
        })
    }

    /// `afs.session.commit` — materialize the delta into a git branch
    /// (DESIGN.md §5).
    ///
    /// Every check that can refuse the commit runs *before* the worktree is
    /// created, so a refusal leaves no branch, no worktree, and no partially
    /// applied delta. The delta itself survives materialization — it is the
    /// audit record — until an explicit discard.
    pub fn commit(&self, id: &str, request: &CommitRequest) -> AfsResult<CommitView> {
        let CommitPlan {
            binding,
            project_root,
            branch,
            worktree_path,
            plan,
            counts,
            high_water,
        } = self.plan_commit(id, request)?;

        // §5.1 — quiesce only once nothing can still refuse.
        let delta = self.open_delta(id)?;
        delta
            .set_session_state(&binding.id, STATE_COMMITTING)
            .map_err(AfsError::from)?;
        drop(delta);

        let outcome = materialize(
            &project_root,
            &worktree_path,
            &branch,
            &plan,
            &binding,
            request,
        );

        let delta = self.open_delta(id)?;
        match outcome {
            Ok(sha) => {
                delta
                    .record_commit(
                        &branch,
                        Some(&sha),
                        Some(&worktree_path.to_string_lossy()),
                        STATE_COMMITTED,
                    )
                    .map_err(AfsError::from)?;
                delta
                    .set_session_state(&binding.id, STATE_COMMITTED)
                    .map_err(AfsError::from)?;
                Ok(CommitView {
                    id: id.to_string(),
                    branch,
                    commit: sha,
                    worktree_path: worktree_path.to_string_lossy().into_owned(),
                    provenance_high_water: high_water,
                    state: STATE_COMMITTED.to_string(),
                    counts,
                })
            }
            Err(error) => {
                // Materialization is all-or-nothing: tear the attempt down and
                // hand the session back in the state the caller found it.
                cleanup_failed_materialization(&project_root, &worktree_path, &branch);
                let _ = delta.record_commit(
                    &branch,
                    None,
                    Some(&worktree_path.to_string_lossy()),
                    "failed",
                );
                delta
                    .set_session_state(&binding.id, STATE_OPEN)
                    .map_err(AfsError::from)?;
                Err(error)
            }
        }
    }
}

// ---- commit materialization ---------------------------------------------

/// One validated change, resolved to bytes before any host mutation happens.
#[derive(Debug)]
enum Planned {
    File {
        path: String,
        data: Vec<u8>,
        executable: bool,
    },
    Symlink {
        path: String,
        target: String,
    },
    Removal {
        path: String,
    },
}

impl Planned {
    fn path(&self) -> &str {
        match self {
            Self::File { path, .. } | Self::Symlink { path, .. } | Self::Removal { path } => path,
        }
    }
}

/// Resolve the change set into a fully validated plan.
///
/// Reads content eagerly so a mid-apply read failure cannot leave a partially
/// written branch, and refuses every escape DESIGN.md §5.5 names before the
/// caller has created anything.
fn plan_materialization(overlay: &OverlayFs, set: &ChangeSet) -> AfsResult<Vec<Planned>> {
    let mut plan = Vec::with_capacity(set.entries.len());
    for entry in &set.entries {
        let path = coven_afs::normalize(&entry.path);
        reject_escape(&path)?;

        if matches!(entry.change, Change::Deleted) {
            plan.push(Planned::Removal { path });
            continue;
        }

        let meta = overlay.stat(&path).map_err(AfsError::from)?;
        if meta.is_symlink() {
            let target = overlay
                .delta()
                .read_link(&path)
                .map_err(AfsError::from)
                .or_else(|_| overlay.base().read_link(&path).map_err(AfsError::from))?;
            if symlink_escapes(&path, &target) {
                return Err(AfsError::PathOutsideRoot {
                    path,
                    reason: format!("its symlink target {target} resolves outside the root"),
                });
            }
            plan.push(Planned::Symlink { path, target });
            continue;
        }
        if meta.is_dir() {
            // Directories materialize implicitly from their files; git does
            // not track empty ones.
            continue;
        }

        if entry.bytes > COPY_UP_MAX_BYTES as i64 {
            return Err(AfsError::CopyUpTooLarge {
                path,
                bytes: entry.bytes,
            });
        }
        let data = overlay.read_file(&path).map_err(AfsError::from)?;
        if data.len() as u64 > COPY_UP_MAX_BYTES {
            return Err(AfsError::CopyUpTooLarge {
                path,
                bytes: data.len() as i64,
            });
        }
        plan.push(Planned::File {
            path,
            data,
            executable: entry.mode.is_some_and(|mode| mode & 0o111 != 0),
        });
    }
    Ok(plan)
}

/// Refuse anything that would leave the repository or touch git's own state.
fn reject_escape(normalized: &str) -> AfsResult<()> {
    if normalized == "/" {
        return Err(AfsError::PathOutsideRoot {
            path: normalized.to_string(),
            reason: "the root itself is not a materializable path".to_string(),
        });
    }
    let mut components = normalized.split('/').filter(|c| !c.is_empty());
    if components.clone().any(|c| c == ".." || c == ".") {
        return Err(AfsError::PathOutsideRoot {
            path: normalized.to_string(),
            reason: "it contains a relative component after normalization".to_string(),
        });
    }
    if components.next() == Some(".git") {
        return Err(AfsError::PathOutsideRoot {
            path: normalized.to_string(),
            reason: "writes under .git/ are never materialized".to_string(),
        });
    }
    Ok(())
}

/// Whether a symlink target leaves the root.
///
/// `path::normalize` clamps `..` at the root, so escape cannot be detected by
/// normalizing — this walks the target without clamping instead. An absolute
/// target escapes by construction: on the host it would point outside the
/// worktree entirely.
fn symlink_escapes(link_path: &str, target: &str) -> bool {
    if target.starts_with('/') {
        return true;
    }
    let mut depth = link_path.split('/').filter(|c| !c.is_empty()).count() as i64 - 1;
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => depth += 1,
        }
    }
    false
}

/// Drop Windows' extended-length path prefix.
///
/// `std::fs::canonicalize` returns verbatim paths (`\\?\C:\...`) on Windows.
/// git accepts one for `-C`, but cannot *create* directories under it:
/// `git worktree add` fails with "could not create leading directories ...
/// Invalid argument". Since the bound project root feeds every git invocation
/// and the worktree path is derived from it, the prefix is stripped at the
/// source instead of at each call site.
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    match strip_verbatim_str(&path.to_string_lossy()) {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

/// The string half of [`strip_verbatim_prefix`], split out so the rule is
/// testable on every platform rather than only where it fires.
fn strip_verbatim_str(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    path.strip_prefix(r"\\?\").map(str::to_string)
}

/// `afs/my-session` → `afs-my-session`, so the worktree directory is flat.
fn worktree_slug(branch: &str) -> String {
    branch
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn git_branch_exists(project_root: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Create the worktree, apply the plan, and produce a signed commit.
fn materialize(
    project_root: &Path,
    worktree_path: &Path,
    branch: &str,
    plan: &[Planned],
    binding: &SessionBinding,
    request: &CommitRequest,
) -> AfsResult<String> {
    let base = binding.base_commit.clone().unwrap_or_default();
    let add = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "add", "-b", branch])
        .arg(worktree_path)
        .arg(&base)
        .output()
        .context("failed to run git worktree add")
        .map_err(AfsError::from)?;
    if !add.status.success() {
        return Err(AfsError::Internal(anyhow::anyhow!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        )));
    }

    apply_plan(worktree_path, plan)?;

    let stage = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(["add", "--all"])
        .output()
        .context("failed to stage materialized changes")
        .map_err(AfsError::from)?;
    if !stage.status.success() {
        return Err(AfsError::Internal(anyhow::anyhow!(
            "git add failed: {}",
            String::from_utf8_lossy(&stage.stderr).trim()
        )));
    }

    // §5.6 — signed, always. A signing failure is surfaced, never worked
    // around by dropping -S.
    let commit = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args([
            "commit",
            "-S",
            "-s",
            "-m",
            &commit_message(binding, request),
        ])
        .output()
        .context("failed to run git commit")
        .map_err(AfsError::from)?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr).to_string();
        let combined = format!("{stderr}{}", String::from_utf8_lossy(&commit.stdout));
        if looks_like_signing_failure(&combined) {
            return Err(AfsError::CommitUnsigned(stderr.trim().to_string()));
        }
        return Err(AfsError::Internal(anyhow::anyhow!(
            "git commit failed: {}",
            stderr.trim()
        )));
    }

    let sha = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("failed to read the materialized commit")
        .map_err(AfsError::from)?;
    Ok(String::from_utf8_lossy(&sha.stdout).trim().to_string())
}

fn apply_plan(worktree_path: &Path, plan: &[Planned]) -> AfsResult<()> {
    for item in plan {
        // Paths are already normalized and escape-checked; strip the leading
        // slash so they join relative to the worktree.
        let item_path = item.path().to_string();
        let relative = item_path.trim_start_matches('/');
        ensure_real_parent_dirs(worktree_path, &item_path, relative)?;
        let host = worktree_path.join(relative);
        match item {
            Planned::Removal { .. } => {
                if std::fs::symlink_metadata(&host).is_ok() {
                    std::fs::remove_file(&host)
                        .with_context(|| format!("failed to remove {}", host.display()))
                        .map_err(AfsError::from)?;
                }
            }
            Planned::File {
                data, executable, ..
            } => {
                if let Ok(meta) = std::fs::symlink_metadata(&host) {
                    if meta.file_type().is_symlink() {
                        std::fs::remove_file(&host)
                            .with_context(|| format!("failed to remove {}", host.display()))
                            .map_err(AfsError::from)?;
                    } else if meta.is_dir() {
                        return Err(AfsError::CommitConflict(format!(
                            "Refusing to materialize {item_path}: an existing directory blocks the file."
                        )));
                    }
                }
                std::fs::write(&host, data)
                    .with_context(|| format!("failed to write {}", host.display()))
                    .map_err(AfsError::from)?;
                set_executable(&host, *executable)?;
            }
            Planned::Symlink { target, .. } => {
                if let Ok(meta) = std::fs::symlink_metadata(&host) {
                    if meta.file_type().is_symlink() || meta.is_file() {
                        std::fs::remove_file(&host)
                            .with_context(|| format!("failed to remove {}", host.display()))
                            .map_err(AfsError::from)?;
                    } else if meta.is_dir() {
                        return Err(AfsError::CommitConflict(format!(
                            "Refusing to materialize {item_path}: an existing directory blocks the symlink."
                        )));
                    }
                }
                create_symlink(target, &host)?;
            }
        }
    }
    Ok(())
}

fn validate_worktrees_root(project_root: &Path) -> AfsResult<PathBuf> {
    let root = project_root.join(".worktrees");
    match std::fs::symlink_metadata(&root) {
        Ok(meta) if meta.file_type().is_symlink() => Err(AfsError::CommitConflict(format!(
            "Refusing to materialize under {}: .worktrees must be a real directory, not a symlink.",
            project_root.display()
        ))),
        Ok(meta) if !meta.is_dir() => Err(AfsError::CommitConflict(format!(
            "Refusing to materialize under {}: .worktrees exists but is not a directory.",
            project_root.display()
        ))),
        Ok(_) => Ok(root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(root),
        Err(error) => Err(AfsError::Internal(anyhow::anyhow!(
            "failed to inspect {}: {error}",
            root.display()
        ))),
    }
}

fn ensure_real_parent_dirs(worktree_path: &Path, item_path: &str, relative: &str) -> AfsResult<()> {
    let mut parent = worktree_path.to_path_buf();
    let mut parts = relative
        .split('/')
        .filter(|part| !part.is_empty())
        .peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        parent.push(part);
        match std::fs::symlink_metadata(&parent) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AfsError::PathOutsideRoot {
                    path: item_path.to_string(),
                    reason: format!(
                        "its parent {} is a symlink that could escape the worktree",
                        parent.display()
                    ),
                });
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(AfsError::CommitConflict(format!(
                    "Refusing to materialize {item_path}: parent {} is not a directory.",
                    parent.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&parent)
                    .with_context(|| format!("failed to create {}", parent.display()))
                    .map_err(AfsError::from)?;
            }
            Err(error) => {
                return Err(AfsError::Internal(anyhow::anyhow!(
                    "failed to inspect {}: {error}",
                    parent.display()
                )));
            }
        }
    }
    Ok(())
}

/// Only the executable bit crosses over; AFS mode bits are not a git concept.
#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> AfsResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))
        .map_err(AfsError::from)?
        .permissions();
    let mode = perms.mode();
    perms.set_mode(if executable {
        mode | 0o111
    } else {
        mode & !0o111
    });
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("failed to set permissions on {}", path.display()))
        .map_err(AfsError::from)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> AfsResult<()> {
    // Windows has no executable bit; git records mode 100644 there anyway.
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &str, host: &Path) -> AfsResult<()> {
    std::os::unix::fs::symlink(target, host)
        .with_context(|| format!("failed to create symlink {}", host.display()))
        .map_err(AfsError::from)
}

#[cfg(not(unix))]
fn create_symlink(_target: &str, host: &Path) -> AfsResult<()> {
    Err(AfsError::Internal(anyhow::anyhow!(
        "cannot materialize the symlink {} on this platform",
        host.display()
    )))
}

/// git reports signing trouble on stderr with no distinguishing exit code.
fn looks_like_signing_failure(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "gpg failed to sign",
        "failed to write commit object",
        "secret key not available",
        "no secret key",
        "signing failed",
        "unable to sign",
        "user.signingkey",
        "no openpgp signing key",
        "cannot run gpg",
        "gpg: skipped",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// §5.7 — trailers carry the provenance a plain branch would lose.
fn commit_message(binding: &SessionBinding, request: &CommitRequest) -> String {
    let subject = match &request.message {
        Some(message) if !message.trim().is_empty() => message.trim().to_string(),
        _ => format!(
            "afs: materialize {}",
            binding.name.clone().unwrap_or_else(|| binding.id.clone())
        ),
    };
    let mut message = subject;
    message.push_str("\n\n");
    if let Some(session) = &binding.coven_session_id {
        message.push_str(&format!("Coven-Session: {session}\n"));
    }
    if let Some(familiar) = &binding.familiar_id {
        message.push_str(&format!("Coven-Familiar: {familiar}\n"));
    }
    if let Some(bead) = &binding.bead_id {
        message.push_str(&format!("Coven-Bead: {bead}\n"));
    }
    message.push_str(&format!("Coven-Afs-Session: {}\n", binding.id));
    for author in &request.co_authors {
        let author = author.trim();
        if !author.is_empty() {
            message.push_str(&format!("Co-authored-by: {author}\n"));
        }
    }
    message
}

/// Best-effort teardown. Any failure here is already secondary to the error
/// being reported, so it must not mask it.
fn cleanup_failed_materialization(project_root: &Path, worktree_path: &Path, branch: &str) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .output();
    if worktree_path.exists() {
        let _ = std::fs::remove_dir_all(worktree_path);
    }
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "prune"])
        .output();
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["branch", "-D", "--", branch])
        .output();
}

fn optional_agent_metadata(fs: &AgentFs, path: &str) -> AfsResult<Option<coven_afs::Metadata>> {
    match fs.stat(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(coven_afs::Error::NotFound(_)) => Ok(None),
        Err(error) => Err(AfsError::from(error)),
    }
}

fn optional_overlay_metadata(
    overlay: &OverlayFs,
    path: &str,
) -> AfsResult<Option<coven_afs::Metadata>> {
    match overlay.stat(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(coven_afs::Error::NotFound(_)) => Ok(None),
        Err(error) => Err(AfsError::from(error)),
    }
}

fn diff_header_path(path: &str) -> String {
    if !path
        .chars()
        .any(|character| character.is_control() || matches!(character, '"' | '\\'))
    {
        return path.to_string();
    }

    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\u{08}' => quoted.push_str("\\b"),
            '\u{0c}' => quoted.push_str("\\f"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            character if character.is_control() => {
                write!(&mut quoted, "\\u{:04x}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn bounded_unified_diff(
    base: &str,
    merged: &str,
    base_header: &str,
    merged_header: &str,
    path: &str,
) -> AfsResult<(String, bool)> {
    let diff = TextDiff::from_lines(base, merged);
    let mut unified = diff.unified_diff();
    unified.context_radius(3).header(base_header, merged_header);

    let mut output = CappedDiffWriter::new(UNIFIED_DIFF_MAX_BYTES);
    let result = unified.to_writer(&mut output);
    finish_bounded_diff(output, result, path)
}

fn bounded_diff_headers(
    base_header: &str,
    merged_header: &str,
    path: &str,
) -> AfsResult<(String, bool)> {
    let mut output = CappedDiffWriter::new(UNIFIED_DIFF_MAX_BYTES);
    let result = writeln!(&mut output, "--- {base_header}")
        .and_then(|()| writeln!(&mut output, "+++ {merged_header}"));
    finish_bounded_diff(output, result, path)
}

fn finish_bounded_diff(
    output: CappedDiffWriter,
    result: io::Result<()>,
    path: &str,
) -> AfsResult<(String, bool)> {
    if let Err(error) = result {
        if !output.truncated {
            return Err(AfsError::Internal(anyhow::anyhow!(
                "failed to render unified diff for {path}: {error}"
            )));
        }
    }
    let truncated = output.truncated;
    let patch = String::from_utf8(output.bytes).map_err(|error| {
        AfsError::Internal(anyhow::anyhow!(
            "unified diff for {path} was not valid UTF-8: {error}"
        ))
    })?;
    Ok((patch, truncated))
}

struct CappedDiffWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    truncated: bool,
}

impl CappedDiffWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(max_bytes),
            max_bytes,
            truncated: false,
        }
    }
}

impl Write for CappedDiffWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.bytes.len() == self.max_bytes {
            self.truncated = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "unified diff output limit reached",
            ));
        }

        let remaining = self.max_bytes - self.bytes.len();
        if buf.len() <= remaining {
            self.bytes.extend_from_slice(buf);
            return Ok(buf.len());
        }

        let text = std::str::from_utf8(buf)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut end = remaining;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            self.truncated = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "unified diff output limit reached",
            ));
        }

        self.bytes.extend_from_slice(&buf[..end]);
        self.truncated = true;
        Ok(end)
    }

    fn flush(&mut self) -> io::Result<()> {
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

    // ---- commit fixtures ------------------------------------------------

    /// Numeric-id no-reply form, as `AGENTS.md` requires of attribution.
    const CO_AUTHOR: &str = "Ada <1+ada@users.noreply.github.com>";

    fn git_ok(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("git {args:?} could not run: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn signing_key(dir: &Path) -> PathBuf {
        let key = dir.join("signing_key");
        let output = std::process::Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-q", "-f"])
            .arg(&key)
            .output()
            .expect("ssh-keygen is required to test commit signing");
        assert!(
            output.status.success(),
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        key
    }

    /// A git project root whose signing configuration is set **locally**, so
    /// the test never inherits (or depends on) the developer's global key.
    fn git_project(dir: &Path, key: &Path) -> PathBuf {
        let root = dir.join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("README.md"), b"# project").unwrap();
        git_ok(&root, &["init", "-q", "-b", "main"]);
        git_ok(&root, &["config", "user.email", "afs@example.test"]);
        git_ok(&root, &["config", "user.name", "AFS Test"]);
        git_ok(&root, &["config", "gpg.format", "ssh"]);
        git_ok(
            &root,
            &["config", "user.signingkey", &key.to_string_lossy()],
        );
        git_ok(&root, &["add", "--all"]);
        git_ok(&root, &["commit", "--no-gpg-sign", "-q", "-m", "base"]);
        root
    }

    fn delta_write(store: &AfsStore, id: &str, path: &str, data: &[u8]) {
        let mut fs = store.open_delta(id).unwrap();
        fs.write_file(path, data).unwrap();
    }

    fn delta_remove(store: &AfsStore, id: &str, path: &str) {
        let binding = store.binding(id).unwrap();
        let mut overlay = store.open_overlay(id, &binding).unwrap();
        overlay.remove_file(path).unwrap();
    }

    fn delta_symlink(store: &AfsStore, id: &str, target: &str, link: &str) {
        let mut fs = store.open_delta(id).unwrap();
        fs.symlink(target, link).unwrap();
    }

    fn commit_err(store: &AfsStore, id: &str) -> AfsError {
        store
            .commit(id, &CommitRequest::default())
            .expect_err("commit should have been refused")
    }

    #[test]
    fn commit_materializes_a_signed_branch_carrying_provenance_trailers() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());
        let view = create(&store, &root);

        delta_write(&store, &view.id, "/src/added.rs", b"// new");
        delta_write(&store, &view.id, "/README.md", b"# changed");

        let committed = store
            .commit(
                &view.id,
                &CommitRequest {
                    message: Some("afs: land the delta".into()),
                    co_authors: vec![CO_AUTHOR.to_string()],
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(committed.state, STATE_COMMITTED);
        assert_eq!(committed.branch, format!("afs/{}", view.id));
        assert!(!committed.commit.is_empty());

        // The commit object itself must carry a signature: signing is not
        // something materialization is allowed to skip.
        let object = git_ok(&root, &["cat-file", "commit", &committed.commit]);
        assert!(object.contains("gpgsig"), "commit must be signed: {object}");

        let message = git_ok(&root, &["log", "-1", "--format=%B", &committed.commit]);
        assert!(message.contains("afs: land the delta"));
        assert!(message.contains(&format!("Coven-Afs-Session: {}", view.id)));
        assert!(message.contains("Coven-Bead: coven-5kt"));
        assert!(message.contains(&format!("Co-authored-by: {CO_AUTHOR}")));

        // The change set is on the branch, not merely staged somewhere.
        let added = git_ok(
            &root,
            &["show", &format!("{}:src/added.rs", committed.commit)],
        );
        assert_eq!(added, "// new");
        let changed = git_ok(&root, &["show", &format!("{}:README.md", committed.commit)]);
        assert_eq!(changed, "# changed");

        // §5.8 — the delta survives commit; it is the audit record.
        let after = store.get(&view.id).unwrap();
        assert_eq!(after.state, STATE_COMMITTED);
        let delta = store.open_delta(&view.id).unwrap();
        let commits = delta.commits().unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].state, STATE_COMMITTED);
        assert_eq!(
            commits[0].commit_sha.as_deref(),
            Some(committed.commit.as_str())
        );
        assert_eq!(
            commits[0].provenance_high_water,
            committed.provenance_high_water
        );
    }

    #[test]
    fn a_dry_run_previews_the_commit_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());
        let view = create(&store, &root);
        delta_write(&store, &view.id, "/src/added.rs", b"// new");
        delta_write(&store, &view.id, "/README.md", b"# changed");

        let preview = store
            .commit_dry_run(&view.id, &CommitRequest::default())
            .unwrap();
        assert!(preview.dry_run && preview.would_commit);
        assert_eq!(preview.branch, format!("afs/{}", view.id));
        assert_eq!(preview.counts.added, 1);
        assert_eq!(preview.counts.modified, 1);
        assert_eq!(preview.files, 2);

        // Zero effects: no branch, no worktree, no state change, no audit row.
        assert!(!git_branch_exists(&root, &preview.branch));
        assert!(!PathBuf::from(&preview.worktree_path).exists());
        assert_eq!(store.get(&view.id).unwrap().state, STATE_OPEN);
        assert!(store
            .open_delta(&view.id)
            .unwrap()
            .commits()
            .unwrap()
            .is_empty());

        // And a dry run that says "would commit" is followed by one that does.
        let committed = store.commit(&view.id, &CommitRequest::default()).unwrap();
        assert_eq!(committed.state, STATE_COMMITTED);
        assert_eq!(committed.branch, preview.branch);
        assert_eq!(
            committed.provenance_high_water,
            preview.provenance_high_water
        );
    }

    #[test]
    fn a_dry_run_raises_the_same_refusals_a_real_commit_would() {
        // Each case is the refusal a real commit raises, proven by asking for
        // both and comparing the dotted code — a preview that disagreed with
        // the commit would be worse than no preview.
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());

        let escaping = create(&store, &root);
        delta_write(&store, &escaping.id, "/.git/config", b"[core]\n");
        let (_, dry_code, _) = store
            .commit_dry_run(&escaping.id, &CommitRequest::default())
            .expect_err("a .git write must be refused")
            .parts();
        let (_, real_code, _) = commit_err(&store, &escaping.id).parts();
        assert_eq!(dry_code, "afs.path_outside_root");
        assert_eq!(dry_code, real_code);

        let conflicted = create(&store, &root);
        delta_write(&store, &conflicted.id, "/src/added.rs", b"// new");
        git_ok(&root, &["branch", &format!("afs/{}", conflicted.id)]);
        let (_, dry_conflict, _) = store
            .commit_dry_run(&conflicted.id, &CommitRequest::default())
            .expect_err("an existing branch must be refused")
            .parts();
        assert_eq!(dry_conflict, "afs.commit_conflict");

        // Divergence is checked before anything else, so it must also surface.
        let diverged = create(&store, &root);
        delta_write(&store, &diverged.id, "/src/added.rs", b"// new");
        std::fs::write(root.join("drift.txt"), b"drift").unwrap();
        git_ok(&root, &["add", "--all"]);
        git_ok(&root, &["commit", "--no-gpg-sign", "-q", "-m", "drift"]);
        let (_, dry_diverged, _) = store
            .commit_dry_run(&diverged.id, &CommitRequest::default())
            .expect_err("a moved base must be refused")
            .parts();
        assert_eq!(dry_diverged, "afs.base_diverged");

        // Still nothing written by any of the three previews.
        assert_eq!(store.get(&escaping.id).unwrap().state, STATE_OPEN);
        assert_eq!(store.get(&diverged.id).unwrap().state, STATE_OPEN);
    }

    #[test]
    fn commit_refuses_a_diverged_base_and_preserves_the_delta() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());
        let view = create(&store, &root);
        delta_write(&store, &view.id, "/src/added.rs", b"// new");

        // The project root moves off the recorded base.
        std::fs::write(root.join("drift.txt"), b"drift").unwrap();
        git_ok(&root, &["add", "--all"]);
        git_ok(&root, &["commit", "--no-gpg-sign", "-q", "-m", "drift"]);

        let error = commit_err(&store, &view.id);
        let (status, code, _) = error.parts();
        assert_eq!(status, 409);
        assert_eq!(code, "afs.base_diverged");

        // Preserved, not discarded, and still open for a retry after a rebase.
        assert_eq!(store.get(&view.id).unwrap().state, STATE_OPEN);
        assert_eq!(store.diff(&view.id).unwrap().counts.added, 1);
    }

    #[test]
    fn commit_refuses_writes_under_git_and_leaves_no_branch() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());
        let view = create(&store, &root);
        delta_write(&store, &view.id, "/.git/config", b"[core]\n");

        let error = commit_err(&store, &view.id);
        let (status, code, _) = error.parts();
        assert_eq!(status, 400);
        assert_eq!(code, "afs.path_outside_root");

        // Refusal happens before anything is created.
        assert!(!git_branch_exists(&root, &format!("afs/{}", view.id)));
        assert!(!root.join(".worktrees").exists());
        assert_eq!(store.get(&view.id).unwrap().state, STATE_OPEN);
    }

    #[test]
    fn commit_refuses_a_symlink_whose_target_escapes_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());
        let view = create(&store, &root);
        delta_symlink(&store, &view.id, "../../../../etc/passwd", "/src/escape");

        let error = commit_err(&store, &view.id);
        let (_, code, message) = error.parts();
        assert_eq!(code, "afs.path_outside_root");
        assert!(message.contains("symlink target"), "{message}");
        assert!(!git_branch_exists(&root, &format!("afs/{}", view.id)));
    }

    #[test]
    fn commit_refuses_a_symlinked_worktrees_root() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());
        let view = create(&store, &root);
        delta_write(&store, &view.id, "/src/added.rs", b"// new");

        let outside = dir.path().join("outside-worktrees");
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join(".worktrees")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, root.join(".worktrees")).unwrap();

        let error = commit_err(&store, &view.id);
        let (status, code, message) = error.parts();
        assert_eq!(status, 409);
        assert_eq!(code, "afs.commit_conflict");
        assert!(message.contains(".worktrees"));
    }

    #[test]
    fn commit_refuses_materializing_through_a_parent_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        git_ok(
            &root,
            &["rm", "--quiet", "--cached", "--force", "--", "src/main.rs"],
        );
        std::fs::remove_file(root.join("src/main.rs")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("src/main.rs")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, root.join("src/main.rs")).unwrap();
        git_ok(&root, &["add", "--all"]);
        git_ok(
            &root,
            &["commit", "--no-gpg-sign", "-q", "-m", "add escape symlink"],
        );

        let store = store(dir.path());
        let view = create(&store, &root);
        delta_write(&store, &view.id, "/src/main.rs/escape.txt", b"must fail");

        let error = commit_err(&store, &view.id);
        let (status, code, message) = error.parts();
        assert_eq!(status, 400);
        assert_eq!(code, "afs.path_outside_root");
        assert!(message.contains("parent"));
    }

    #[test]
    fn commit_replaces_a_symlink_leaf_without_following_it() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::remove_file(root.join("src/main.rs")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("src/main.rs")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, root.join("src/main.rs")).unwrap();
        git_ok(&root, &["add", "--all"]);
        git_ok(
            &root,
            &["commit", "--no-gpg-sign", "-q", "-m", "leaf symlink"],
        );

        let store = store(dir.path());
        let view = create(&store, &root);
        delta_write(
            &store,
            &view.id,
            "/src/main.rs",
            b"fn main() { println!(\"safe\"); }",
        );

        let committed = store
            .commit(&view.id, &CommitRequest::default())
            .expect("commit should replace the symlink with a file");
        assert_eq!(committed.state, STATE_COMMITTED);
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        assert_eq!(
            git_ok(
                &root,
                &["show", &format!("{}:src/main.rs", committed.commit)]
            ),
            "fn main() { println!(\"safe\"); }"
        );
    }

    #[test]
    fn apply_plan_removes_a_dangling_symlink_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("worktree");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let dangling_target = root.join("missing-target");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dangling_target, root.join("src/dangle")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&dangling_target, root.join("src/dangle")).unwrap();

        apply_plan(
            &root,
            &[Planned::Removal {
                path: "/src/dangle".to_string(),
            }],
        )
        .expect("apply_plan should remove a dangling symlink");
        assert!(std::fs::symlink_metadata(root.join("src/dangle")).is_err());
    }

    #[test]
    fn commit_refuses_a_file_over_the_copy_up_cap() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());
        let view = create(&store, &root);
        let oversized = vec![b'x'; COPY_UP_MAX_BYTES as usize + 1];
        delta_write(&store, &view.id, "/src/huge.bin", &oversized);

        let error = commit_err(&store, &view.id);
        let (status, code, _) = error.parts();
        assert_eq!(status, 413);
        assert_eq!(code, "afs.copy_up_too_large");
        assert!(!git_branch_exists(&root, &format!("afs/{}", view.id)));
    }

    #[test]
    fn commit_refuses_a_branch_that_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        let store = store(dir.path());
        let view = create(&store, &root);
        delta_write(&store, &view.id, "/src/added.rs", b"// new");
        git_ok(&root, &["branch", &format!("afs/{}", view.id)]);

        let error = commit_err(&store, &view.id);
        let (status, code, _) = error.parts();
        assert_eq!(status, 409);
        assert_eq!(code, "afs.commit_conflict");
        assert_eq!(store.get(&view.id).unwrap().state, STATE_OPEN);
    }

    #[test]
    fn commit_reports_unsigned_and_rolls_the_attempt_back() {
        let dir = tempfile::tempdir().unwrap();
        let key = signing_key(dir.path());
        let root = git_project(dir.path(), &key);
        // Point signing at a key that does not exist: git will refuse to
        // produce the object, and Coven must not fall back to unsigned.
        git_ok(
            &root,
            &[
                "config",
                "user.signingkey",
                &dir.path().join("missing_key").to_string_lossy(),
            ],
        );
        let store = store(dir.path());
        let view = create(&store, &root);
        delta_write(&store, &view.id, "/src/added.rs", b"// new");

        let error = commit_err(&store, &view.id);
        let (status, code, _) = error.parts();
        assert_eq!(status, 500);
        assert_eq!(code, "afs.commit_unsigned");

        // All-or-nothing: no branch, no worktree, and the session is handed
        // back exactly as the caller found it.
        let branch = format!("afs/{}", view.id);
        assert!(
            !git_branch_exists(&root, &branch),
            "branch must be torn down"
        );
        assert!(
            !root
                .join(".worktrees")
                .join(worktree_slug(&branch))
                .exists(),
            "worktree must be torn down"
        );
        assert_eq!(store.get(&view.id).unwrap().state, STATE_OPEN);
        assert_eq!(store.diff(&view.id).unwrap().counts.added, 1);

        // The failed attempt is still recorded, so the audit shows it.
        let delta = store.open_delta(&view.id).unwrap();
        let commits = delta.commits().unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].state, "failed");
        assert!(commits[0].commit_sha.is_none());
    }

    #[test]
    fn symlink_escape_detection_is_lexical_and_unclamped() {
        // `normalize` clamps `..` at the root, so escape has to be judged
        // without it — these are the cases that clamping would hide.
        assert!(symlink_escapes("/a/link", "../../etc/passwd"));
        assert!(symlink_escapes("/link", "../outside"));
        assert!(symlink_escapes("/a/link", "/etc/passwd"));
        assert!(!symlink_escapes("/a/link", "sibling"));
        assert!(!symlink_escapes("/a/b/link", "../c"));
        assert!(!symlink_escapes("/a/b/link", "../../a/c"));
    }

    #[test]
    fn verbatim_prefixes_are_stripped_for_git() {
        // What canonicalize hands back on Windows, and what git can actually
        // create directories under.
        assert_eq!(
            strip_verbatim_str(r"\\?\C:\work\repo").as_deref(),
            Some(r"C:\work\repo")
        );
        assert_eq!(
            strip_verbatim_str(r"\\?\UNC\server\share\repo").as_deref(),
            Some(r"\\server\share\repo")
        );
        // Everything else is left exactly as it was.
        assert_eq!(strip_verbatim_str(r"C:\work\repo"), None);
        assert_eq!(strip_verbatim_str("/srv/repo"), None);
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from("/srv/repo")),
            PathBuf::from("/srv/repo")
        );
    }

    #[test]
    fn escape_rejection_covers_git_and_the_root_itself() {
        assert!(reject_escape("/src/main.rs").is_ok());
        assert!(
            reject_escape("/.gitignore").is_ok(),
            "only .git/ is refused"
        );
        assert!(reject_escape("/").is_err());
        assert!(reject_escape("/.git/config").is_err());
        assert!(reject_escape("/.git").is_err());
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
    fn file_diff_reports_a_modified_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        std::fs::write(root.join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let store = store(dir.path());
        let view = create(&store, &root);

        delta_write(
            &store,
            &view.id,
            "/notes.txt",
            b"alpha\nbeta changed\ngamma\n",
        );

        let diff = store.file_diff(&view.id, "/notes.txt").unwrap();
        assert_eq!(diff.path, "/notes.txt");
        assert!(!diff.binary);
        assert!(!diff.truncated);
        assert!(diff.patch.contains("--- /notes.txt"));
        assert!(diff.patch.contains("+++ /notes.txt"));
        assert!(diff.patch.contains("-beta"));
        assert!(diff.patch.contains("+beta changed"));
    }

    #[test]
    fn file_diff_reports_an_added_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);

        delta_write(&store, &view.id, "/src/added.txt", b"new line\n");

        let diff = store.file_diff(&view.id, "/src/added.txt").unwrap();
        assert_eq!(diff.path, "/src/added.txt");
        assert!(!diff.binary);
        assert!(!diff.truncated);
        assert!(diff.patch.contains("--- /dev/null"));
        assert!(diff.patch.contains("+++ /src/added.txt"));
        assert!(diff.patch.contains("+new line"));
    }

    #[test]
    fn file_diff_reports_a_deleted_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        std::fs::write(root.join("notes.txt"), "alpha\nbeta\n").unwrap();
        let store = store(dir.path());
        let view = create(&store, &root);

        delta_remove(&store, &view.id, "/notes.txt");

        let diff = store.file_diff(&view.id, "/notes.txt").unwrap();
        assert_eq!(diff.path, "/notes.txt");
        assert!(!diff.binary);
        assert!(!diff.truncated);
        assert!(diff.patch.contains("--- /notes.txt"));
        assert!(diff.patch.contains("+++ /dev/null"));
        assert!(diff.patch.contains("-alpha"));
        assert!(diff.patch.contains("-beta"));
    }

    #[test]
    fn file_diff_distinguishes_added_and_deleted_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        std::fs::write(root.join("deleted-empty.txt"), b"").unwrap();
        let store = store(dir.path());
        let view = create(&store, &root);

        delta_write(&store, &view.id, "/added-empty.txt", b"");
        delta_remove(&store, &view.id, "/deleted-empty.txt");

        let added = store.file_diff(&view.id, "/added-empty.txt").unwrap();
        assert_eq!(
            added.patch, "--- /dev/null\n+++ /added-empty.txt\n",
            "an empty added file still needs an explicit patch"
        );
        assert!(!added.truncated);
        assert!(!added.binary);

        let deleted = store.file_diff(&view.id, "/deleted-empty.txt").unwrap();
        assert_eq!(
            deleted.patch, "--- /deleted-empty.txt\n+++ /dev/null\n",
            "an empty deleted file still needs an explicit patch"
        );
        assert!(!deleted.truncated);
        assert!(!deleted.binary);
    }

    #[test]
    fn file_diff_escapes_control_characters_in_headers() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);
        let path = "/odd\tname\n.txt";

        delta_write(&store, &view.id, path, b"contents\n");

        let diff = store.file_diff(&view.id, path).unwrap();
        assert_eq!(diff.path, path);
        assert!(diff.patch.contains("+++ \"/odd\\tname\\n.txt\""));
        assert!(!diff.patch.contains("+++ /odd\tname\n.txt"));
    }

    #[test]
    fn file_diff_reports_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);

        delta_write(&store, &view.id, "/README.md", &[0xff, 0xfe, 0xfd]);

        let diff = store.file_diff(&view.id, "/README.md").unwrap();
        assert_eq!(diff.path, "/README.md");
        assert!(diff.binary);
        assert!(!diff.truncated);
        assert_eq!(diff.patch, "Binary files differ\n");
    }

    #[test]
    fn file_diff_does_not_report_unchanged_binary_files_as_different() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        std::fs::write(root.join("image.bin"), [0xff, 0xfe, 0xfd]).unwrap();
        let store = store(dir.path());
        let view = create(&store, &root);

        let diff = store.file_diff(&view.id, "/image.bin").unwrap();
        assert_eq!(diff.path, "/image.bin");
        assert!(diff.binary);
        assert!(!diff.truncated);
        assert_eq!(diff.patch, "");
    }

    #[test]
    fn file_diff_reports_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);

        assert!(matches!(
            store.file_diff(&view.id, "/missing.txt"),
            Err(AfsError::PathNotFound(path)) if path == "/missing.txt"
        ));
    }

    #[test]
    fn file_diff_rejects_non_regular_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);

        let mut delta = store.open_delta(&view.id).unwrap();
        delta.mkdir_p("/fresh").unwrap();
        delta_symlink(&store, &view.id, "/README.md", "/readme-link");

        for path in ["/fresh", "/readme-link"] {
            let error = store.file_diff(&view.id, path).unwrap_err();
            let (status, code, message) = error.parts();
            assert_eq!(status, 400);
            assert_eq!(code, "afs.path_not_file");
            assert!(message.contains(path));
        }
    }

    #[test]
    fn file_diff_truncates_oversized_patches() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path());
        let store = store(dir.path());
        let view = create(&store, &root);

        let large = "é\n".repeat(140_000);
        delta_write(&store, &view.id, "/src/large.txt", large.as_bytes());

        let diff = store.file_diff(&view.id, "/src/large.txt").unwrap();
        assert_eq!(diff.path, "/src/large.txt");
        assert!(!diff.binary);
        assert!(diff.truncated);
        assert!(diff.patch.len() <= UNIFIED_DIFF_MAX_BYTES);
        assert!(std::str::from_utf8(diff.patch.as_bytes()).is_ok());
        assert!(diff.patch.contains("--- /dev/null"));
        assert!(diff.patch.contains("+++ /src/large.txt"));
        assert!(diff.patch.starts_with("--- /dev/null"));
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
