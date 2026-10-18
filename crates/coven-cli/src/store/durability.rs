//! Durable authority transactions without changing session/event commit policy.
use crate::threads_clock::request_diagnostics::{checkpoint, Phase};
use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior};

/// Nested authority writes may join only an already durable transaction. The
/// outer caller owns its commit/barrier; a NORMAL transaction cannot be upgraded
/// after BEGIN. On every exit, ordinary writes regain their original mode.
pub(crate) fn transaction<T>(conn: &Connection, write: impl FnOnce() -> Result<T>) -> Result<T> {
    checkpoint(Phase::DurableModeBegin);
    let synchronous: i64 = conn.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    let fullfsync: bool = conn.pragma_query_value(None, "fullfsync", |row| row.get(0))?;
    if !conn.is_autocommit() {
        anyhow::ensure!(
            synchronous >= 2 && fullfsync,
            "authority write requires a durable outer transaction"
        );
        checkpoint(Phase::DurableModeReady);
        return write();
    }
    let restore = RestoreMode {
        conn,
        synchronous,
        fullfsync,
    };
    conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA fullfsync = ON;")
        .context("enabling durable authority commits")?;
    checkpoint(Phase::DurableModeReady);
    // Retain the directory before committing. SQLite's Unix directory sync is
    // best effort; ours is checked (and uses F_FULLFSYNC on macOS).
    checkpoint(Phase::DurableDirectoryOpenBegin);
    let directory = conn
        .path()
        .filter(|path| !path.is_empty())
        .map(std::path::Path::new)
        .and_then(std::path::Path::parent)
        .map(crate::durable_fs::open_directory)
        .transpose()?;
    checkpoint(Phase::DurableDirectoryOpenReady);
    checkpoint(Phase::DurableParentOpenBegin);
    let home_parent = conn
        .path()
        .filter(|path| !path.is_empty())
        .map(std::path::Path::new)
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .map(crate::durable_fs::open_directory)
        .transpose()?;
    checkpoint(Phase::DurableParentOpenReady);
    let result = (|| {
        checkpoint(Phase::DurableTransactionBegin);
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        checkpoint(Phase::DurableTransactionReady);
        checkpoint(Phase::DurableWriteBegin);
        let value = write()?;
        checkpoint(Phase::DurableWriteReady);
        checkpoint(Phase::DurableCommitBegin);
        tx.commit()
            .context("committing durable authority evidence")?;
        checkpoint(Phase::DurableCommitReady);
        if let Some(directory) = directory.as_ref() {
            checkpoint(Phase::DurableDirectorySyncBegin);
            crate::durable_fs::sync_directory(directory)?;
            checkpoint(Phase::DurableDirectorySyncReady);
        }
        if let Some(home_parent) = home_parent.as_ref() {
            checkpoint(Phase::DurableParentSyncBegin);
            crate::durable_fs::sync_directory(home_parent)?;
            checkpoint(Phase::DurableParentSyncReady);
        }
        Ok(value)
    })();
    checkpoint(Phase::DurableRestoreBegin);
    let restored = restore.restore().context("restoring store commit policy");
    // Include the guard's existing fallback restoration in the observed phase.
    drop(restore);
    restored?;
    checkpoint(Phase::DurableRestoreReady);
    result
}

/// Force a real FULL commit even when the authority evidence already exists.
/// Used on retries after a possibly failed post-COMMIT namespace barrier, and
/// when recovering intents written by older NORMAL-mode versions.
pub(crate) fn barrier(conn: &Connection) -> Result<()> {
    transaction(conn, || {
        conn.execute("INSERT INTO store_meta(key, value) VALUES ('ward_durability_barrier', hex(randomblob(16)))
            ON CONFLICT(key) DO UPDATE SET value = excluded.value", [])?;
        Ok(())
    })
}

struct RestoreMode<'a> {
    conn: &'a Connection,
    synchronous: i64,
    fullfsync: bool,
}
impl RestoreMode<'_> {
    fn restore(&self) -> rusqlite::Result<()> {
        self.conn
            .pragma_update(None, "synchronous", self.synchronous)?;
        self.conn.pragma_update(None, "fullfsync", self.fullfsync)
    }
}
impl Drop for RestoreMode<'_> {
    fn drop(&mut self) {
        // The transaction is declared after this guard and rolls back first on
        // unwind. If restoring fails, leave the connection stricter, never OFF.
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_post_commit_directory_barrier_is_retried() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp.path().join("coven.sqlite3"))?;
        let result = transaction(&conn, || {
            conn.execute(
                "INSERT INTO store_meta(key, value) VALUES ('committed-before-sync-error', 'yes')",
                [],
            )?;
            crate::durable_fs::fail_next_directory_sync();
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(
            conn.query_row(
                "SELECT value FROM store_meta WHERE key = 'committed-before-sync-error'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "yes"
        );
        crate::durable_fs::fail_next_directory_sync();
        assert!(
            barrier(&conn).is_err(),
            "retry must repeat the failed namespace barrier"
        );
        barrier(&conn)?;
        assert_eq!(
            conn.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
            1
        );
        Ok(())
    }

    #[test]
    fn authority_transaction_restores_normal_on_success_error_and_unwind() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp.path().join("coven.sqlite3"))?;
        for outcome in 0..3 {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                transaction(&conn, || -> Result<()> {
                    conn.execute(
                        "INSERT INTO store_meta(key, value) VALUES (?1, 'written')",
                        [format!("durability-{outcome}")],
                    )?;
                    match outcome {
                        0 => Ok(()),
                        1 => anyhow::bail!("injected transaction failure"),
                        _ => panic!("injected transaction unwind"),
                    }
                })
            }));
            assert_eq!(result.is_ok_and(|result| result.is_ok()), outcome == 0);
            assert!(conn.is_autocommit());
            assert_eq!(
                conn.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
                1
            );
        }
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM store_meta WHERE key LIKE 'durability-%'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 1, "failed/unwound transactions must roll back");
        let tx = conn.unchecked_transaction()?;
        assert!(
            transaction(&conn, || Ok(())).is_err(),
            "never join a NORMAL transaction"
        );
        tx.rollback()?;
        Ok(())
    }
}
