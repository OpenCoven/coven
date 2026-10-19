//! Owned, cooperative diagnostic read budget. No caller handlers are replaced.
use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const OPS_PER_CALLBACK: i32 = 1000;
const MAX_CALLBACKS: usize = 2000;
const ELAPSED_BUDGET: Duration = Duration::from_secs(2);
#[derive(Debug)]
pub(crate) struct ReadBudgetExceeded;
impl std::fmt::Display for ReadBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Automation doctor read budget exceeded.")
    }
}
impl std::error::Error for ReadBudgetExceeded {}

#[cfg(test)]
fn owned_connection(source: &Connection) -> Result<Connection> {
    let path = source
        .path()
        .filter(|path| !path.is_empty())
        .context("doctor requires an existing file-backed main store")?;
    open_existing(Path::new(path))
}

fn open_existing(path: &Path) -> Result<Connection> {
    // SQLite NOFOLLOW also rejects symlinked ancestors (including macOS /tmp
    // and /var). Resolve only the parent, keeping the final component under
    // metadata and SQLite NOFOLLOW checks rather than following a DB symlink.
    let parent = path.parent().context("doctor store requires a parent")?;
    let name = path
        .file_name()
        .context("doctor store requires a filename")?;
    let path = std::fs::canonicalize(parent)?.join(name);
    // Store-opening helpers may harden filesystem permissions. Diagnostics
    // inspect metadata only; NOFOLLOW below closes the final-component race.
    let metadata = std::fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "doctor requires an existing regular non-symlink store"
    );
    // No CREATE/URI flags, initialization, writable-store setup or shared hooks.
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    conn.busy_timeout(Duration::ZERO)?;
    Ok(conn)
}

fn install_budget(
    conn: &Connection,
    started: Instant,
    stride: i32,
    max_callbacks: usize,
    elapsed: Duration,
) -> Result<Arc<AtomicBool>> {
    let exhausted = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&exhausted);
    let mut calls = 0usize;
    conn.progress_handler(
        stride,
        Some(move || {
            calls = calls.saturating_add(1);
            let stop = calls >= max_callbacks || started.elapsed() >= elapsed;
            if stop {
                observed.store(true, Ordering::Relaxed);
            }
            stop
        }),
    )?;
    Ok(exhausted)
}

pub(super) fn inspect(source: &Connection, now: DateTime<Utc>) -> Result<super::Report> {
    let path = source
        .path()
        .filter(|path| !path.is_empty())
        .context("doctor requires an existing file-backed main store")?;
    inspect_path(Path::new(path), now)
}

pub(super) fn inspect_path(path: &Path, now: DateTime<Utc>) -> Result<super::Report> {
    let started = Instant::now();
    let conn = open_existing(path)?;
    if started.elapsed() >= ELAPSED_BUDGET {
        bail!(ReadBudgetExceeded);
    }
    let exhausted = install_budget(
        &conn,
        started,
        OPS_PER_CALLBACK,
        MAX_CALLBACKS,
        ELAPSED_BUDGET,
    )?;
    let report = super::inspect(&conn, now);
    if exhausted.load(Ordering::Relaxed) || started.elapsed() >= ELAPSED_BUDGET {
        bail!(ReadBudgetExceeded);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_budget_interrupts_without_writes_or_leaking_to_the_source() {
        let root = tempfile::tempdir().unwrap();
        let source = crate::store::open_store(&root.path().join("coven.sqlite3")).unwrap();
        let conn = owned_connection(&source).unwrap();
        assert!(conn.is_readonly("main").unwrap());
        let before = conn.total_changes();
        let exhausted = install_budget(&conn, Instant::now(), 1, 10, Duration::MAX).unwrap();
        let error = conn.query_row::<i64,_,_>("WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<100000) SELECT sum(i) FROM n", [], |row|row.get(0)).unwrap_err();
        assert!(
            matches!(error,rusqlite::Error::SqliteFailure(ref error,_) if error.code == rusqlite::ErrorCode::OperationInterrupted)
        );
        assert!(exhausted.load(Ordering::Relaxed));
        assert_eq!(conn.total_changes(), before);
        drop(conn);
        assert_eq!(
            source
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        source
            .execute_batch("CREATE TABLE doctor_after_interrupt(id)")
            .unwrap();
    }

    #[test]
    fn owned_read_cannot_write_create_or_fall_back_to_memory() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("coven.sqlite3");
        let source = crate::store::open_store(&path).unwrap();
        let conn = owned_connection(&source).unwrap();
        assert!(conn
            .execute_batch("CREATE TABLE doctor_must_not_write(id)")
            .is_err());
        assert_eq!(
            conn.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(conn);

        assert!(owned_connection(&Connection::open_in_memory().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn owned_read_does_not_recreate_an_unlinked_store() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("coven.sqlite3");
        let source = crate::store::open_store(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(owned_connection(&source).is_err());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn owned_read_refuses_a_replaced_symlink_store() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("coven.sqlite3");
        let source = crate::store::open_store(&path).unwrap();
        let target = root.path().join("PRIVATE_other.sqlite3");
        let _other = crate::store::open_store(&target).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(owned_connection(&source).is_err());
    }

    #[test]
    fn elapsed_budget_interrupts_deterministically_without_sleeping() {
        let conn = Connection::open_in_memory().unwrap();
        let exhausted =
            install_budget(&conn, Instant::now(), 1, usize::MAX, Duration::ZERO).unwrap();
        assert!(conn
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .is_err());
        assert!(exhausted.load(Ordering::Relaxed));
    }
}
