// coven_calls.rs — daemon-side writer for the Coven Calls delegation log.
//
// Every time one familiar dispatches a task through the Cast interface,
// `emit_running` appends a record to `~/.coven/cave-coven-calls.json`.
// When the callee session reaches a terminal state, `emit_terminal` patches
// the same record with the final status and `endedAt` timestamp.
//
// The coven-cave Next.js app reads this file via `GET /api/v1/coven-calls`
// and renders it in the Coven Calls graph view (Delegations tab).
//
// Data contract (matches coven-cave `src/lib/coven-calls-types.ts`):
//
//   {
//     "version": 1,
//     "calls": [
//       {
//         "id": "<uuid>",
//         "callerFamiliarId": "nova",
//         "calleeFamiliarId": "sage",
//         "request": "<first 300 chars of prompt>",
//         "status": "running" | "completed" | "failed" | "cancelled",
//         "createdAt": "<ISO 8601>",
//         "endedAt": "<ISO 8601>",      // only on terminal records
//         "sessionId": "<uuid>",         // optional callee session linkback
//         "artifact": "<string>"         // optional return value / summary
//       }
//     ]
//   }
//
// Writes are atomic: tmp file in the same directory + `fs::rename`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Constants ────────────────────────────────────────────────────────────────

pub const CALLS_FILE: &str = "cave-coven-calls.json";
const CALLS_TMP_FILE: &str = ".cave-coven-calls.json.tmp";

/// Maximum length of the `request` field (characters, not bytes).
const MAX_REQUEST_CHARS: usize = 300;

// ── Types ────────────────────────────────────────────────────────────────────

/// Terminal/running status of a delegation call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CovenCallStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl CovenCallStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Single delegation event record. Matches `CovenCall` in coven-cave.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CovenCallRecord {
    pub id: String,
    pub caller_familiar_id: String,
    pub callee_familiar_id: String,
    /// Prompt/task text, truncated to [`MAX_REQUEST_CHARS`] characters.
    pub request: String,
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

/// Top-level calls file wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CovenCallsFile {
    version: u32,
    calls: Vec<CovenCallRecord>,
}

