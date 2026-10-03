//! Legacy Codex automation import (coven#816).
//!
//! Reads `~/.codex/automations/<id>/automation.toml` definitions and imports
//! them as Coven routines. The import is NON-DESTRUCTIVE: source files are
//! never modified, moved, or deleted, and every imported routine is created
//! PAUSED. Definitions whose schedules use vocabulary the Coven scheduler
//! does not support are reported and skipped — never silently downgraded.

use std::fs;
use std::path::{Path, PathBuf};

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

#[cfg(test)]
thread_local! {
    static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Runs `body` with this thread's Codex automations directory at `root`, so
/// tests need not mutate the process-wide `HOME`.
#[cfg(test)]
pub(crate) fn with_test_root<T>(root: &Path, body: impl FnOnce() -> T) -> T {
    TEST_ROOT.with(|cell| *cell.borrow_mut() = Some(root.to_path_buf()));
    let result = body();
    TEST_ROOT.with(|cell| *cell.borrow_mut() = None);
    result
}

fn codex_automations_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = TEST_ROOT.with(|cell| cell.borrow().clone()) {
        return root;
    }
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

/// What an import of `root` would create, read from the filesystem only.
struct ImportPlan {
    candidates: Vec<RoutineDefinition>,
    report: ImportReport,
}

/// Reads every definition under `root` into validated, PAUSED candidates,
/// recording skipped and failed entries. Neither the store nor the source
/// files are touched.
fn plan_codex_import(root: &Path) -> Result<ImportPlan> {
    let mut report = ImportReport::default();
    let mut candidates = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ImportPlan { candidates, report })
        }
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
        candidates.push(definition);
    }

    Ok(ImportPlan { candidates, report })
}

