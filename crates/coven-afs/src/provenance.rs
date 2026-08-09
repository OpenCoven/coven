//! Coven's extension tables: session binding, per-operation provenance, and
//! commit materialization records.
//!
//! These are the `afs_*` tables specified in `specs/coven-agent-fs/DESIGN.md`
//! §4. The interop rules from §1 are enforced here by construction:
//!
//! - **E1** — no SPEC v0.4 table is touched. Nothing in this module writes to
//!   `fs_*`, `kv_store`, or `tool_calls`.
//! - **E2** — every table is named `afs_*` and carries metadata only. Dropping
//!   all of them must leave filesystem semantics unchanged, which
//!   [`drop_extensions`] plus the conformance test prove rather than assert.
//! - **E3** — tables are created lazily and every column is nullable or
//!   defaulted, so a database produced by upstream `agentfs` reads as one with
//!   no provenance yet rather than as an error.

use rusqlite::{params, OptionalExtension};

use crate::{now_parts, AgentFs, Result};

/// DDL for every coven extension table. Idempotent.
const EXTENSION_DDL: &str = "
CREATE TABLE IF NOT EXISTS afs_session (
  id                TEXT PRIMARY KEY,
  name              TEXT,
  state             TEXT NOT NULL DEFAULT 'open',
  base_fingerprint  TEXT,
  base_commit       TEXT,
  project_root      TEXT,
  coven_session_id  TEXT,
  familiar_id       TEXT,
  bead_id           TEXT,
  created_at        INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at        INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS afs_provenance (
  seq               INTEGER PRIMARY KEY AUTOINCREMENT,
  op                TEXT NOT NULL,
  path              TEXT NOT NULL,
  to_path           TEXT,
  ino               INTEGER,
  base_ino          INTEGER,
  bytes             INTEGER NOT NULL DEFAULT 0,
  at                INTEGER NOT NULL DEFAULT (unixepoch()),
  at_nsec           INTEGER NOT NULL DEFAULT 0,
  afs_session_id    TEXT,
  coven_session_id  TEXT,
  familiar_id       TEXT,
  bead_id           TEXT,
  turn              INTEGER,
  tool_call_id      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_afs_provenance_path ON afs_provenance(path, seq);
CREATE INDEX IF NOT EXISTS idx_afs_provenance_session ON afs_provenance(coven_session_id, seq);
CREATE INDEX IF NOT EXISTS idx_afs_provenance_bead ON afs_provenance(bead_id, seq);
CREATE INDEX IF NOT EXISTS idx_afs_provenance_tool_call ON afs_provenance(tool_call_id);

CREATE TABLE IF NOT EXISTS afs_commit (
  id                     INTEGER PRIMARY KEY AUTOINCREMENT,
  branch                 TEXT NOT NULL,
  commit_sha             TEXT,
  worktree_path          TEXT,
  provenance_high_water  INTEGER NOT NULL DEFAULT 0,
  state                  TEXT NOT NULL DEFAULT 'planned',
  created_at             INTEGER NOT NULL DEFAULT (unixepoch())
);
";

/// Every table this module owns. The conformance test drops exactly this list.
pub const EXTENSION_TABLES: &[&str] = &["afs_provenance", "afs_commit", "afs_session"];

/// Lifecycle state of a session delta.
pub const STATE_OPEN: &str = "open";
pub const STATE_COMMITTING: &str = "committing";
pub const STATE_COMMITTED: &str = "committed";
pub const STATE_DISCARDED: &str = "discarded";

/// The binding of a delta database to the Coven session that opened it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionBinding {
    pub id: String,
    pub name: Option<String>,
    pub state: String,
    pub base_fingerprint: Option<String>,
    pub base_commit: Option<String>,
    pub project_root: Option<String>,
    pub coven_session_id: Option<String>,
    pub familiar_id: Option<String>,
    pub bead_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Who performed an operation. Repeated per row rather than held once on
/// [`SessionBinding`] because `afs.session.join` means more than one actor can
/// write to one delta: the acting identity is a property of the operation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Actor {
    pub afs_session_id: Option<String>,
    pub coven_session_id: Option<String>,
    pub familiar_id: Option<String>,
    pub bead_id: Option<String>,
    /// The session's event cursor (`MAX(events.rowid)`) when the operation ran.
    pub turn: Option<i64>,
    pub tool_call_id: Option<i64>,
}

/// One recorded file operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceRecord {
    pub seq: i64,
    pub op: String,
    pub path: String,
    pub to_path: Option<String>,
    pub ino: Option<i64>,
    pub base_ino: Option<i64>,
    pub bytes: i64,
    pub at: i64,
    pub at_nsec: i64,
    pub actor: Actor,
}

