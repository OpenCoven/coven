// ── eval_loop.rs ─────────────────────────────────────────────────────────────
//
// Daemon-side implementation of the eval-loop skill.
//
// State is entirely filesystem-backed:
//   <familiar-workspace>/results.tsv          ← iteration history (read)
//   <familiar-workspace>/eval-loop/run.lock   ← in-progress lock (run trigger)
//   <familiar-workspace>/eval-loop/run.json   ← pending run spec (run trigger)
//
// The daemon never executes the loop itself — it:
//   1. Reads + aggregates the TSV for GET state
//   2. Writes a run.json spec + touches run.lock for POST /run
//      (the familiar's runtime watches for run.json and drives the iteration)
//
// Familiar workspace path is resolved from ~/.coven/familiars.toml via the
// `workspace` field, falling back to ~/.coven/familiars/<id>/.
//
// Author: Sage 🌿 · 2026-06-05

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── TSV schema ────────────────────────────────────────────────────────────────
//
// Col indices (0-based):
//   0  timestamp     ISO 8601
//   1  track         synthesis | prompt | memory
//   2  iteration     u32
//   3  change_summary
//   4  metric_before f64
//   5  metric_after  f64
//   6  delta         f64
//   7  outcome       ACCEPT | REVERT
//   8  branch        git branch name
//   9  proposer_reasoning  (may be empty)
//  10  failure_modes       (may be empty)

const RESULTS_TSV: &str = "results.tsv";
const EVAL_LOOP_DIR: &str = "eval-loop";
const RUN_SPEC_FILE: &str = "run.json";
const RUN_LOCK_FILE: &str = "run.lock";
const STALE_LOCK_AFTER_SECS: i64 = 60 * 60;

