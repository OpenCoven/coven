//! Legacy Codex automation import (coven#816).
//!
//! Reads `~/.codex/automations/<id>/automation.toml` definitions and imports
//! them as Coven routines. The import is NON-DESTRUCTIVE: source files are
//! never modified, moved, or deleted, and every imported routine is created
//! PAUSED. Definitions whose schedules use vocabulary the Coven scheduler
//! does not support are reported and skipped — never silently downgraded.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde::Deserialize;

use super::definition::{RoutineDefinition, RoutineStatus, RoutineTimezone};
use super::store::insert_definition;

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)] // fields read selectively during mapping
struct CodexAutomationToml {
    id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    rrule: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
    pub failures: Vec<String>,
}

fn codex_automations_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".codex").join("automations")
}

/// Normalizes a Codex RRULE (`RRULE:FREQ=WEEKLY;BYHOUR=21;BYMINUTE=0;BYDAY=…`)
/// into the Coven vocabulary. Returns `None` when the schedule cannot be
/// represented faithfully.
fn normalize_codex_rrule(raw: &str) -> Option<String> {
    let mut body = raw.trim().to_string();
    if let Some(stripped) = body.strip_prefix("RRULE:") {
        body = stripped.trim().to_string();
    }
    let mut kept: Vec<String> = Vec::new();
    for part in body.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part.split_once('=')?;
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" | "BYHOUR" | "BYDAY" => kept.push(part.to_string()),
            // Codex emits BYMINUTE=0 on every cron; minute-zero is the Coven
            // default, so dropping it preserves the schedule exactly.
            "BYMINUTE" if value.trim() == "0" => {}
            "INTERVAL" if value.trim() == "1" => {}
            _ => return None,
        }
    }
    let normalized = kept.join(";");
    if normalized.is_empty() {
        return None;
    }
    // The normalized schedule must still satisfy the scheduler's vocabulary
    // gate; import refuses rather than silently downgrading.
    super::rrule::parse_rrule(&normalized).ok()?;
    Some(normalized)
}

