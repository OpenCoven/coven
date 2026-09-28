//! Executable status of every `coven.automations.v1` command (coven#1054).
//!
//! The command envelope names nineteen versioned commands. This producer
//! dispatches some of them as `coven.automations.<command>` control actions,
//! covers some only through an unversioned legacy action that takes no caller
//! adoption key or expected revision, and implements the rest not at all. A versioned
//! name is advertised only when it is `Implemented`; every other versioned
//! name is refused with `CAPABILITY_UNSUPPORTED` before it can touch state.

/// How this producer executes one versioned command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSupport {
    /// Dispatched under its versioned action name.
    Implemented,
    /// Refused under its versioned name. The legacy action covers part of the
    /// behaviour but takes no caller adoption key or expected revision.
    CompatibilityOnly { legacy_action: &'static str },
    /// Refused; nothing in this producer performs it.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandEntry {
    /// The envelope `command` name, e.g. `definition.revise.v1`.
    pub command: &'static str,
    pub support: CommandSupport,
}

use CommandSupport::{CompatibilityOnly, Implemented, Unsupported};

/// Every command in `command-envelope.schema.json`, in schema order.
pub const COMMAND_MATRIX: &[CommandEntry] = &[
    entry("definition.create.v1", Implemented),
    entry("definition.revise.v1", Implemented),
    entry(
        "definition.activate.v1",
        CompatibilityOnly {
            legacy_action: "coven.automations.update",
        },
    ),
    entry(
        "definition.pause.v1",
        CompatibilityOnly {
            legacy_action: "coven.automations.update",
        },
    ),
    entry("definition.disable.v1", Implemented),
    entry("definition.tombstone.v1", Implemented),
    entry(
        "occurrence.runNow.v1",
        CompatibilityOnly {
            legacy_action: "coven.automations.run",
        },
    ),
    entry("occurrence.cancel.v1", Unsupported),
    entry("run.cancel.v1", Implemented),
    entry("attempt.cancel.v1", Unsupported),
    entry("attempt.retry.v1", Unsupported),
    entry("occurrence.recover.v1", Unsupported),
    entry("definition.list.v1", Implemented),
    entry("definition.get.v1", Implemented),
    entry(
        "run.history.v1",
        CompatibilityOnly {
            legacy_action: "coven.automations.runs",
        },
    ),
    entry(
        "definition.health.v1",
        CompatibilityOnly {
            legacy_action: "coven.automations.health",
        },
    ),
    entry("events.read.v1", Implemented),
    entry("events.subscribe.v1", Implemented),
    entry(
        "legacy.import.v1",
        CompatibilityOnly {
            legacy_action: "coven.automations.import",
        },
    ),
];

/// Versioned read actions this producer adds outside the command enum. They
/// are diagnostics, not commands, and carry no adoption semantics.
#[cfg(test)]
pub const PRODUCER_READ_EXTENSIONS: &[&str] = &[
    "coven.automations.receipt.get.v1",
    "coven.automations.scheduler.status.v1",
    "coven.automations.occurrence.list.v1",
    "coven.automations.occurrence.get.v1",
    "coven.automations.run.get.v1",
    "coven.automations.occurrence.history.v1",
];

const ACTION_PREFIX: &str = "coven.automations.";

const fn entry(command: &'static str, support: CommandSupport) -> CommandEntry {
    CommandEntry { command, support }
}

#[cfg(test)]
pub fn action_name(command: &str) -> String {
    format!("{ACTION_PREFIX}{command}")
}

/// The matrix entry for a versioned action this producer refuses, if any.
pub fn refused_command(action: &str) -> Option<&'static CommandEntry> {
    let command = action.strip_prefix(ACTION_PREFIX)?;
    COMMAND_MATRIX
        .iter()
        .find(|entry| entry.command == command && entry.support != Implemented)
}

/// The operator-facing refusal for a command this producer does not implement.
pub fn refusal_message(entry: &CommandEntry) -> String {
    match entry.support {
        CompatibilityOnly { legacy_action } => format!(
            "`{}` is not implemented by this producer. `{legacy_action}` covers part of it \
             but takes no adoption key or expected revision.",
            entry.command
        ),
        Unsupported | Implemented => {
            format!("`{}` is not implemented by this producer.", entry.command)
        }
    }
}

/// The matrix as the Markdown table published in
/// `docs/architecture/coven-automations-v1.md`.
#[cfg(test)]
pub fn markdown_table() -> String {
    let mut table = String::from(
        "| Command | Status | Versioned action or legacy coverage |\n| --- | --- | --- |\n",
    );
    for entry in COMMAND_MATRIX {
        let (status, coverage) = match entry.support {
            Implemented => ("implemented", format!("`{}`", action_name(entry.command))),
            CompatibilityOnly { legacy_action } => {
                ("compatibility-only", format!("`{legacy_action}`"))
            }
            Unsupported => ("unsupported", "none".to_owned()),
        };
        table.push_str(&format!(
            "| `{}` | {status} | {coverage} |\n",
            entry.command
        ));
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND_ENVELOPE_SCHEMA: &str =
        include_str!("../../../../spec/coven-automations/v1/command-envelope.schema.json");
    const ARCHITECTURE_DOC: &str =
        include_str!("../../../../docs/architecture/coven-automations-v1.md");

    #[test]
    fn matrix_lists_every_schema_command_once_in_schema_order() {
        let schema: serde_json::Value = serde_json::from_str(COMMAND_ENVELOPE_SCHEMA).unwrap();
        let commands = schema["$defs"]["commandName"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|command| command.as_str().unwrap())
            .collect::<Vec<_>>();
        let matrix = COMMAND_MATRIX
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>();
        assert_eq!(matrix, commands);
    }

    #[test]
    fn compatibility_coverage_names_only_unversioned_actions() {
        for entry in COMMAND_MATRIX {
            if let CompatibilityOnly { legacy_action } = entry.support {
                assert!(legacy_action.starts_with(ACTION_PREFIX));
                assert!(!legacy_action.ends_with(".v1"), "{legacy_action}");
            }
        }
    }

    #[test]
    fn refuses_only_versioned_names_that_are_not_implemented() {
        assert_eq!(
            refused_command("coven.automations.attempt.retry.v1").map(|entry| entry.support),
            Some(Unsupported)
        );
        assert!(refused_command("coven.automations.definition.revise.v1").is_none());
        assert!(refused_command("coven.automations.update").is_none());
        assert!(refused_command("attempt.retry.v1").is_none());
        assert!(refused_command("coven.automations.attempt.retry.v2").is_none());
    }

    #[test]
    fn architecture_doc_publishes_the_current_matrix() {
        // Windows checkouts may convert the doc to CRLF.
        let doc = ARCHITECTURE_DOC.replace("\r\n", "\n");
        assert!(
            doc.contains(&markdown_table()),
            "docs/architecture/coven-automations-v1.md must contain:\n{}",
            markdown_table()
        );
    }
}