// ── DTOs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopIterationDto {
    pub id: String,
    pub timestamp: String,
    pub track: String,
    pub iteration: u32,
    pub change_summary: String,
    pub metric_before: f64,
    pub metric_after: f64,
    pub delta: f64,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalLoopStateDto {
    pub familiar_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    pub iterations: Vec<LoopIterationDto>,
    pub track_counts: TrackCounts,
    pub total_accepted: u32,
    pub total_reverted: u32,
    pub running: bool,
    pub lock: EvalLoopLockDto,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalLoopLockDto {
    pub locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub run_json_exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_updated_at: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TrackCounts {
    pub synthesis: u32,
    pub prompt: u32,
    pub memory: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSpec {
    pub run_id: String,
    pub familiar_id: String,
    pub track: String,
    pub requested_at: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Read and aggregate eval-loop state for a familiar.
///
/// Returns `Ok(None)` when the familiar workspace has no `results.tsv`
/// (skill not active yet). Returns structured state otherwise.
pub fn get_eval_loop_state(
    coven_home: &Path,
    familiar_id: &str,
) -> Result<Option<EvalLoopStateDto>> {
    let workspace = familiar_workspace(coven_home, familiar_id);
    let tsv_path = workspace.join(RESULTS_TSV);

    let raw = match fs::read_to_string(&tsv_path) {
        Ok(raw) => Some(raw),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", tsv_path.display()))
        }
    };

    let lock = eval_loop_lock(&workspace);
    if raw.is_none() && !lock.locked && !lock.run_json_exists {
        return Ok(None);
    }

    let iterations = raw.as_deref().map(parse_results_tsv).unwrap_or_default();
    let running = lock.locked;

    let mut track_counts = TrackCounts::default();
    let mut total_accepted: u32 = 0;
    let mut total_reverted: u32 = 0;
    let mut last_run: Option<String> = None;

    for it in &iterations {
        match it.track.as_str() {
            "synthesis" => track_counts.synthesis += 1,
            "prompt" => track_counts.prompt += 1,
            "memory" => track_counts.memory += 1,
            _ => {}
        }
        match it.outcome.as_str() {
            "ACCEPT" => total_accepted += 1,
            "REVERT" => total_reverted += 1,
            _ => {}
        }
        // The TSV is appended newest-last; last row's timestamp = most recent run.
        last_run = Some(it.timestamp.clone());
    }

    Ok(Some(EvalLoopStateDto {
        familiar_id: familiar_id.to_string(),
        last_run,
        iterations,
        track_counts,
        total_accepted,
        total_reverted,
        running,
        lock,
    }))
}

/// Write a run spec so the familiar's runtime can pick it up and execute
/// the iteration. The daemon does not run the loop directly.
///
/// Returns `Err` if a run is already in progress (lock file exists).
pub fn enqueue_run(coven_home: &Path, familiar_id: &str, track: &str) -> Result<RunSpec> {
    let track = validate_track(track)?;
    let workspace = familiar_workspace(coven_home, familiar_id);
    let eval_dir = workspace.join(EVAL_LOOP_DIR);

    fs::create_dir_all(&eval_dir)
        .with_context(|| format!("failed to create eval-loop dir at {}", eval_dir.display()))?;

    let lock_path = eval_dir.join(RUN_LOCK_FILE);
    if lock_path.exists() {
        anyhow::bail!("eval-loop run already in progress for familiar `{familiar_id}`");
    }

    let spec = RunSpec {
        run_id: Uuid::new_v4().to_string(),
        familiar_id: familiar_id.to_string(),
        track: track.to_string(),
        requested_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    };

    // Write spec first, then lock — the familiar watches for lock disappearance
    // to know a run completed. Atomic enough: both are small local writes.
    let spec_path = eval_dir.join(RUN_SPEC_FILE);
    let spec_json = serde_json::to_string_pretty(&spec).context("failed to serialize run spec")?;
    fs::write(&spec_path, &spec_json)
        .with_context(|| format!("failed to write run spec at {}", spec_path.display()))?;

    fs::write(&lock_path, &spec.run_id)
        .with_context(|| format!("failed to write run lock at {}", lock_path.display()))?;

    Ok(spec)
}

/// Clear the eval-loop run lock for a familiar.
///
/// Returns `Ok(false)` when no lock exists. Fresh locks require `force` so an
/// active familiar run is not interrupted by accident.
pub fn clear_eval_loop_lock(coven_home: &Path, familiar_id: &str, force: bool) -> Result<bool> {
    let workspace = familiar_workspace(coven_home, familiar_id);
    let eval_dir = workspace.join(EVAL_LOOP_DIR);
    let lock_path = eval_dir.join(RUN_LOCK_FILE);

    if !lock_path.exists() {
        return Ok(false);
    }

    let lock = eval_loop_lock(&workspace);
    if !force && !lock.stale {
        anyhow::bail!(
            "eval-loop run lock for familiar `{familiar_id}` is not stale; pass force=true to clear it"
        );
    }

    fs::remove_file(&lock_path)
        .with_context(|| format!("failed to remove run lock at {}", lock_path.display()))?;
    Ok(true)
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn familiar_workspace(coven_home: &Path, familiar_id: &str) -> PathBuf {
    // Prefer the explicit `workspace` path declared in familiars.toml when present.
    // Falls back to the conventional `~/.coven/familiars/<id>/` path.
    crate::cockpit_sources::read_familiars(coven_home)
        .ok()
        .and_then(|familiars| {
            familiars
                .into_iter()
                .find(|f| f.id == familiar_id)
                .and_then(|f| f.workspace)
        })
        .unwrap_or_else(|| coven_home.join("familiars").join(familiar_id))
}

fn eval_loop_lock(workspace: &Path) -> EvalLoopLockDto {
    let eval_dir = workspace.join(EVAL_LOOP_DIR);
    let lock_path = eval_dir.join(RUN_LOCK_FILE);
    let spec_path = eval_dir.join(RUN_SPEC_FILE);
    let locked = lock_path.exists();
    let run_json_exists = spec_path.exists();
    let run_id = if locked {
        fs::read_to_string(&lock_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let requested_at = read_run_requested_at(&spec_path);
    let lock_updated_at = lock_path
        .metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .map(|modified| {
            let dt: DateTime<Utc> = modified.into();
            dt.to_rfc3339_opts(SecondsFormat::Millis, true)
        });
    let stale = locked
        && (!run_json_exists
            || requested_at
                .as_deref()
                .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
                .map(|dt| {
                    Utc::now()
                        .signed_duration_since(dt.with_timezone(&Utc))
                        .num_seconds()
                        > STALE_LOCK_AFTER_SECS
                })
                .unwrap_or(false));

    EvalLoopLockDto {
        locked,
        run_id,
        run_json_exists,
        requested_at,
        lock_updated_at,
        stale,
    }
}

fn read_run_requested_at(spec_path: &Path) -> Option<String> {
    let raw = fs::read_to_string(spec_path).ok()?;
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| {
            value
                .get("requestedAt")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

fn validate_track(track: &str) -> Result<&str> {
    match track {
        "synthesis" | "prompt" | "memory" => Ok(track),
        other => anyhow::bail!("track must be `synthesis`, `prompt`, or `memory`, got `{other}`"),
    }
}

fn parse_results_tsv(raw: &str) -> Vec<LoopIterationDto> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        if cols.len() < 8 {
            continue;
        }
        // Skip header row: first col must parse as a timestamp, not "timestamp"
        if cols[0] == "timestamp" || !cols[0].contains('T') {
            continue;
        }
        let Ok(iteration) = cols[2].parse::<u32>() else {
            continue;
        };
        let metric_before = cols[4].parse::<f64>().unwrap_or(0.0);
        let metric_after = cols[5].parse::<f64>().unwrap_or(0.0);
        let delta = cols[6].parse::<f64>().unwrap_or(0.0);
        // Notes: combine proposer_reasoning (col 9) + failure_modes (col 10) if present
        let notes = {
            let reasoning = cols.get(9).copied().unwrap_or("").trim();
            let failures = cols.get(10).copied().unwrap_or("").trim();
            match (reasoning.is_empty(), failures.is_empty()) {
                (true, true) => None,
                (false, true) => Some(reasoning.to_string()),
                (true, false) => Some(format!("failures: {failures}")),
                (false, false) => Some(format!("{reasoning} | failures: {failures}")),
            }
        };
        out.push(LoopIterationDto {
            id: Uuid::new_v4().to_string(),
            timestamp: cols[0].to_string(),
            track: cols[1].to_string(),
            iteration,
            change_summary: cols[3].to_string(),
            metric_before,
            metric_after,
            delta,
            outcome: cols[7].to_string(),
            notes,
        });
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn tsv_line(
        ts: &str,
        track: &str,
        n: u32,
        summary: &str,
        before: f64,
        after: f64,
        delta: f64,
        outcome: &str,
    ) -> String {
        format!("{ts}\t{track}\t{n}\t{summary}\t{before}\t{after}\t{delta}\t{outcome}\tmain\t\t\n")
    }

    #[test]
    fn parse_results_tsv_skips_header_blank_and_comment_lines() {
        let raw = format!(
            "timestamp\ttrack\titeration\tchange_summary\tmetric_before\tmetric_after\tdelta\toutcome\tbranch\n\
            \n\
            # this is a comment\n\
            {}",
            tsv_line(
                "2026-06-05T10:00:00Z",
                "synthesis",
                1,
                "Tightened intro",
                0.60,
                0.68,
                0.08,
                "ACCEPT"
            )
        );
        let rows = parse_results_tsv(&raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].track, "synthesis");
        assert_eq!(rows[0].outcome, "ACCEPT");
        assert!((rows[0].delta - 0.08).abs() < 1e-9);
    }

    #[test]
    fn parse_results_tsv_handles_notes_columns() {
        let line = "2026-06-05T11:00:00Z\tprompt\t2\tAdded context clue\t0.55\t0.50\t-0.05\tREVERT\tmain\treasoning here\tbad drift\n";
        let rows = parse_results_tsv(line);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, "REVERT");
        let notes = rows[0].notes.as_deref().unwrap_or("");
        assert!(notes.contains("reasoning here"));
        assert!(notes.contains("bad drift"));
    }

    #[test]
    fn parse_results_tsv_empty_notes_when_absent() {
        let line = tsv_line(
            "2026-06-05T12:00:00Z",
            "memory",
            3,
            "Pruned redundant entry",
            0.70,
            0.74,
            0.04,
            "ACCEPT",
        );
        let rows = parse_results_tsv(&line);
        assert!(rows[0].notes.is_none());
    }

    #[test]
    fn get_eval_loop_state_returns_none_when_tsv_missing() {
        let temp = tempfile::tempdir().unwrap();
        let result = get_eval_loop_state(temp.path(), "sage").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_eval_loop_state_aggregates_correctly() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        fs::create_dir_all(&workspace).unwrap();
        let tsv = format!(
            "{}{}{}",
            tsv_line(
                "2026-06-05T10:00:00Z",
                "synthesis",
                1,
                "S1",
                0.60,
                0.68,
                0.08,
                "ACCEPT"
            ),
            tsv_line(
                "2026-06-05T11:00:00Z",
                "prompt",
                2,
                "P1",
                0.55,
                0.50,
                -0.05,
                "REVERT"
            ),
            tsv_line(
                "2026-06-05T12:00:00Z",
                "memory",
                3,
                "M1",
                0.70,
                0.74,
                0.04,
                "ACCEPT"
            ),
        );
        fs::write(workspace.join(RESULTS_TSV), tsv).unwrap();

        let state = get_eval_loop_state(temp.path(), "sage").unwrap().unwrap();
        assert_eq!(state.familiar_id, "sage");
        assert_eq!(state.total_accepted, 2);
        assert_eq!(state.total_reverted, 1);
        assert_eq!(state.track_counts.synthesis, 1);
        assert_eq!(state.track_counts.prompt, 1);
        assert_eq!(state.track_counts.memory, 1);
        assert_eq!(state.iterations.len(), 3);
        assert_eq!(state.last_run.as_deref(), Some("2026-06-05T12:00:00Z"));
        assert!(!state.running);
    }

    #[test]
    fn get_eval_loop_state_detects_running_lock() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        fs::create_dir_all(workspace.join(EVAL_LOOP_DIR)).unwrap();
        fs::write(workspace.join(RESULTS_TSV), "").unwrap();
        fs::write(workspace.join(EVAL_LOOP_DIR).join(RUN_LOCK_FILE), "lock-id").unwrap();

        let state = get_eval_loop_state(temp.path(), "sage").unwrap().unwrap();
        assert!(state.running);
    }

    #[test]
    fn get_eval_loop_state_reports_lock_without_results_tsv() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        let eval_dir = workspace.join(EVAL_LOOP_DIR);
        fs::create_dir_all(&eval_dir).unwrap();
        fs::write(eval_dir.join(RUN_LOCK_FILE), "first-run").unwrap();

        let state = get_eval_loop_state(temp.path(), "sage").unwrap().unwrap();

        assert!(state.running);
        assert!(state.iterations.is_empty());
        assert_eq!(state.lock.run_id.as_deref(), Some("first-run"));
    }

    #[test]
    fn get_eval_loop_state_reports_lock_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        let eval_dir = workspace.join(EVAL_LOOP_DIR);
        fs::create_dir_all(&eval_dir).unwrap();
        fs::write(workspace.join(RESULTS_TSV), "").unwrap();
        fs::write(eval_dir.join(RUN_SPEC_FILE), r#"{"runId":"run-123"}"#).unwrap();
        fs::write(eval_dir.join(RUN_LOCK_FILE), "run-123").unwrap();

        let state = get_eval_loop_state(temp.path(), "sage").unwrap().unwrap();

        assert!(state.lock.locked);
        assert_eq!(state.lock.run_id.as_deref(), Some("run-123"));
        assert!(state.lock.run_json_exists);
        assert!(state.lock.lock_updated_at.is_some());
        assert!(!state.lock.stale);
    }

    #[test]
    fn get_eval_loop_state_marks_old_lock_stale() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        let eval_dir = workspace.join(EVAL_LOOP_DIR);
        fs::create_dir_all(&eval_dir).unwrap();
        fs::write(workspace.join(RESULTS_TSV), "").unwrap();
        let requested_at =
            (Utc::now() - chrono::Duration::hours(2)).to_rfc3339_opts(SecondsFormat::Millis, true);
        fs::write(
            eval_dir.join(RUN_SPEC_FILE),
            format!(r#"{{"runId":"old-run","requestedAt":"{requested_at}"}}"#),
        )
        .unwrap();
        let lock_path = eval_dir.join(RUN_LOCK_FILE);
        fs::write(&lock_path, "old-run").unwrap();

        let state = get_eval_loop_state(temp.path(), "sage").unwrap().unwrap();

        assert!(state.lock.locked);
        assert!(state.lock.stale);
    }

    #[test]
    fn enqueue_run_writes_spec_and_lock() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        fs::create_dir_all(&workspace).unwrap();

        let spec = enqueue_run(temp.path(), "sage", "synthesis").unwrap();
        assert_eq!(spec.familiar_id, "sage");
        assert_eq!(spec.track, "synthesis");

        let lock_path = workspace.join(EVAL_LOOP_DIR).join(RUN_LOCK_FILE);
        assert!(lock_path.exists(), "lock file must exist after enqueue");

        let spec_path = workspace.join(EVAL_LOOP_DIR).join(RUN_SPEC_FILE);
        let spec_raw = fs::read_to_string(&spec_path).unwrap();
        let parsed: RunSpec = serde_json::from_str(&spec_raw).unwrap();
        assert_eq!(parsed.run_id, spec.run_id);
        assert_eq!(parsed.track, "synthesis");
    }

    #[test]
    fn enqueue_run_rejects_unknown_track() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        fs::create_dir_all(&workspace).unwrap();

        let err = enqueue_run(temp.path(), "sage", "bogus").unwrap_err();
        assert!(err.to_string().contains("track must be"));
    }

    #[test]
    fn enqueue_run_errors_when_already_running() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        let eval_dir = workspace.join(EVAL_LOOP_DIR);
        fs::create_dir_all(&eval_dir).unwrap();
        fs::write(eval_dir.join(RUN_LOCK_FILE), "prior-lock-id").unwrap();

        let err = enqueue_run(temp.path(), "sage", "prompt").unwrap_err();
        assert!(err.to_string().contains("already in progress"));
    }

    #[test]
    fn clear_eval_loop_lock_removes_stale_lock() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        let eval_dir = workspace.join(EVAL_LOOP_DIR);
        fs::create_dir_all(&eval_dir).unwrap();
        let lock_path = eval_dir.join(RUN_LOCK_FILE);
        let requested_at =
            (Utc::now() - chrono::Duration::hours(2)).to_rfc3339_opts(SecondsFormat::Millis, true);
        fs::write(
            eval_dir.join(RUN_SPEC_FILE),
            format!(r#"{{"runId":"old-run","requestedAt":"{requested_at}"}}"#),
        )
        .unwrap();
        fs::write(&lock_path, "old-run").unwrap();

        let cleared = clear_eval_loop_lock(temp.path(), "sage", false).unwrap();

        assert!(cleared);
        assert!(!lock_path.exists());
    }

    #[test]
    fn clear_eval_loop_lock_requires_force_for_fresh_lock() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        let eval_dir = workspace.join(EVAL_LOOP_DIR);
        fs::create_dir_all(&eval_dir).unwrap();
        let lock_path = eval_dir.join(RUN_LOCK_FILE);
        fs::write(eval_dir.join(RUN_SPEC_FILE), r#"{"runId":"fresh-run"}"#).unwrap();
        fs::write(&lock_path, "fresh-run").unwrap();

        let err = clear_eval_loop_lock(temp.path(), "sage", false).unwrap_err();

        assert!(err.to_string().contains("not stale"));
        assert!(lock_path.exists());
    }

    #[test]
    fn clear_eval_loop_lock_returns_false_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("familiars").join("sage");
        fs::create_dir_all(workspace.join(EVAL_LOOP_DIR)).unwrap();

        let cleared = clear_eval_loop_lock(temp.path(), "sage", false).unwrap();

        assert!(!cleared);
    }

    #[test]
    fn validate_track_accepts_known_values() {
        assert!(validate_track("synthesis").is_ok());
        assert!(validate_track("prompt").is_ok());
        assert!(validate_track("memory").is_ok());
    }

    #[test]
    fn validate_track_rejects_unknown() {
        assert!(validate_track("").is_err());
        assert!(validate_track("harness").is_err());
        assert!(validate_track("SYNTHESIS").is_err()); // case-sensitive
    }
}

