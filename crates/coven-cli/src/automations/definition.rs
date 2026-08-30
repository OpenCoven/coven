//! Coven-native routine definitions.
//!
//! A routine is a durable, Coven-owned automation definition: a recurring
//! schedule plus a familiar-bound prompt that a runtime executes. Definitions
//! are the source of truth for identity and lifecycle; execution state lives
//! in the occurrence ledger, not here (coven#816).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::rrule::{parse_rrule, ParsedRrule};

pub const AUTOMATION_SCHEMA_VERSION: u32 = 1;

/// An automation identifier. Keep it stable across edits: the ledger and the
/// scheduler key occurrences by this id.
pub const AUTOMATION_ID_MAX_CHARS: usize = 96;

/// Wire default for `runtime`: Coven's native runtime executes routine work
/// unless a definition explicitly selects another adapter (coven#816).
fn default_runtime() -> String {
    "coven-code".to_string()
}

/// Accepted `status` values. New definitions default to PAUSED so nothing
/// runs until a human opts in (coven#816 acceptance: "default PAUSED").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RoutineStatus {
    #[default]
    Paused,
    Active,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineTimezone {
    #[default]
    Local,
    Utc,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineMisfire {
    /// On recovery, only the latest missed occurrence runs.
    #[default]
    Latest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineOverlap {
    /// A run is skipped when the previous occurrence has not settled.
    #[default]
    Forbid,
}

/// A validated routine definition. Serialized to `definition_json` in the
/// store with camelCase keys, mirroring the control-plane wire style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineDefinition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    /// Omitted on the wire this defaults to PAUSED: nothing runs until a
    /// human activates the routine (coven#816 fail-closed default).
    #[serde(default)]
    pub status: RoutineStatus,
    /// RRULE text, e.g. `FREQ=DAILY;BYHOUR=9,17`.
    pub rrule: String,
    /// Omitted on the wire this defaults to `local` (coven#816 import parity).
    #[serde(default)]
    pub timezone: RoutineTimezone,
    /// Omitted on the wire this defaults to `latest`: after a restart only
    /// the latest missed occurrence runs (coven#816 default misfire).
    #[serde(default)]
    pub misfire: RoutineMisfire,
    /// Omitted on the wire this defaults to `forbid`: a routine never runs
    /// over its own unsettled previous occurrence (coven#816 overlap rule).
    #[serde(default)]
    pub overlap: RoutineOverlap,
    /// Per-run wall-clock timeout in minutes. Bounded and required.
    pub timeout_minutes: u32,
    /// Runtime identifier (harness). Omitted on the wire this defaults to
    /// `coven-code`, Coven's native runtime; Codex/Claude/Copilot remain
    /// selectable workers but none may own the schedule (coven#816).
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub familiar_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Optional atomic output target path (delivery lands there only on a
    /// completed, verified run).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_target: Option<String>,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,
}

impl RoutineDefinition {
    /// Validates a parsed definition and returns a structured error message
    /// for the control plane. Parsing and validation are one reviewable
    /// surface so every path (create, update, import) applies the same rules.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let parsed: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("routine definition failed validation: {error}"))?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != AUTOMATION_SCHEMA_VERSION {
            return Err(format!(
                "schemaVersion must be {AUTOMATION_SCHEMA_VERSION}, got {}",
                self.schema_version
            ));
        }
        if self.id.is_empty() || self.id.len() > AUTOMATION_ID_MAX_CHARS {
            return Err(format!(
                "id must be 1..={AUTOMATION_ID_MAX_CHARS} characters, got {}",
                self.id.len()
            ));
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err("id may contain only ASCII letters, digits, '-', '_', and '.'".to_string());
        }
        let trimmed_name = self.name.trim();
        if trimmed_name.is_empty() || trimmed_name.len() > 160 {
            return Err("name must be 1..=160 characters".to_string());
        }
        if self.rrule.trim().is_empty() {
            return Err("rrule is required".to_string());
        }
        let _: ParsedRrule = parse_rrule(&self.rrule)
            .map_err(|error| format!("rrule failed validation: {error}"))?;
        if self.timeout_minutes == 0 || self.timeout_minutes > 60 * 24 * 31 {
            return Err("timeoutMinutes must be 1..=44640".to_string());
        }
        if self.runtime.trim().is_empty() || self.runtime.len() > 64 {
            return Err("runtime must be 1..=64 characters".to_string());
        }
        if self.prompt.trim().is_empty() {
            return Err("prompt is required".to_string());
        }
        if let Some(familiar_id) = &self.familiar_id {
            let trimmed = familiar_id.trim();
            if trimmed.is_empty() || trimmed.len() > 64 {
                return Err("familiarId must be 1..=64 characters".to_string());
            }
        }
        Ok(())
    }

    /// Normalized wire form (camelCase, schema stamped) used by list/get.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_definition() -> Value {
        json!({
            "schemaVersion": 1,
            "id": "daily-notes",
            "name": "Daily notes",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "local",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "familiarId": "charm",
            "prompt": "Write the daily reflection."
        })
    }

    #[test]
    fn parses_and_validates_a_complete_definition() {
        let definition = RoutineDefinition::from_json(&valid_definition()).unwrap();
        assert_eq!(definition.id, "daily-notes");
        assert_eq!(definition.status, RoutineStatus::Paused);
        assert_eq!(definition.timezone, RoutineTimezone::Local);
        assert_eq!(definition.timeout_minutes, 30);
    }

    #[test]
    fn rejects_a_missing_prompt() {
        let mut value = valid_definition();
        value.as_object_mut().unwrap().remove("prompt");
        let error = RoutineDefinition::from_json(&value).unwrap_err();
        assert!(error.contains("failed validation"), "{error}");
    }

    #[test]
    fn rejects_a_bad_identifier() {
        let mut value = valid_definition();
        value
            .as_object_mut()
            .unwrap()
            .insert("id".to_string(), json!("daily notes!"));
        let error = RoutineDefinition::from_json(&value).unwrap_err();
        assert!(error.contains("id may contain only"), "{error}");
    }

    #[test]
    fn rejects_an_unsupported_rrule() {
        let mut value = valid_definition();
        value
            .as_object_mut()
            .unwrap()
            .insert("rrule".to_string(), json!("FREQ=HOURLY"));
        let error = RoutineDefinition::from_json(&value).unwrap_err();
        assert!(error.contains("rrule"), "{error}");
    }

    #[test]
    fn rejects_zero_timeout() {
        let mut value = valid_definition();
        value
            .as_object_mut()
            .unwrap()
            .insert("timeoutMinutes".to_string(), json!(0));
        let error = RoutineDefinition::from_json(&value).unwrap_err();
        assert!(error.contains("timeoutMinutes"), "{error}");
    }

    #[test]
    fn wire_defaults_are_fail_closed_and_coven_native() {
        let value = json!({
            "schemaVersion": 1,
            "id": "defaults",
            "name": "Defaults",
            "rrule": "FREQ=DAILY;BYHOUR=9,17",
            "timeoutMinutes": 30,
            "prompt": "Twice daily."
        });
        let definition = RoutineDefinition::from_json(&value).unwrap();
        assert_eq!(definition.status, RoutineStatus::Paused);
        assert_eq!(definition.runtime, "coven-code");
        assert_eq!(definition.misfire, RoutineMisfire::Latest);
        assert_eq!(definition.overlap, RoutineOverlap::Forbid);
        assert_eq!(definition.timezone, RoutineTimezone::Local);
    }
}
