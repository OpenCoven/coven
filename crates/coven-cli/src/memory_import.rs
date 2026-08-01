use std::path::Path;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MemoryImportSourceKind {
    Native,
    Openclaw,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum MemoryImportStatus {
    Preview,
    Applied,
    Restored,
    Conflict,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum MemoryImportEntryStatus {
    Planned,
    Created,
    Unchanged,
    Restored,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(dead_code)]
pub(crate) struct MemoryImportEntry {
    pub(crate) source_label: String,
    pub(crate) target_name: String,
    pub(crate) digest: String,
    pub(crate) status: MemoryImportEntryStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(dead_code)]
pub(crate) struct MemoryImportReport {
    pub(crate) familiar_id: String,
    pub(crate) source_kind: MemoryImportSourceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bundle_id: Option<String>,
    pub(crate) status: MemoryImportStatus,
    pub(crate) file_count: usize,
    pub(crate) created_count: usize,
    pub(crate) unchanged_count: usize,
    pub(crate) restored_count: usize,
    pub(crate) conflict_count: usize,
    pub(crate) entries: Vec<MemoryImportEntry>,
}

pub(crate) fn run_import(
    _familiar: &str,
    _source: MemoryImportSourceKind,
    _openclaw_root: Option<&Path>,
    _apply: bool,
    _json: bool,
) -> Result<()> {
    bail!("coven memory import is not implemented yet")
}

pub(crate) fn run_restore(_familiar: &str, _bundle: &str, _json: bool) -> Result<()> {
    bail!("coven memory restore is not implemented yet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_import_report_json_is_stable_and_redacted() {
        let report = MemoryImportReport {
            familiar_id: "sage".to_owned(),
            source_kind: MemoryImportSourceKind::Openclaw,
            bundle_id: Some("bundle-1".to_owned()),
            status: MemoryImportStatus::Preview,
            file_count: 1,
            created_count: 0,
            unchanged_count: 0,
            restored_count: 0,
            conflict_count: 0,
            entries: vec![MemoryImportEntry {
                source_label: "memory/notes.md".to_owned(),
                target_name: "openclaw-notes.md".to_owned(),
                digest: "blake3:abc123".to_owned(),
                status: MemoryImportEntryStatus::Planned,
            }],
        };

        let value = serde_json::to_value(&report).expect("report must serialize");
        assert_eq!(value["familiar_id"], "sage");
        assert_eq!(value["source_kind"], "openclaw");
        assert_eq!(value["status"], "preview");
        assert_eq!(value["entries"][0]["status"], "planned");

        let json = serde_json::to_string(&report).expect("report must serialize");
        for forbidden in [
            "content",
            "source_path",
            "absolute_path",
            "/Users/sage/.openclaw",
        ] {
            assert!(
                !json.contains(forbidden),
                "serialized report leaked forbidden value {forbidden:?}: {json}"
            );
        }

        let decoded: MemoryImportReport =
            serde_json::from_str(&json).expect("report must deserialize");
        assert_eq!(decoded, report);
    }

    #[test]
    fn memory_import_report_json_omits_absent_bundle_id_without_path_fields() {
        let report = MemoryImportReport {
            familiar_id: "sage".to_owned(),
            source_kind: MemoryImportSourceKind::Native,
            bundle_id: None,
            status: MemoryImportStatus::Preview,
            file_count: 0,
            created_count: 0,
            unchanged_count: 0,
            restored_count: 0,
            conflict_count: 0,
            entries: Vec::new(),
        };

        let value = serde_json::to_value(report).expect("report must serialize");
        let object = value
            .as_object()
            .expect("report must serialize as an object");
        assert!(!object.contains_key("bundle_id"));
        assert!(!object.contains_key("content"));
        assert!(!object.contains_key("source_path"));
        assert!(!object.contains_key("openclaw_root"));
        assert_no_absolute_path_values(&value);
    }

    fn assert_no_absolute_path_values(value: &serde_json::Value) {
        match value {
            serde_json::Value::String(value) => {
                assert!(
                    !value.starts_with('/'),
                    "serialized report contains an absolute path: {value}"
                );
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    assert_no_absolute_path_values(value);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    assert_no_absolute_path_values(value);
                }
            }
            _ => {}
        }
    }
}
