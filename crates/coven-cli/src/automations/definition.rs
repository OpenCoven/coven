//! Coven-native routine definitions.
//!
//! A routine is a durable, Coven-owned automation definition: a recurring
//! schedule plus a familiar-bound prompt that a runtime executes. Definitions
//! are the source of truth for identity and lifecycle; execution state lives
//! in the occurrence ledger, not here (coven#816).

use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::rrule::{parse_rrule, ParsedRrule};

pub const AUTOMATION_SCHEMA_VERSION: u32 = 1;

/// An automation identifier. Keep it stable across edits: the ledger and the
/// scheduler key occurrences by this id.
pub const AUTOMATION_ID_MAX_CHARS: usize = 96;

/// Accepted `status` values. New definitions default to PAUSED so nothing
/// runs until a human opts in (coven#816 acceptance: "default PAUSED").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RoutineStatus {
    Active,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutineTimezone {
    /// Compatibility input only. Durable definitions must resolve this to an
    /// exact IANA zone before they are stored.
    Local,
    Utc,
    Iana(Tz),
}

impl RoutineTimezone {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "local" => Ok(Self::Local),
            "utc" => Ok(Self::Utc),
            value => value.parse::<Tz>().map(Self::Iana).map_err(|_| {
                format!(
                    "timezone must be `utc`, compatibility input `local`, or a valid IANA timezone, got `{value}`"
                )
            }),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Utc => "utc",
            Self::Iana(timezone) => timezone.name(),
        }
    }

    pub fn resolve_for_persistence(self) -> Result<Self, String> {
        if self != Self::Local {
            return Ok(self);
        }
        #[cfg(unix)]
        match std::env::var("TZ") {
            Ok(value) => {
                let timezone = Self::parse(&value).map_err(|_| {
                    format!("TZ override must be `utc` or an exact IANA timezone, got `{value}`")
                })?;
                if timezone == Self::Local {
                    return Err(
                        "TZ override must resolve to `utc` or an exact IANA timezone, not `local`"
                            .to_string(),
                    );
                }
                return Ok(timezone);
            }
            Err(std::env::VarError::NotPresent) => {}
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(
                    "TZ override must be valid UTF-8 naming `utc` or an exact IANA timezone"
                        .to_string(),
                );
            }
        }
        self.resolve_local_with(|| {
            iana_time_zone::get_timezone()
                .map_err(|error| format!("could not determine the system IANA timezone: {error}"))
        })
    }

    pub(crate) fn resolve_local_with(
        self,
        resolver: impl FnOnce() -> Result<String, String>,
    ) -> Result<Self, String> {
        if self != Self::Local {
            return Ok(self);
        }
        let resolved = resolver()?;
        let timezone = Self::parse(&resolved)?;
        if timezone == Self::Local {
            return Err(
                "system timezone resolver returned `local` instead of an exact IANA timezone"
                    .to_string(),
            );
        }
        Ok(timezone)
    }

    pub fn is_durable(self) -> bool {
        self != Self::Local
    }
}

impl Serialize for RoutineTimezone {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RoutineTimezone {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineMisfire {
    /// On recovery, only the latest missed occurrence runs.
    Latest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineOverlap {
    /// A run is skipped when the previous occurrence has not settled.
    Forbid,
}

/// A validated routine definition. Serialized to `definition_json` in the
/// store with camelCase keys, mirroring the control-plane wire style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutineDefinition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub status: RoutineStatus,
    /// RRULE text, e.g. `FREQ=DAILY;BYHOUR=9,17`.
    pub rrule: String,
    pub timezone: RoutineTimezone,
    pub misfire: RoutineMisfire,
    pub overlap: RoutineOverlap,
    /// Per-run wall-clock timeout in minutes. Bounded and required.
    pub timeout_minutes: u32,
    /// Runtime identifier (harness), default `coven-code`.
    pub runtime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub familiar_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Reserved output delivery field. Validation refuses it until delivery
    /// has a pinned definition revision and crash-recoverable commit protocol.
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

    pub fn from_legacy_json(value: &Value) -> Result<Self, String> {
        Self::from_json(&Self::legacy_wire_projection(value))
    }

