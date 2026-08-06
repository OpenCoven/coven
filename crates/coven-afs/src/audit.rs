//! Insert-only tool-call audit log (SPEC tool-calls section).
//!
//! Records MUST NOT be updated or deleted; this module deliberately exposes
//! no mutation beyond the single completion-time insert.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
            .query_map(params, |r| {
                Ok(ToolCall {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parameters: r.get(2)?,
                    result: r.get(3)?,
                    error: r.get(4)?,
                    started_at: r.get(5)?,
                    completed_at: r.get(6)?,
                    duration_ms: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
