//! Rich `coven.automations.v1` definitions on the flat executor (coven#1054).
//!
//! A rich `AutomationDefinition` is validated with the typed contract,
//! including its JCS integrity digest, and projected onto the executable
//! `RoutineDefinition`, which the scheduler and runner still run. The rich
//! body is stored verbatim beside the routine so nothing it states is lost.
//! Its `revision`, `lifecycleState` and `integrity` describe one moment, so a
//! read regenerates them from the current row rather than serving a stale copy.
//!
//! The projection covers the executable subset. Variants the executor cannot
//! honour are refused rather than dropped: `policies.delivery` (atomic output
//! targets are refused by the capability profile) and `activation` windows.
//! `binding.authority` is required by the schema; it is stored as a reference
//! and not enforced until trusted Runtime Authority exists (coven#857).

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

use super::capability_negotiation::preflight_definition;
use super::contract::canonical_json::{canonicalize, canonicalize_without_integrity, sha256_hex};
use super::contract::types::AutomationDefinition;

/// Why a rich definition cannot be taken as submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Not a valid `coven.automations.v1` definition.
    Invalid(String),
    /// Valid, but uses a variant this producer cannot execute.
    Unsupported { variant: String, reason: String },
    /// Valid, but not a lawful lifecycle transition.
    Transition(String),
}

/// A validated rich definition and the routine the executor runs for it.
pub(crate) struct Projection {
    pub automation_id: String,
    pub revision: u64,
    pub lifecycle_state: String,
    /// Canonical JCS text of the submitted body.
    pub canonical: String,
    /// `RoutineDefinition` JSON with the status the lifecycle state implies.
    pub routine: Value,
}