/// Imports every parseable definition under `~/.codex/automations`. Returns
/// a report of imported ids, skipped ids (unsupported schedule or invalid
/// shape), and per-id failures. Source files are never touched.
pub fn import_legacy_codex_automations(conn: &Connection) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    let root = codex_automations_dir();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error).with_context(|| format!("reading {}", root.display())),
    };

    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", root.display()))?;
        let toml_path = entry.path().join("automation.toml");
        let raw = match fs::read_to_string(&toml_path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                report.failures.push(format!(
                    "{}: could not read automation.toml: {error}",
                    entry.path().display()
                ));
                continue;
            }
        };
        let parsed: CodexAutomationToml = match toml::from_str(&raw) {
            Ok(parsed) => parsed,
            Err(error) => {
                report.failures.push(format!(
                    "{}: could not parse automation.toml: {error}",
                    entry.path().display()
                ));
                continue;
            }
        };

        let id = parsed
            .id
            .clone()
            .or_else(|| entry.file_name().to_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| "unknown".to_string());
        let Some(raw_rrule) = parsed.rrule.as_deref() else {
            report.skipped.push(format!("{id}: no schedule"));
            continue;
        };
        let Some(rrule) = normalize_codex_rrule(raw_rrule) else {
            report
                .skipped
                .push(format!("{id}: unsupported schedule `{raw_rrule}`"));
            continue;
        };
        let Some(prompt) = parsed
            .prompt
            .clone()
            .filter(|prompt| !prompt.trim().is_empty())
        else {
            report.skipped.push(format!("{id}: no prompt"));
            continue;
        };

        let definition = RoutineDefinition {
            schema_version: super::definition::AUTOMATION_SCHEMA_VERSION,
            id: id.clone(),
            name: parsed.name.clone().unwrap_or_else(|| id.clone()),
            // Imported routines are always paused: nothing runs until a human
            // reviews the migrated definition (coven#816 acceptance).
            status: RoutineStatus::Paused,
            rrule,
            timezone: RoutineTimezone::Local,
            misfire: super::definition::RoutineMisfire::Latest,
            overlap: super::definition::RoutineOverlap::Forbid,
            timeout_minutes: 60,
            retry: super::definition::RoutineRetryPolicy::default(),
            runtime: "coven-code".to_string(),
            familiar_id: None,
            cwd: None,
            output_target: None,
            prompt,
            model: None,
            tags: Vec::new(),
        };

        if let Err(error) = definition.validate() {
            report.skipped.push(format!("{id}: {error}"));
            continue;
        }

        let imported = (|| {
            let transaction =
                rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                    .context("failed to begin legacy automation import transaction")?;
            insert_definition(&transaction, &definition)?;
            let record = super::store::get_definition(&transaction, &definition.id)?
                .context("imported automation definition is missing")?;
            super::contract::events::append_imported_definition_event(
                &transaction,
                super::contract::events::ImportedDefinitionEventInput {
                    automation_id: &record.id,
                    revision: record.revision,
                    definition_digest: record.definition_digest.as_deref(),
                    lifecycle_state: &record.lifecycle_state,
                    imported_from: "codex-automation-toml",
                    recorded_at: &record.updated_at,
                    observed_at: &record.updated_at,
                },
            )?;
            transaction
                .commit()
                .context("failed to commit legacy automation import")?;
            Ok::<_, anyhow::Error>(())
        })();
        match imported {
            Ok(()) => report.imported.push(id),
            Err(error) => report.failures.push(format!("{id}: {error:#}")),
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codex_rrule_into_coven_vocabulary() {
        let normalized = normalize_codex_rrule(
            "RRULE:FREQ=WEEKLY;BYHOUR=21;BYMINUTE=0;BYDAY=SU,MO,TU,WE,TH,FR,SA",
        )
        .unwrap();
        assert_eq!(
            normalized,
            "FREQ=WEEKLY;BYHOUR=21;BYDAY=SU,MO,TU,WE,TH,FR,SA"
        );
    }

    #[test]
    fn drops_interval_one() {
        let normalized = normalize_codex_rrule("FREQ=DAILY;BYHOUR=9;INTERVAL=1").unwrap();
        assert_eq!(normalized, "FREQ=DAILY;BYHOUR=9");
    }

    #[test]
    fn refuses_unsupported_vocabulary() {
        assert!(normalize_codex_rrule("FREQ=HOURLY").is_none());
        assert!(normalize_codex_rrule("FREQ=DAILY;BYMINUTE=30").is_none());
        assert!(normalize_codex_rrule("FREQ=DAILY;INTERVAL=2").is_none());
    }

    #[test]
    fn import_is_paused_and_reads_the_codex_dir() {
        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&store_path).unwrap();
        let conn = crate::store::open_store(&store_path).unwrap();

        // Point HOME at a scratch dir containing a legacy definition.
        let home = temp.path().join("home");
        let def_dir = home.join(".codex/automations/legacy-daily");
        std::fs::create_dir_all(&def_dir).unwrap();
        std::fs::write(
            def_dir.join("automation.toml"),
            r#"version = 1
id = "legacy-daily"
name = "Legacy Daily"
rrule = "RRULE:FREQ=DAILY;BYHOUR=9;BYMINUTE=0"
prompt = "Do the legacy thing."
"#,
        )
        .unwrap();

        // The import helper reads HOME, so run it in a scoped thread with the
        // env var pinned. Rust tests share the process env, so serialize via a
        // mutex and restore afterwards.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        let old_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", &home);
        }
        let report = import_legacy_codex_automations(&conn).unwrap();
        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert_eq!(report.imported, vec!["legacy-daily"]);
        let record = super::super::store::get_definition(&conn, "legacy-daily")
            .unwrap()
            .unwrap();
        assert_eq!(record.status, "PAUSED");
        let stored: serde_json::Value = serde_json::from_str(&record.definition_json).unwrap();
        assert_ne!(stored["timezone"], "local");
        let event: String = conn
            .query_row(
                "SELECT event_json FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'legacy-daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let event: serde_json::Value = serde_json::from_str(&event).unwrap();
        assert_eq!(event["kind"], "definition.imported");
        assert_eq!(event["payload"]["importedFrom"], "codex-automation-toml");
    }
}