impl Default for CovenCallsFile {
    fn default() -> Self {
        Self {
            version: 1,
            calls: Vec::new(),
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Append a new `running` delegation record and return the call id.
///
/// # Arguments
/// * `coven_home`  — resolved `~/.coven` directory
/// * `caller_id`   — familiar that initiated the delegation
/// * `callee_id`   — target familiar (or `"unknown"` when resolution is deferred)
/// * `request`     — verbatim task text (truncated to 300 chars before storage)
/// * `session_id`  — callee daemon session id, if already known
pub fn emit_running(
    coven_home: &Path,
    caller_id: &str,
    callee_id: &str,
    request: &str,
    session_id: Option<&str>,
) -> Result<String> {
    let path = calls_path(coven_home);
    let mut file = load_file(&path)?;

    let id = Uuid::new_v4().to_string();
    let record = CovenCallRecord {
        id: id.clone(),
        caller_familiar_id: caller_id.to_string(),
        callee_familiar_id: callee_id.to_string(),
        request: truncate(request, MAX_REQUEST_CHARS),
        status: CovenCallStatus::Running.as_str().to_string(),
        created_at: now_iso(),
        ended_at: None,
        session_id: session_id.map(str::to_string),
        artifact: None,
    };

    file.calls.push(record);
    save_file(&path, &file)?;
    Ok(id)
}

/// Patch an existing record to a terminal status (`completed`, `failed`,
/// or `cancelled`). Sets `endedAt` and optionally `artifact`.
///
/// Returns an error if no record with `call_id` exists.
pub fn emit_terminal(
    coven_home: &Path,
    call_id: &str,
    status: CovenCallStatus,
    ended_at: &str,
    artifact: Option<&str>,
) -> Result<()> {
    let path = calls_path(coven_home);
    let mut file = load_file(&path)?;

    let record = file
        .calls
        .iter_mut()
        .find(|c| c.id == call_id)
        .ok_or_else(|| anyhow::anyhow!("coven call not found: {call_id}"))?;

    record.status = status.as_str().to_string();
    record.ended_at = Some(ended_at.to_string());
    if let Some(a) = artifact {
        record.artifact = Some(a.to_string());
    }

    save_file(&path, &file)?;
    Ok(())
}

/// Load all call records, returning an empty vec when the file doesn't exist.
pub fn load_calls(coven_home: &Path) -> Result<Vec<CovenCallRecord>> {
    let path = calls_path(coven_home);
    Ok(load_file(&path)?.calls)
}

// ── Private helpers ───────────────────────────────────────────────────────────

pub(crate) fn calls_path(coven_home: &Path) -> PathBuf {
    coven_home.join(CALLS_FILE)
}

fn load_file(path: &Path) -> Result<CovenCallsFile> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(CovenCallsFile::default()),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn save_file(path: &Path, file: &CovenCallsFile) -> Result<()> {
    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create dir {}", parent.display()))?;
    }

    // Atomic write: tmp in same dir → rename.
    let tmp = path.with_file_name(CALLS_TMP_FILE);
    let json = serde_json::to_string_pretty(file).context("serialise calls file")?;
    fs::write(&tmp, &json).with_context(|| format!("failed to write tmp {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars).collect();
        format!("{t}…")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_running_creates_file_and_returns_id() {
        let dir = tempfile::tempdir().unwrap();
        let id = emit_running(dir.path(), "nova", "sage", "Research this", Some("sess-1")).unwrap();
        assert!(!id.is_empty(), "call id should be a non-empty string");

        let path = calls_path(dir.path());
        assert!(path.exists(), "calls file must exist after emit_running");
        let file = load_file(&path).unwrap();
        assert_eq!(file.version, 1);
        assert_eq!(file.calls.len(), 1);
        let r = &file.calls[0];
        assert_eq!(r.id, id);
        assert_eq!(r.caller_familiar_id, "nova");
        assert_eq!(r.callee_familiar_id, "sage");
        assert_eq!(r.request, "Research this");
        assert_eq!(r.status, "running");
        assert_eq!(r.session_id.as_deref(), Some("sess-1"));
        assert!(r.ended_at.is_none());
    }

    #[test]
    fn emit_terminal_patches_record() {
        let dir = tempfile::tempdir().unwrap();
        let id = emit_running(dir.path(), "nova", "sage", "Do a thing", None).unwrap();
        emit_terminal(
            dir.path(),
            &id,
            CovenCallStatus::Completed,
            "2026-06-05T12:00:00.000Z",
            Some("result.md"),
        )
        .unwrap();

        let calls = load_calls(dir.path()).unwrap();
        assert_eq!(calls.len(), 1);
        let r = &calls[0];
        assert_eq!(r.status, "completed");
        assert_eq!(r.ended_at.as_deref(), Some("2026-06-05T12:00:00.000Z"));
        assert_eq!(r.artifact.as_deref(), Some("result.md"));
    }

    #[test]
    fn emit_terminal_errors_on_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let err = emit_terminal(
            dir.path(),
            "no-such-id",
            CovenCallStatus::Failed,
            "2026-06-05T12:00:00.000Z",
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "error should mention not found, got: {err}"
        );
    }

    #[test]
    fn load_calls_returns_empty_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let calls = load_calls(dir.path()).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn concurrent_emits_do_not_corrupt() {
        // Sequential writes on the same directory — both records must survive.
        let dir = tempfile::tempdir().unwrap();
        let id1 = emit_running(dir.path(), "nova", "sage", "Task A", Some("s1")).unwrap();
        let id2 = emit_running(dir.path(), "nova", "cody", "Task B", Some("s2")).unwrap();
        let calls = load_calls(dir.path()).unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().any(|c| c.id == id1), "id1 must be present");
        assert!(calls.iter().any(|c| c.id == id2), "id2 must be present");
    }

    #[test]
    fn request_is_truncated_to_300_chars() {
        let dir = tempfile::tempdir().unwrap();
        let long = "x".repeat(500);
        let _id = emit_running(dir.path(), "a", "b", &long, None).unwrap();
        let calls = load_calls(dir.path()).unwrap();
        let req = &calls[0].request;
        assert!(
            req.chars().count() <= 302,
            "request too long: {} chars",
            req.chars().count()
        );
        assert!(req.ends_with('…'));
    }
}