    pub(crate) fn legacy_wire_projection(value: &Value) -> Value {
        const LEGACY_FIELDS: &[&str] = &[
            "schemaVersion",
            "id",
            "name",
            "status",
            "rrule",
            "timezone",
            "misfire",
            "overlap",
            "timeoutMinutes",
            "runtime",
            "familiarId",
            "cwd",
            "outputTarget",
            "prompt",
            "model",
            "tags",
        ];
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .filter(|(key, _)| LEGACY_FIELDS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            _ => value.clone(),
        }
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
        if self.output_target.is_some() {
            return Err(
                "outputTarget is not supported until atomic delivery is certified".to_string(),
            );
        }
        Ok(())
    }

    pub fn validate_durable(&self) -> Result<(), String> {
        self.validate()?;
        if !self.timezone.is_durable() {
            return Err(
                "timezone `local` is compatibility input only and must resolve to an exact IANA timezone before persistence"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn resolve_timezone_for_persistence(mut self) -> Result<Self, String> {
        self.timezone = self.timezone.resolve_for_persistence()?;
        Ok(self)
    }

    /// Normalized wire form (camelCase, schema stamped) used by list/get.
    #[allow(dead_code)]
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
    fn accepts_a_valid_iana_timezone() {
        let mut value = valid_definition();
        value
            .as_object_mut()
            .unwrap()
            .insert("timezone".to_string(), json!("America/New_York"));

        let definition = RoutineDefinition::from_json(&value).unwrap();

        assert_eq!(
            serde_json::to_value(definition).unwrap()["timezone"],
            "America/New_York"
        );
    }

    #[test]
    fn rejects_an_unknown_iana_timezone() {
        let mut value = valid_definition();
        value
            .as_object_mut()
            .unwrap()
            .insert("timezone".to_string(), json!("Mars/Olympus"));

        let error = RoutineDefinition::from_json(&value).unwrap_err();

        assert!(error.contains("valid IANA timezone"), "{error}");
    }

    #[test]
    fn local_resolution_fails_explicitly_when_the_platform_cannot_prove_a_zone() {
        let error = RoutineTimezone::Local
            .resolve_local_with(|| Err("platform did not provide a TZID".to_string()))
            .unwrap_err();

        assert_eq!(error, "platform did not provide a TZID");
    }

    #[cfg(unix)]
    #[test]
    fn local_resolution_rejects_an_unrepresentable_tz_override() {
        const CHILD_ENV: &str = "COVEN_TEST_LOCAL_TIMEZONE_CHILD";
        const TEST_NAME: &str =
            "automations::definition::tests::local_resolution_rejects_an_unrepresentable_tz_override";

        if std::env::var_os(CHILD_ENV).is_some() {
            let error = RoutineTimezone::Local
                .resolve_for_persistence()
                .unwrap_err();
            assert!(error.contains("TZ"), "{error}");
            return;
        }

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .env("TZ", ":/tmp/coven-custom-zoneinfo")
            .status()
            .unwrap();

        assert!(status.success(), "child timezone assertion failed");
    }

    #[cfg(unix)]
    #[test]
    fn local_resolution_prefers_an_iana_tz_override() {
        const CHILD_ENV: &str = "COVEN_TEST_IANA_TIMEZONE_CHILD";
        const TEST_NAME: &str =
            "automations::definition::tests::local_resolution_prefers_an_iana_tz_override";

        if std::env::var_os(CHILD_ENV).is_some() {
            let timezone = RoutineTimezone::Local.resolve_for_persistence().unwrap();
            assert_eq!(timezone.as_str(), "Pacific/Kiritimati");
            return;
        }

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .env("TZ", "Pacific/Kiritimati")
            .status()
            .unwrap();

        assert!(status.success(), "child timezone assertion failed");
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
    fn rejects_output_target_until_atomic_delivery_is_certified() {
        let mut value = valid_definition();
        value
            .as_object_mut()
            .unwrap()
            .insert("outputTarget".to_string(), json!("result.md"));
        let error = RoutineDefinition::from_json(&value).unwrap_err();
        assert!(error.contains("outputTarget is not supported"), "{error}");
    }

    #[test]
    fn legacy_parser_ignores_unknown_fields_while_v1_parser_rejects_them() {
        let mut value = valid_definition();
        value
            .as_object_mut()
            .unwrap()
            .insert("futureField".to_string(), json!("ignored by legacy"));

        assert!(RoutineDefinition::from_legacy_json(&value).is_ok());
        let error = RoutineDefinition::from_json(&value).unwrap_err();
        assert!(error.contains("unknown field `futureField`"), "{error}");
    }
}