#[cfg(test)]
mod workspace_resolution_tests {
    use super::*;
    use std::fs;

    #[test]
    fn uses_toml_workspace_field_when_set() {
        let temp = tempfile::tempdir().unwrap();
        let custom_ws = temp.path().join("custom-ws");
        fs::create_dir_all(&custom_ws).unwrap();

        let toml = format!(
            "[[familiar]]\nid = \"sage\"\ndisplay_name = \"Sage\"\nrole = \"Research\"\ndescription = \"Reads.\"\nworkspace = '{}'\n",
            custom_ws.display()
        );
        fs::write(temp.path().join("familiars.toml"), toml).unwrap();

        let resolved = familiar_workspace(temp.path(), "sage");
        assert_eq!(resolved, custom_ws, "should use TOML workspace path");
    }

    #[test]
    fn falls_back_to_convention_when_no_workspace_field() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("familiars.toml"),
            "[[familiar]]\nid = \"sage\"\ndisplay_name = \"Sage\"\nrole = \"R\"\ndescription = \"D\"\n",
        ).unwrap();

        let resolved = familiar_workspace(temp.path(), "sage");
        assert_eq!(resolved, temp.path().join("familiars").join("sage"));
    }

    #[test]
    fn falls_back_to_convention_when_toml_missing() {
        let temp = tempfile::tempdir().unwrap();
        let resolved = familiar_workspace(temp.path(), "sage");
        assert_eq!(resolved, temp.path().join("familiars").join("sage"));
    }

    #[test]
    fn unknown_familiar_uses_convention_path() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("familiars.toml"),
            "[[familiar]]\nid = \"nova\"\ndisplay_name = \"Nova\"\nrole = \"Queen\"\ndescription = \"Orchestrates.\"\n",
        ).unwrap();

        // sage is not in the TOML — should still get a sensible conventional path
        let resolved = familiar_workspace(temp.path(), "sage");
        assert_eq!(resolved, temp.path().join("familiars").join("sage"));
    }
}
