//! Fixture pauses and parent-side probes shared by the disposable
//! process-crash suites. A worker parks inside a production transaction, and
//! the parent inspects the store before and after killing it.
use std::path::{Component, Path};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use rusqlite::{functions::FunctionFlags, Connection, ErrorCode};
use serde_json::Value;

use super::command_process_crash_recovery::announce;

const FAILED: &str = "conformance suite execution failed";
const PAUSE_FUNCTION: &str = "coven_process_crash_pause";

/// The worker's handshake when it pauses. The pause runs inside SQLite (a
/// trigger function or the WAL hook), which cannot capture it.
static PAUSE: OnceLock<Value> = OnceLock::new();

/// Commits the WAL hook lets through before it pauses.
static COMMITS_BEFORE_PAUSE: AtomicUsize = AtomicUsize::new(0);

/// Where a fixture worker parks inside a production transaction.
pub(super) enum Pause<'a> {
    /// A TEMP trigger on the row change this clause names, before commit.
    After(&'a str),
    /// The connection's WAL hook, once SQLite has committed.
    AfterCommit,
    /// The connection's WAL hook, once SQLite has committed this many
    /// transactions (at least one).
    AfterCommits(usize),
}

/// Announces the pause, then parks until the parent kills this process.
fn pause() -> ! {
    match PAUSE.get() {
        Some(message) => park(message),
        None => loop {
            std::thread::park();
        },
    }
}

/// Announces `handshake`, then parks until the parent kills this process. For
/// a pause outside SQLite, such as a runtime's.
pub(super) fn park(handshake: &Value) -> ! {
    let _ = announce(handshake);
    loop {
        std::thread::park();
    }
}

/// Runs after SQLite has committed a transaction to the WAL.
fn pause_after_commit(_: &rusqlite::hooks::Wal, _: std::ffi::c_int) -> rusqlite::Result<()> {
    // Only the worker's one connection, on its one thread, runs this hook.
    let left = COMMITS_BEFORE_PAUSE.load(Ordering::SeqCst);
    if left > 0 {
        COMMITS_BEFORE_PAUSE.store(left - 1, Ordering::SeqCst);
        return Ok(());
    }
    pause()
}

/// Makes `conn` announce `handshake` and park at `pause_at`. One per worker
/// process. `open_store` already set `wal_autocheckpoint`, which would replace
/// the WAL hook.
pub(super) fn install_pause(
    conn: &Connection,
    pause_at: Pause<'_>,
    handshake: Value,
) -> Result<(), &'static str> {
    PAUSE.set(handshake).map_err(|_| FAILED)?;
    match pause_at {
        Pause::AfterCommit => conn.wal_hook(Some(pause_after_commit)),
        Pause::AfterCommits(commits) => {
            let before_pause = commits.checked_sub(1).ok_or(FAILED)?;
            COMMITS_BEFORE_PAUSE.store(before_pause, Ordering::SeqCst);
            conn.wal_hook(Some(pause_after_commit));
        }
        Pause::After(trigger) => {
            conn.create_scalar_function(
                PAUSE_FUNCTION,
                0,
                FunctionFlags::SQLITE_UTF8,
                |_| -> rusqlite::Result<i64> { pause() },
            )
            .map_err(|_| FAILED)?;
            conn.execute_batch(&format!(
                "CREATE TEMP TRIGGER coven_process_crash_fixture {trigger}
                 BEGIN SELECT {PAUSE_FUNCTION}(); END;"
            ))
            .map_err(|_| FAILED)?;
        }
    }
    Ok(())
}

/// Whether another connection holds the store's write lock: a `BEGIN
/// IMMEDIATE` that cannot wait fails busy.
pub(super) fn write_transaction_open(probe: &Connection) -> Result<bool, &'static str> {
    probe
        .busy_timeout(std::time::Duration::ZERO)
        .map_err(|_| FAILED)?;
    match probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;") {
        Ok(()) => Ok(false),
        Err(error) if error.sqlite_error_code() == Some(ErrorCode::DatabaseBusy) => Ok(true),
        Err(_) => Err(FAILED),
    }
}

/// Opens the store without the production initializer, which could repair
/// schema or metadata, and never creates one the worker lost.
pub(super) fn open_raw(path: &Path) -> Result<Connection, &'static str> {
    if !std::fs::symlink_metadata(path)
        .map_err(|_| FAILED)?
        .is_file()
    {
        return Err(FAILED);
    }
    Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| FAILED)
}

/// The raw integrity check, before the production initializer opens the store.
pub(super) fn raw_integrity_ok(path: &Path) -> Result<bool, &'static str> {
    let integrity: String = open_raw(path)?
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| FAILED)?;
    Ok(integrity == "ok")
}

/// Checks the worker's handshake and returns its private home under `root`.
pub(super) fn worker_home(
    root: &Path,
    relative_home: &str,
    prefix: &str,
) -> Result<std::path::PathBuf, &'static str> {
    let mut components = Path::new(relative_home).components();
    if !relative_home.starts_with(prefix)
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(FAILED);
    }
    let home = root
        .join(relative_home)
        .canonicalize()
        .map_err(|_| FAILED)?;
    if !home.starts_with(root.canonicalize().map_err(|_| FAILED)?) {
        return Err(FAILED);
    }
    Ok(home)
}