/// Imports every parseable definition under `~/.codex/automations`. Returns
/// a report of imported ids, skipped ids (unsupported schedule or invalid
/// shape), and per-id failures. Source files are never touched.
pub fn import_legacy_codex_automations(conn: &Connection) -> Result<ImportReport> {
    let ImportPlan {
        candidates,
        mut report,
    } = plan_codex_import(&codex_automations_dir())?;
    for definition in candidates {
        let id = definition.id.clone();
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

/// `legacy.import.v1`: imports into the `draft` lifecycle state as v1-managed
/// rows, inside the caller's transaction with one savepoint per definition.
/// An id already in the store, tombstoned or not, is skipped. With
/// `dry_run`, reports the same outcome without writing anything.
pub fn import_codex_as_draft(conn: &Connection, dry_run: bool) -> Result<ImportReport> {
    let ImportPlan {
        candidates,
        mut report,
    } = plan_codex_import(&codex_automations_dir())?;
    // Ids this run has already imported (or, dry, would have): a second
    // source file with the same id is skipped either way.
    let mut claimed = std::collections::HashSet::new();
    for definition in candidates {
        let id = definition.id.clone();
        if !claimed.insert(id.clone())
            || super::store::get_definition_with_tombstone(conn, &id, true)?.is_some()
        {
            report.skipped.push(format!("{id}: already exists"));
            continue;
        }
        if dry_run {
            report.imported.push(id);
            continue;
        }
        conn.execute_batch("SAVEPOINT legacy_import_definition")
            .context("failed to open legacy import savepoint")?;
        let imported = (|| {
            insert_definition(conn, &definition)?;
            conn.execute(
                "UPDATE automation_definitions
                 SET lifecycle_state = 'draft', authority_version = 1
                 WHERE id = ?1",
                [&id],
            )
            .context("failed to mark imported definition as draft")?;
            let record = super::store::get_definition(conn, &id)?
                .context("imported automation definition is missing")?;
            super::contract::events::append_imported_definition_event(
                conn,
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
            Ok::<_, anyhow::Error>(())
        })();
        match imported {
            Ok(()) => {
                conn.execute_batch("RELEASE legacy_import_definition")
                    .context("failed to release legacy import savepoint")?;
                report.imported.push(id);
            }
            Err(error) => {
                conn.execute_batch(
                    "ROLLBACK TO legacy_import_definition; RELEASE legacy_import_definition",
                )
                .context("failed to roll back legacy import savepoint")?;
                report.failures.push(format!("{id}: {error:#}"));
            }
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

    fn write_codex_automation(root: &Path, id: &str, rrule: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("automation.toml"),
            format!("version = 1\nid = \"{id}\"\nname = \"{id}\"\nrrule = \"{rrule}\"\nprompt = \"Do {id}.\"\n"),
        )
        .unwrap();
    }

    fn draft_import(
        conn: &Connection,
        root: &Path,
        key: &str,
        dry_run: bool,
    ) -> super::super::command_adoption::DefinitionCommandResponse {
        with_test_root(root, || {
            super::super::command_adoption::execute_definition_command(
                conn,
                key,
                super::super::command_adoption::DefinitionCommand::LegacyImport { dry_run },
                "2026-09-28T09:00:00.000Z",
                super::super::owner_grants::CommandAuthority::OwnerLocal,
            )
            .unwrap()
        })
    }

    fn definition_rows(conn: &Connection) -> Vec<(String, String, String, i64)> {
        conn.prepare(
            "SELECT id, status, lifecycle_state, authority_version
             FROM automation_definitions ORDER BY id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    fn event_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM automation_events", [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn v1_import_lands_in_draft_and_dry_run_writes_nothing() {
        use super::super::command_adoption::DefinitionCommandOutcome;
        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&store_path).unwrap();
        let conn = crate::store::open_store(&store_path).unwrap();
        let root = temp.path().join("automations");
        write_codex_automation(&root, "nightly", "RRULE:FREQ=DAILY;BYHOUR=2;BYMINUTE=0");
        write_codex_automation(&root, "minutely", "RRULE:FREQ=MINUTELY");
        // A second source directory declaring the same id.
        let twin = root.join("nightly-copy");
        std::fs::create_dir_all(&twin).unwrap();
        std::fs::copy(
            root.join("nightly/automation.toml"),
            twin.join("automation.toml"),
        )
        .unwrap();

        let dry = draft_import(&conn, &root, "adopt:import:dry", true);
        assert_eq!(dry.outcome, DefinitionCommandOutcome::Committed);
        let result = dry.result.unwrap();
        assert_eq!(result["dryRun"], true);
        assert_eq!(result["imported"], serde_json::json!(["nightly"]));
        let mut predicted = result["skipped"].as_array().unwrap().clone();
        assert_eq!(predicted.len(), 2);
        assert!(predicted
            .iter()
            .any(|skip| skip == "nightly: already exists"));
        assert!(definition_rows(&conn).is_empty());
        assert_eq!(event_count(&conn), 0);

        let imported = draft_import(&conn, &root, "adopt:import:real", false);
        assert_eq!(imported.outcome, DefinitionCommandOutcome::Committed);
        let real = imported.result.unwrap();
        // The dry run predicted exactly this outcome.
        assert_eq!(real["imported"], serde_json::json!(["nightly"]));
        let mut skipped = real["skipped"].as_array().unwrap().clone();
        skipped.sort_by_key(ToString::to_string);
        predicted.sort_by_key(ToString::to_string);
        assert_eq!(skipped, predicted);
        // Draft, v1-managed, and still PAUSED to the scheduler.
        assert_eq!(
            definition_rows(&conn),
            vec![(
                "nightly".to_owned(),
                "PAUSED".to_owned(),
                "draft".to_owned(),
                1
            )]
        );
        let event: String = conn
            .query_row(
                "SELECT event_json FROM automation_events WHERE stream_id = 'nightly'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let event: serde_json::Value = serde_json::from_str(&event).unwrap();
        assert_eq!(event["kind"], "definition.imported");
        assert_eq!(event["payload"]["lifecycleState"], "draft");

        // A draft cannot be activated; a revise must validate it first.
        let activate = super::super::command_adoption::execute_definition_command(
            &conn,
            "adopt:activate:nightly",
            super::super::command_adoption::DefinitionCommand::Activate {
                automation_id: "nightly".to_owned(),
                expected_revision: Some(1),
                reason: None,
            },
            "2026-09-28T09:01:00.000Z",
            super::super::owner_grants::CommandAuthority::OwnerLocal,
        )
        .unwrap();
        assert_eq!(
            activate.error.unwrap().code(),
            super::super::contract::error::ErrorCode::IllegalTransition
        );

        // Nor can a revise skip `paused`: the only revise exit from draft is
        // `draft -> paused`.
        let stored: serde_json::Value = serde_json::from_str(
            &super::super::store::get_definition(&conn, "nightly")
                .unwrap()
                .unwrap()
                .definition_json,
        )
        .unwrap();
        let revise = |key: &str, status: &str| {
            let mut definition = stored.clone();
            definition["status"] = serde_json::json!(status);
            super::super::command_adoption::execute_definition_command(
                &conn,
                key,
                super::super::command_adoption::DefinitionCommand::Revise {
                    definition,
                    expected_revision: Some(1),
                },
                "2026-09-28T09:02:00.000Z",
                super::super::owner_grants::CommandAuthority::OwnerLocal,
            )
            .unwrap()
        };
        let to_active = revise("adopt:revise:nightly:active", "ACTIVE");
        assert_eq!(
            to_active.error.unwrap().code(),
            super::super::contract::error::ErrorCode::IllegalTransition
        );
        assert_eq!(definition_rows(&conn)[0].2, "draft");
        let to_paused = revise("adopt:revise:nightly:paused", "PAUSED");
        assert_eq!(to_paused.outcome, DefinitionCommandOutcome::Committed);
        assert_eq!(
            definition_rows(&conn),
            vec![(
                "nightly".to_owned(),
                "PAUSED".to_owned(),
                "paused".to_owned(),
                1
            )]
        );

        // A second import skips what is already present.
        let again = draft_import(&conn, &root, "adopt:import:again", false);
        let again = again.result.unwrap();
        assert_eq!(again["imported"], serde_json::json!([]));
        assert!(again["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skip| skip == "nightly: already exists"));
        // The import and the paused revise; the refused revise and the re-import append nothing.
        assert_eq!(event_count(&conn), 2);
    }

    #[test]
    fn v1_import_replays_its_stored_report_and_refuses_a_changed_request() {
        use super::super::command_adoption::DefinitionCommandOutcome;
        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&store_path).unwrap();
        let conn = crate::store::open_store(&store_path).unwrap();
        let root = temp.path().join("automations");
        write_codex_automation(&root, "first", "RRULE:FREQ=DAILY;BYHOUR=9");

        let first = draft_import(&conn, &root, "adopt:import", false);
        assert_eq!(first.outcome, DefinitionCommandOutcome::Committed);
        // A replay answers from the adoption record, not the filesystem.
        write_codex_automation(&root, "second", "RRULE:FREQ=DAILY;BYHOUR=10");
        let replay = draft_import(&conn, &root, "adopt:import", false);
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Replayed);
        assert_eq!(replay.result, first.result);
        assert_eq!(definition_rows(&conn).len(), 1);

        let changed = draft_import(&conn, &root, "adopt:import", true);
        assert_eq!(
            changed.error.unwrap().code(),
            super::super::contract::error::ErrorCode::AdoptionReplayMismatch
        );
        assert_eq!(definition_rows(&conn).len(), 1);
    }
}
