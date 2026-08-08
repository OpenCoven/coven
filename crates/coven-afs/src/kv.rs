//! JSON key-value store (SPEC key-value section).

use rusqlite::OptionalExtension;
use serde_json::Value;

use crate::fs::AgentFs;
use crate::{Error, Result};

impl AgentFs {
    /// Upsert a JSON value under `key` (SPEC set: refreshes `updated_at`).
    pub fn kv_set(&self, key: &str, value: &Value) -> Result<()> {
        if self.is_read_only() {
            return Err(Error::ReadOnly);
        }
        self.conn.execute(
            "INSERT INTO kv_store (key, value, updated_at)
             VALUES (?1, ?2, unixepoch())
             ON CONFLICT(key) DO UPDATE SET
               value = excluded.value,
               updated_at = unixepoch()",
            rusqlite::params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }

    /// Get the JSON value stored under `key`.
    pub fn kv_get(&self, key: &str) -> Result<Option<Value>> {
        let raw: Option<String> = self
            .conn
            .query_row("SELECT value FROM kv_store WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    /// Delete `key`. Returns whether a row was removed.
    pub fn kv_delete(&self, key: &str) -> Result<bool> {
        if self.is_read_only() {
            return Err(Error::ReadOnly);
        }
        let n = self
            .conn
            .execute("DELETE FROM kv_store WHERE key = ?1", [key])?;
        Ok(n > 0)
    }

    /// All keys, sorted ascending.
    pub fn kv_keys(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key FROM kv_store ORDER BY key ASC")?;
        let keys = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(keys)
    }
}