/// Validates `definition` and projects it onto a routine.
pub(crate) fn project(definition: &Value) -> Result<Projection, Refusal> {
    let typed: AutomationDefinition =
        serde_json::from_value(definition.clone()).map_err(|error| {
            Refusal::Invalid(format!(
                "definition does not match coven.automations.v1: {error}"
            ))
        })?;
    typed.verify_integrity().map_err(|_| {
        Refusal::Invalid("definition integrity digest does not match its body".to_owned())
    })?;
    let field = |path: &[&str]| -> Value {
        path.iter()
            .try_fold(definition, |value, key| value.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };
    // The shared capability preflight refuses every variant the profile does
    // not advertise, such as non-standard retention classes.
    if let Some(unsupported) = preflight_definition(definition) {
        return Err(Refusal::Unsupported {
            variant: unsupported.variant,
            reason: unsupported.reason,
        });
    }
    if !field(&["policies", "delivery"]).is_null() {
        return Err(Refusal::Unsupported {
            variant: "outputTarget.atomic".to_owned(),
            reason: "policies.delivery is not executable by this producer".to_owned(),
        });
    }
    if !field(&["activation"]).is_null() {
        return Err(Refusal::Unsupported {
            variant: "activation".to_owned(),
            reason: "activation windows are not executable by this producer".to_owned(),
        });
    }
    let lifecycle_state = field(&["lifecycleState"])
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let status = match lifecycle_state.as_str() {
        "active" => "ACTIVE",
        "draft" | "paused" => "PAUSED",
        other => {
            return Err(Refusal::Transition(format!(
                "a definition cannot be written in the `{other}` state"
            )))
        }
    };
    let mut routine = Map::new();
    routine.insert("schemaVersion".into(), json!(1));
    routine.insert("id".into(), field(&["automationId"]));
    routine.insert("name".into(), field(&["display", "name"]));
    routine.insert("status".into(), json!(status));
    routine.insert("rrule".into(), field(&["trigger", "schedule", "rrule"]));
    routine.insert(
        "timezone".into(),
        field(&["trigger", "schedule", "timezone"]),
    );
    routine.insert("misfire".into(), json!("latest"));
    routine.insert("overlap".into(), json!("forbid"));
    routine.insert(
        "timeoutMinutes".into(),
        field(&["policies", "timeout", "perRunMinutes"]),
    );
    let runtime = field(&["runtimeRequirements", "runtimeId"]);
    routine.insert(
        "runtime".into(),
        if runtime.is_null() {
            json!("coven-code")
        } else {
            runtime
        },
    );
    routine.insert("prompt".into(), field(&["action", "prompt"]));
    routine.insert("retry".into(), field(&["policies", "retry"]));
    for (key, path) in [
        ("cwd", &["action", "cwd"][..]),
        ("familiarId", &["binding", "familiarId"][..]),
        ("model", &["runtimeRequirements", "model"][..]),
        ("tags", &["display", "tags"][..]),
    ] {
        let value = field(path);
        if !value.is_null() {
            routine.insert(key.into(), value);
        }
    }
    let canonical = String::from_utf8(
        canonicalize(definition).map_err(|error| Refusal::Invalid(format!("{error:#}")))?,
    )
    .map_err(|error| Refusal::Invalid(error.to_string()))?;
    Ok(Projection {
        automation_id: field(&["automationId"])
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        revision: field(&["revision"]).as_u64().unwrap_or_default(),
        lifecycle_state,
        canonical,
        routine: Value::Object(routine),
    })
}

/// The stored rich body regenerated for the row's current revision and
/// lifecycle state, with its integrity recomputed; `None` when the definition
/// was not authored richly.
pub(crate) fn current_view(
    conn: &Connection,
    automation_id: &str,
) -> anyhow::Result<Option<Value>> {
    let Some((stored, revision, lifecycle_state)) = conn
        .query_row(
            "SELECT rich_definition_json, revision, lifecycle_state
             FROM automation_definitions
             WHERE id = ?1 AND rich_definition_json IS NOT NULL",
            [automation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut definition: Value = serde_json::from_str(&stored)?;
    definition["revision"] = json!(revision);
    definition["lifecycleState"] = json!(lifecycle_state);
    let digest = sha256_hex(&canonicalize_without_integrity(&definition)?);
    definition["integrity"] = json!({
        "algorithm": "sha256",
        "canonicalization": "jcs-rfc8785",
        "value": digest,
    });
    Ok(Some(definition))
}

/// Drops rich bodies that no longer describe their definition's current
/// revision. Every revision this producer commits for a richly authored
/// definition is recorded below, so a current revision without a record was
/// committed by a binary that predates rich bodies (a rollback). That binary
/// kept the old body, which would otherwise be served and re-recorded as the
/// new revision. Recorded revisions stay, so nothing authored is lost.
pub(crate) fn clear_stale_rich_bodies(conn: &Connection) -> anyhow::Result<usize> {
    Ok(conn.execute(
        "UPDATE automation_definitions
         SET rich_definition_json = NULL
         WHERE rich_definition_json IS NOT NULL
           AND NOT EXISTS (
               SELECT 1
               FROM automation_rich_definition_revisions AS recorded
               WHERE recorded.automation_id = automation_definitions.id
                 AND recorded.revision = automation_definitions.revision
           )",
        [],
    )?)
}

/// Records what the definition's current revision means in rich terms, when
/// it was authored richly. Called once per committed revision, inside its
/// transaction, so every revision's rich body stays recoverable.
pub(crate) fn record_revision(
    conn: &Connection,
    automation_id: &str,
    recorded_at: &str,
) -> anyhow::Result<()> {
    let Some(view) = current_view(conn, automation_id)? else {
        return Ok(());
    };
    conn.execute(
        "INSERT INTO automation_rich_definition_revisions
            (automation_id, revision, rich_definition_json, integrity, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            automation_id,
            view["revision"].as_i64(),
            String::from_utf8(canonicalize(&view)?)?,
            view["integrity"]["value"].as_str(),
            recorded_at,
        ],
    )?;
    Ok(())
}
