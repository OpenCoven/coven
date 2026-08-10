//! Insert-only tool-call audit log (SPEC tool-calls section).
//!
//! Records MUST NOT be updated or deleted; this module deliberately exposes
//! no mutation beyond the single completion-time insert.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rusqlite::OptionalExtension;

use crate::fs::AgentFs;
use crate::{Error, Result};

/// One recorded tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: i64,
    pub name: String,
    pub parameters: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub started_at: i64,
    pub completed_at: i64,
    pub duration_ms: i64,
}

impl AgentFs {
    /// Record a completed tool call. Exactly one of `result` / `error` must
    /// be provided (SPEC mutual-exclusion rule); `duration_ms` is computed as
    /// `(completed_at - started_at) * 1000`.
    pub fn record_tool_call(
        &self,
        name: &str,
        parameters: Option<&Value>,
        result: Option<&Value>,
        error: Option<&str>,
        started_at: i64,
        completed_at: i64,
    ) -> Result<i64> {
        if self.is_read_only() {
            return Err(Error::ReadOnly);
        }
        if result.is_some() == error.is_some() {
            return Err(Error::InvalidArgument(
                "exactly one of result or error must be set".into(),
            ));
        }
        if completed_at < started_at {
            return Err(Error::InvalidArgument(
                "completed_at must be >= started_at".into(),
            ));
        }
        let params_json = parameters.map(serde_json::to_string).transpose()?;
        let result_json = result.map(serde_json::to_string).transpose()?;
        let duration_ms = (completed_at - started_at) * 1000;
        let id: i64 = self.conn.query_row(
            "INSERT INTO tool_calls (name, parameters, result, error,
                                     started_at, completed_at, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             RETURNING id",
            rusqlite::params![
                name,
                params_json,
                result_json,
                error,
                started_at,
                completed_at,
                duration_ms
            ],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// Look up one tool call by its stable row id.
    pub fn tool_call(&self, id: i64) -> Result<Option<ToolCall>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, parameters, result, error,
                        started_at, completed_at, duration_ms
                 FROM tool_calls WHERE id = ?1",
                [id],
                tool_call_from_row,
            )
            .optional()?)
    }

    /// Tool calls with a given name, most recent first.
    pub fn tool_calls_by_name(&self, name: &str) -> Result<Vec<ToolCall>> {
        self.query_tool_calls(
            "SELECT id, name, parameters, result, error, started_at, completed_at, duration_ms
             FROM tool_calls WHERE name = ?1 ORDER BY started_at DESC",
            rusqlite::params![name],
        )
    }

    /// Tool calls started strictly after `since` (Unix seconds), most recent
    /// first.
    pub fn recent_tool_calls(&self, since: i64) -> Result<Vec<ToolCall>> {
        self.query_tool_calls(
            "SELECT id, name, parameters, result, error, started_at, completed_at, duration_ms
             FROM tool_calls WHERE started_at > ?1 ORDER BY started_at DESC",
            rusqlite::params![since],
        )
    }

    fn query_tool_calls(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::types::ToSql],
    ) -> Result<Vec<ToolCall>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(params, tool_call_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn tool_call_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ToolCall> {
    Ok(ToolCall {
        id: row.get(0)?,
        name: row.get(1)?,
        parameters: row.get(2)?,
        result: row.get(3)?,
        error: row.get(4)?,
        started_at: row.get(5)?,
        completed_at: row.get(6)?,
        duration_ms: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn looks_up_tool_call_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let fs = AgentFs::create(dir.path().join("afs.db")).unwrap();
        let id = fs
            .record_tool_call(
                "write_file",
                Some(&json!({ "path": "/src/main.rs" })),
                Some(&json!({ "bytes": 7 })),
                None,
                10,
                12,
            )
            .unwrap();

        let call = fs.tool_call(id).unwrap().unwrap();
        assert_eq!(call.id, id);
        assert_eq!(call.name, "write_file");
        assert_eq!(
            call.parameters.as_deref(),
            Some(r#"{"path":"/src/main.rs"}"#)
        );
        assert_eq!(call.result.as_deref(), Some(r#"{"bytes":7}"#));
        assert_eq!(call.duration_ms, 2_000);
        assert!(fs.tool_call(id + 1).unwrap().is_none());
    }
}