/// A materialization of a delta into a git branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRecord {
    pub id: i64,
    pub branch: String,
    pub commit_sha: Option<String>,
    pub worktree_path: Option<String>,
    pub provenance_high_water: i64,
    pub state: String,
    pub created_at: i64,
}

impl AgentFs {
    /// Create the coven extension tables if they are absent.
    ///
    /// Safe to call on a database produced by upstream `agentfs`; that is what
    /// makes rule E3 hold in the coven-reads-foreign direction.
    pub fn init_extensions(&self) -> Result<()> {
        self.conn.execute_batch(EXTENSION_DDL)?;
        Ok(())
    }

    /// Whether this database carries coven extensions at all.
    pub fn has_extensions(&self) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'afs_session'",
            [],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Whether a table exists in this database. Useful for interop checks
    /// against databases produced elsewhere.
    pub fn table_exists(&self, name: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Drop every coven extension table, leaving a database that upstream
    /// tooling sees as an ordinary AgentFS filesystem.
    ///
    /// Exists so rule E2 can be *tested* rather than asserted: the conformance
    /// test runs the SPEC consistency rules, drops these, and runs them again.
    pub fn drop_extensions(&self) -> Result<()> {
        for table in EXTENSION_TABLES {
            self.conn
                .execute(&format!("DROP TABLE IF EXISTS {table}"), [])?;
        }
        Ok(())
    }

    /// Record (or replace) this delta's session binding.
    pub fn bind_session(&self, binding: &SessionBinding) -> Result<()> {
        self.init_extensions()?;
        let (secs, _) = now_parts();
        self.conn.execute(
            "INSERT INTO afs_session
               (id, name, state, base_fingerprint, base_commit, project_root,
                coven_session_id, familiar_id, bead_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               state = excluded.state,
               base_fingerprint = excluded.base_fingerprint,
               base_commit = excluded.base_commit,
               project_root = excluded.project_root,
               coven_session_id = excluded.coven_session_id,
               familiar_id = excluded.familiar_id,
               bead_id = excluded.bead_id,
               updated_at = excluded.updated_at",
            params![
                binding.id,
                binding.name,
                binding.state,
                binding.base_fingerprint,
                binding.base_commit,
                binding.project_root,
                binding.coven_session_id,
                binding.familiar_id,
                binding.bead_id,
                secs,
            ],
        )?;
        Ok(())
    }

    /// Read the session binding, if this database has one.
    pub fn session_binding(&self) -> Result<Option<SessionBinding>> {
        if !self.has_extensions()? {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, state, base_fingerprint, base_commit, project_root,
                        coven_session_id, familiar_id, bead_id, created_at, updated_at
                 FROM afs_session ORDER BY created_at ASC LIMIT 1",
                [],
                |r| {
                    Ok(SessionBinding {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        state: r.get(2)?,
                        base_fingerprint: r.get(3)?,
                        base_commit: r.get(4)?,
                        project_root: r.get(5)?,
                        coven_session_id: r.get(6)?,
                        familiar_id: r.get(7)?,
                        bead_id: r.get(8)?,
                        created_at: r.get(9)?,
                        updated_at: r.get(10)?,
                    })
                },
            )
            .optional()?)
    }

    /// Move the session to a new lifecycle state.
    pub fn set_session_state(&self, id: &str, state: &str) -> Result<()> {
        self.init_extensions()?;
        let (secs, _) = now_parts();
        self.conn.execute(
            "UPDATE afs_session SET state = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, state, secs],
        )?;
        Ok(())
    }

    /// Append one operation to the provenance log. Returns its `seq`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_operation(
        &self,
        op: &str,
        path: &str,
        to_path: Option<&str>,
        ino: Option<i64>,
        base_ino: Option<i64>,
        bytes: i64,
        actor: &Actor,
    ) -> Result<i64> {
        self.init_extensions()?;
        let (secs, nsec) = now_parts();
        self.conn.execute(
            "INSERT INTO afs_provenance
               (op, path, to_path, ino, base_ino, bytes, at, at_nsec,
                afs_session_id, coven_session_id, familiar_id, bead_id, turn, tool_call_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                op,
                path,
                to_path,
                ino,
                base_ino,
                bytes,
                secs,
                nsec,
                actor.afs_session_id,
                actor.coven_session_id,
                actor.familiar_id,
                actor.bead_id,
                actor.turn,
                actor.tool_call_id,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Read provenance after `since`, oldest first, capped at `limit`.
    ///
    /// Cursor-paginated on `seq` to match the daemon's existing
    /// `eventCursor: "sequence"` idiom.
    pub fn provenance_since(&self, since: i64, limit: usize) -> Result<Vec<ProvenanceRecord>> {
        if !self.has_extensions()? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT seq, op, path, to_path, ino, base_ino, bytes, at, at_nsec,
                    afs_session_id, coven_session_id, familiar_id, bead_id, turn, tool_call_id
             FROM afs_provenance WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![since, limit as i64], |r| {
                Ok(ProvenanceRecord {
                    seq: r.get(0)?,
                    op: r.get(1)?,
                    path: r.get(2)?,
                    to_path: r.get(3)?,
                    ino: r.get(4)?,
                    base_ino: r.get(5)?,
                    bytes: r.get(6)?,
                    at: r.get(7)?,
                    at_nsec: r.get(8)?,
                    actor: Actor {
                        afs_session_id: r.get(9)?,
                        coven_session_id: r.get(10)?,
                        familiar_id: r.get(11)?,
                        bead_id: r.get(12)?,
                        turn: r.get(13)?,
                        tool_call_id: r.get(14)?,
                    },
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The set of paths that provenance can account for. Used to mark diff
    /// entries whose attribution is unknown (DESIGN.md §4.4).
    pub fn attributed_paths(&self) -> Result<std::collections::HashSet<String>> {
        let mut out = std::collections::HashSet::new();
        if !self.has_extensions()? {
            return Ok(out);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM afs_provenance UNION SELECT to_path FROM afs_provenance WHERE to_path IS NOT NULL")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Highest recorded provenance sequence, or 0 when there is none.
    pub fn provenance_high_water(&self) -> Result<i64> {
        if !self.has_extensions()? {
            return Ok(0);
        }
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM afs_provenance",
            [],
            |r| r.get(0),
        )?)
    }

    /// Record a materialization attempt. Returns its id.
    pub fn record_commit(
        &self,
        branch: &str,
        commit_sha: Option<&str>,
        worktree_path: Option<&str>,
        state: &str,
    ) -> Result<i64> {
        self.init_extensions()?;
        let high_water = self.provenance_high_water()?;
        let (secs, _) = now_parts();
        self.conn.execute(
            "INSERT INTO afs_commit
               (branch, commit_sha, worktree_path, provenance_high_water, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![branch, commit_sha, worktree_path, high_water, state, secs],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List materializations, newest first.
    pub fn commits(&self) -> Result<Vec<CommitRecord>> {
        if !self.has_extensions()? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, branch, commit_sha, worktree_path, provenance_high_water, state, created_at
             FROM afs_commit ORDER BY id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(CommitRecord {
                    id: r.get(0)?,
                    branch: r.get(1)?,
                    commit_sha: r.get(2)?,
                    worktree_path: r.get(3)?,
                    provenance_high_water: r.get(4)?,
                    state: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Actor {
        Actor {
            afs_session_id: Some("afs-1".into()),
            coven_session_id: Some("sess-1".into()),
            familiar_id: Some("sage".into()),
            bead_id: Some("coven-5kt".into()),
            turn: Some(42),
            tool_call_id: None,
        }
    }

    #[test]
    fn foreign_database_without_extensions_reads_as_unbound() {
        let fs = AgentFs::in_memory().unwrap();
        // No init_extensions: this is what an upstream agentfs database looks
        // like. Reads must degrade, not fail (rule E3).
        assert!(!fs.has_extensions().unwrap());
        assert_eq!(fs.session_binding().unwrap(), None);
        assert!(fs.provenance_since(0, 10).unwrap().is_empty());
        assert_eq!(fs.provenance_high_water().unwrap(), 0);
        assert!(fs.commits().unwrap().is_empty());
        assert!(fs.attributed_paths().unwrap().is_empty());
    }

    #[test]
    fn binding_round_trips_and_updates_in_place() {
        let fs = AgentFs::in_memory().unwrap();
        let mut binding = SessionBinding {
            id: "afs-1".into(),
            name: Some("spike".into()),
            state: STATE_OPEN.into(),
            bead_id: Some("coven-5kt".into()),
            ..Default::default()
        };
        fs.bind_session(&binding).unwrap();
        binding.state = STATE_COMMITTED.into();
        fs.bind_session(&binding).unwrap();

        let stored = fs.session_binding().unwrap().unwrap();
        assert_eq!(stored.id, "afs-1");
        assert_eq!(stored.state, STATE_COMMITTED);
        let count: i64 = fs
            .conn
            .query_row("SELECT COUNT(*) FROM afs_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "binding must update in place, not accumulate");
    }

    #[test]
    fn provenance_paginates_on_seq_and_keeps_per_operation_identity() {
        let fs = AgentFs::in_memory().unwrap();
        for index in 0..5 {
            fs.record_operation(
                "write",
                &format!("/f{index}"),
                None,
                Some(index),
                None,
                10,
                &actor(),
            )
            .unwrap();
        }
        // A second actor writing to the same delta, as afs.session.join allows.
        let other = Actor {
            familiar_id: Some("echo".into()),
            turn: Some(99),
            ..actor()
        };
        fs.record_operation("write", "/shared", None, Some(9), None, 3, &other)
            .unwrap();

        let first = fs.provenance_since(0, 3).unwrap();
        assert_eq!(first.len(), 3);
        let rest = fs.provenance_since(first.last().unwrap().seq, 10).unwrap();
        assert_eq!(rest.len(), 3);
        assert_eq!(rest.last().unwrap().path, "/shared");
        assert_eq!(
            rest.last().unwrap().actor.familiar_id.as_deref(),
            Some("echo")
        );
        assert_eq!(rest.last().unwrap().actor.turn, Some(99));
        assert_eq!(fs.provenance_high_water().unwrap(), 6);
    }

    #[test]
    fn commit_records_the_provenance_it_covers() {
        let fs = AgentFs::in_memory().unwrap();
        fs.record_operation("write", "/a", None, None, None, 1, &actor())
            .unwrap();
        fs.record_operation("write", "/b", None, None, None, 1, &actor())
            .unwrap();
        fs.record_commit("afs/spike", Some("abc123"), Some("/tmp/wt"), "committed")
            .unwrap();

        let commits = fs.commits().unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].provenance_high_water, 2);
        assert_eq!(commits[0].branch, "afs/spike");
    }

    #[test]
    fn attributed_paths_include_rename_destinations() {
        let fs = AgentFs::in_memory().unwrap();
        fs.record_operation("rename", "/from", Some("/to"), None, None, 0, &actor())
            .unwrap();
        let paths = fs.attributed_paths().unwrap();
        assert!(paths.contains("/from"));
        assert!(paths.contains("/to"));
    }
}
