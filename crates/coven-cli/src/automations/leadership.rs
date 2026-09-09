use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rusqlite::{params, Connection, TransactionBehavior};

pub(crate) const AUTOMATION_SCHEDULER_AUTHORITY_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_scheduler_authority (
        id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
        owner_id TEXT,
        generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
        acquired_at TEXT
    );

    INSERT OR IGNORE INTO automation_scheduler_authority
        (id, owner_id, generation, acquired_at)
    VALUES (1, NULL, 0, NULL);
";

const SCHEDULER_LOCK_FILE: &str = "automations-scheduler.lock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SchedulerFence {
    owner_id: String,
    generation: i64,
}

impl SchedulerFence {
    pub(crate) fn generation(&self) -> i64 {
        self.generation
    }

    pub(crate) fn is_current(&self, conn: &Connection) -> Result<bool> {
        conn.query_row(
            "SELECT COALESCE(owner_id = ?1 AND generation = ?2, 0)
             FROM automation_scheduler_authority
             WHERE id = 1",
            params![self.owner_id, self.generation],
            |row| row.get(0),
        )
        .context("failed to verify automations scheduler fence")
    }

    pub(crate) fn owner_id(&self) -> &str {
        &self.owner_id
    }
}

pub(crate) struct SchedulerLeadership {
    _lock: std::fs::File,
    fence: SchedulerFence,
}

impl SchedulerLeadership {
    pub(crate) fn acquire(
        coven_home: &Path,
        conn: &Connection,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        crate::daemon::ensure_private_coven_home(coven_home)?;
        let lock_path = scheduler_lock_path(coven_home);
        let lock = crate::state_lock::open_lock_file(&lock_path)?;
        match lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if crate::state_lock::is_lock_contended(&error) => {
                anyhow::bail!(
                    "automations scheduler leadership is already held for {}",
                    coven_home.display()
                );
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to acquire automations scheduler leadership {}",
                        lock_path.display()
                    )
                });
            }
        }

        let owner_id = uuid::Uuid::new_v4().to_string();
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .context("failed to begin automations scheduler authority transaction")?;
        transaction
            .execute(
                "UPDATE automation_scheduler_authority
                 SET owner_id = ?1,
                     generation = generation + 1,
                     acquired_at = ?2
                 WHERE id = 1",
                params![
                    owner_id,
                    now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                ],
            )
            .context("failed to advance automations scheduler fence")?;
        let generation = transaction
            .query_row(
                "SELECT generation
                 FROM automation_scheduler_authority
                 WHERE id = 1 AND owner_id = ?1",
                [&owner_id],
                |row| row.get(0),
            )
            .context("failed to read automations scheduler fence")?;
        transaction
            .commit()
            .context("failed to commit automations scheduler authority")?;

        Ok(Self {
            _lock: lock,
            fence: SchedulerFence {
                owner_id,
                generation,
            },
        })
    }

    pub(crate) fn fence(&self) -> SchedulerFence {
        self.fence.clone()
    }

    pub(crate) fn release(&mut self, conn: &Connection) -> Result<bool> {
        let released = conn
            .execute(
                "UPDATE automation_scheduler_authority
                 SET owner_id = NULL
                 WHERE id = 1 AND owner_id = ?1 AND generation = ?2",
                params![self.fence.owner_id, self.fence.generation],
            )
            .context("failed to release automations scheduler authority")?;
        Ok(released == 1)
    }
}

impl Drop for SchedulerLeadership {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._lock);
    }
}

fn scheduler_lock_path(coven_home: &Path) -> PathBuf {
    coven_home.join(SCHEDULER_LOCK_FILE)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::SchedulerLeadership;

    #[test]
    fn scheduler_leadership_is_exclusive_for_one_coven_home() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let first_conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let second_conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();

        let _first = SchedulerLeadership::acquire(home, &first_conn, now).unwrap();
        let error = SchedulerLeadership::acquire(home, &second_conn, now)
            .err()
            .expect("a second scheduler must not share local authority");

        assert!(
            error
                .to_string()
                .contains("automations scheduler leadership is already held"),
            "{error:#}"
        );
    }

    #[test]
    fn scheduler_restart_advances_the_durable_fence_generation() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();

        let first = SchedulerLeadership::acquire(home, &conn, now).unwrap();
        let first_fence = first.fence();
        assert_eq!(first_fence.generation(), 1);
        assert!(first_fence.is_current(&conn).unwrap());
        drop(first);

        let second =
            SchedulerLeadership::acquire(home, &conn, now + chrono::Duration::seconds(1)).unwrap();
        let second_fence = second.fence();

        assert_eq!(second_fence.generation(), 2);
        assert!(!first_fence.is_current(&conn).unwrap());
        assert!(second_fence.is_current(&conn).unwrap());
    }

    #[test]
    fn scheduler_release_only_clears_its_exact_generation() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();
        let mut leadership = SchedulerLeadership::acquire(home, &conn, now).unwrap();
        let fence = leadership.fence();

        assert!(leadership.release(&conn).unwrap());
        assert!(!fence.is_current(&conn).unwrap());

        drop(leadership);
        let mut successor =
            SchedulerLeadership::acquire(home, &conn, now + chrono::Duration::seconds(1)).unwrap();
        conn.execute(
            "UPDATE automation_scheduler_authority
             SET owner_id = 'new-owner', generation = generation + 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        assert!(!successor.release(&conn).unwrap());
        let owner: String = conn
            .query_row(
                "SELECT owner_id FROM automation_scheduler_authority WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "new-owner");
    }
}
