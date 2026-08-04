//! Versioned contract shared by the Coven runner and harness adapters.
//!
//! This module deliberately contains no process-spawning code.  The daemon
//! remains the authority for cwd validation, process ownership, and policy;
//! adapters negotiate only the capability and event vocabulary they can
//! faithfully implement.  Keeping this boundary pure makes the wire rules
//! fixture-testable for bundled and external adapters alike.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

pub const CONTRACT_NAME: &str = "coven.harness";
pub const CONTRACT_V1: &str = "coven.harness.v1";

/// Extensions understood by the v1 runner.  Base lifecycle, text input,
/// streamed output, cancellation, and errors are mandatory v1 semantics; the
/// extension list is for additive behavior only.
const KNOWN_EXTENSIONS: &[&str] = &["input.image.v1", "output.tool-use.v1"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractOffer {
    pub contract: String,
    pub versions: Vec<String>,
    #[serde(default)]
    pub required_extensions: Vec<String>,
    #[serde(default)]
    pub optional_extensions: Vec<String>,
}

impl ContractOffer {
    pub fn v1() -> Self {
        Self {
            contract: CONTRACT_NAME.to_string(),
            versions: vec![CONTRACT_V1.to_string()],
            required_extensions: Vec::new(),
            optional_extensions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedContract {
    pub version: String,
    pub extensions: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ContractError {}

fn contract_error(code: &'static str, message: impl Into<String>) -> ContractError {
    ContractError {
        code,
        message: message.into(),
    }
}

/// Select the newest mutually supported contract and validate required
/// extensions on *both* sides.  Optional extensions are deliberately ignored
/// when unknown, which is the v1 forward-compatibility rule.
pub fn negotiate(
    runner: &ContractOffer,
    adapter: &ContractOffer,
) -> Result<NegotiatedContract, ContractError> {
    if runner.contract != CONTRACT_NAME || adapter.contract != CONTRACT_NAME {
        return Err(contract_error(
            "contract_name_mismatch",
            format!(
                "runner and adapter must both use `{CONTRACT_NAME}` (got `{}` and `{}`)",
                runner.contract, adapter.contract
            ),
        ));
    }

    let version = runner
        .versions
        .iter()
        .filter(|version| adapter.versions.contains(*version))
        .filter(|version| version.as_str() == CONTRACT_V1)
        .max()
        .cloned()
        .ok_or_else(|| {
            contract_error(
                "unsupported_contract_version",
                format!(
                    "no mutually supported harness contract version; runner={:?}, adapter={:?}",
                    runner.versions, adapter.versions
                ),
            )
        })?;

    let runner_extensions = known_extensions(runner)?;
    let adapter_extensions = known_extensions(adapter)?;
    for extension in &runner.required_extensions {
        if !adapter_extensions.contains(extension) {
            return Err(contract_error(
                "required_extension_unavailable",
                format!("adapter does not offer runner-required extension `{extension}`"),
            ));
        }
    }
    for extension in &adapter.required_extensions {
        if !runner_extensions.contains(extension) {
            return Err(contract_error(
                "required_extension_unavailable",
                format!("runner does not offer adapter-required extension `{extension}`"),
            ));
        }
    }

    Ok(NegotiatedContract {
        version,
        extensions: runner_extensions
            .intersection(&adapter_extensions)
            .cloned()
            .collect(),
    })
}

fn known_extensions(offer: &ContractOffer) -> Result<BTreeSet<String>, ContractError> {
    let mut extensions = BTreeSet::new();
    for (required, extension) in offer
        .required_extensions
        .iter()
        .map(|extension| (true, extension))
        .chain(
            offer
                .optional_extensions
                .iter()
                .map(|extension| (false, extension)),
        )
    {
        if KNOWN_EXTENSIONS.contains(&extension.as_str()) {
            extensions.insert(extension.clone());
        } else if required {
            return Err(contract_error(
                "unknown_required_extension",
                format!("required extension `{extension}` is not understood by v1"),
            ));
        }
    }
    Ok(extensions)
}

/// Frames sent by the runner after negotiation.  Input is structured so an
/// adapter never has to infer session ownership from a PTY byte stream.  Image
/// input is admitted only when `input.image.v1` was negotiated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerFrame {
    Input {
        session_id: String,
        request_id: String,
        content: InputContent,
    },
    Cancel {
        session_id: String,
        request_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContent {
    Text { text: String },
    Image { media_type: String, data: String },
}

/// Frames sent by an adapter after v1 negotiation.  Serde intentionally
/// accepts unknown fields: all newly added fields are optional unless they are
/// introduced as a required, negotiated extension.  Unknown frame *types* and
/// missing base fields still fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdapterFrame {
    Ready {
        session_id: String,
    },
    Output {
        session_id: String,
        sequence: u64,
        kind: OutputKind,
        text: String,
    },
    CancellationAcknowledged {
        session_id: String,
        request_id: String,
    },
    Terminal {
        session_id: String,
        sequence: u64,
        outcome: TerminalOutcome,
        #[serde(default)]
        error: Option<AdapterError>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    Text,
    Raw,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterError {
    pub code: String,
    pub message: String,
}

/// Per-session conformance state.  It preserves already accepted output if a
/// later terminal frame fails: partial output is evidence, never a success
/// fallback, and every session must end with one explicit terminal outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConformance {
    session_id: String,
    ready: bool,
    last_sequence: Option<u64>,
    terminal: Option<TerminalOutcome>,
    cancellation_requested: Option<String>,
}

impl SessionConformance {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            ready: false,
            last_sequence: None,
            terminal: None,
            cancellation_requested: None,
        }
    }

    pub fn request_cancellation(
        &mut self,
        request_id: impl Into<String>,
    ) -> Result<(), ContractError> {
        if self.terminal.is_some() {
            return Err(contract_error(
                "session_already_terminal",
                "cannot cancel a session after its terminal frame",
            ));
        }
        self.cancellation_requested = Some(request_id.into());
        Ok(())
    }

    pub fn accept(&mut self, frame: &AdapterFrame) -> Result<(), ContractError> {
        let session_id = match frame {
            AdapterFrame::Ready { session_id }
            | AdapterFrame::CancellationAcknowledged { session_id, .. }
            | AdapterFrame::Output { session_id, .. }
            | AdapterFrame::Terminal { session_id, .. } => session_id,
        };
        if session_id != &self.session_id {
            return Err(contract_error(
                "session_id_mismatch",
                format!(
                    "frame is for `{session_id}`, expected `{}`",
                    self.session_id
                ),
            ));
        }
        if self.terminal.is_some() {
            return Err(contract_error(
                "frame_after_terminal",
                "adapter emitted a frame after the terminal outcome",
            ));
        }

        if matches!(frame, AdapterFrame::Ready { .. }) {
            if self.ready {
                return Err(contract_error(
                    "duplicate_ready_frame",
                    "adapter emitted ready more than once",
                ));
            }
            self.ready = true;
            return Ok(());
        }
        if !self.ready {
            return Err(contract_error(
                "output_before_ready",
                "adapter emitted a non-ready frame before ready",
            ));
        }

        match frame {
            AdapterFrame::Ready { .. } => unreachable!("ready frame returns above"),
            AdapterFrame::Output { sequence, .. } => self.accept_sequence(*sequence),
            AdapterFrame::CancellationAcknowledged { request_id, .. } => {
                if self.cancellation_requested.as_deref() != Some(request_id) {
                    return Err(contract_error(
                        "unexpected_cancellation_acknowledgement",
                        format!("no matching cancellation request `{request_id}`"),
                    ));
                }
                Ok(())
            }
            AdapterFrame::Terminal {
                sequence,
                outcome,
                error,
                ..
            } => {
                self.accept_sequence(*sequence)?;
                if *outcome == TerminalOutcome::Failed
                    && error.as_ref().is_none_or(|error| {
                        error.code.trim().is_empty() || error.message.trim().is_empty()
                    })
                {
                    return Err(contract_error(
                        "missing_terminal_error",
                        "a failed terminal frame must include a non-empty error code and message",
                    ));
                }
                if *outcome != TerminalOutcome::Failed && error.is_some() {
                    return Err(contract_error(
                        "unexpected_terminal_error",
                        "completed or cancelled terminal frames must not carry an error object",
                    ));
                }
                if self.cancellation_requested.is_some() && *outcome != TerminalOutcome::Cancelled {
                    return Err(contract_error(
                        "cancellation_outcome_mismatch",
                        "a cancellation request must end in the cancelled terminal outcome",
                    ));
                }
                self.terminal = Some(*outcome);
                Ok(())
            }
        }
    }

    pub fn finish(self) -> Result<TerminalOutcome, ContractError> {
        self.terminal.ok_or_else(|| {
            contract_error(
                "missing_terminal_frame",
                "adapter stream ended without an explicit terminal outcome",
            )
        })
    }

    fn accept_sequence(&mut self, sequence: u64) -> Result<(), ContractError> {
        if self
            .last_sequence
            .is_some_and(|previous| sequence <= previous)
        {
            return Err(contract_error(
                "non_monotonic_sequence",
                format!("sequence {sequence} is not greater than the prior frame"),
            ));
        }
        self.last_sequence = Some(sequence);
        Ok(())
    }
}

pub fn parse_adapter_frame(json: &str) -> Result<AdapterFrame, ContractError> {
    serde_json::from_str(json).map_err(|error| {
        contract_error(
            "invalid_adapter_frame",
            format!("adapter frame does not satisfy the negotiated v1 schema: {error}"),
        )
    })
}

pub fn parse_runner_frame(json: &str) -> Result<RunnerFrame, ContractError> {
    serde_json::from_str(json).map_err(|error| {
        contract_error(
            "invalid_runner_frame",
            format!("runner frame does not satisfy the negotiated v1 schema: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use coven_runtime_spec::Capabilities;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GoldenFixture {
        adapter_id: String,
        offer: ContractOffer,
        #[serde(default)]
        capabilities: Option<Capabilities>,
        session_id: String,
        #[serde(default)]
        cancellation_request_id: Option<String>,
        frames: Vec<serde_json::Value>,
    }

    fn fixture(raw: &str) -> GoldenFixture {
        serde_json::from_str(raw).expect("golden fixture must be valid JSON")
    }

    fn assert_fixture(raw: &str) {
        let fixture = fixture(raw);
        if let Some(expected_capabilities) = fixture.capabilities {
            let actual = crate::harness::built_in_harnesses()
                .into_iter()
                .find(|summary| summary.id == fixture.adapter_id)
                .unwrap_or_else(|| panic!("{} must remain a bundled adapter", fixture.adapter_id));
            assert_eq!(
                actual.capabilities, expected_capabilities,
                "{} golden fixture drifted from its declared built-in capabilities",
                fixture.adapter_id
            );
        }
        negotiate(&ContractOffer::v1(), &fixture.offer)
            .unwrap_or_else(|error| panic!("{} did not negotiate: {error}", fixture.adapter_id));
        let mut session = SessionConformance::new(&fixture.session_id);
        for frame in fixture.frames {
            let type_name = frame["type"].as_str().unwrap_or_default();
            if type_name == "cancellation_acknowledged" {
                session
                    .request_cancellation(
                        fixture
                            .cancellation_request_id
                            .as_deref()
                            .expect("cancellation fixture needs a request id"),
                    )
                    .unwrap();
            }
            session
                .accept(&parse_adapter_frame(&frame.to_string()).unwrap())
                .unwrap();
        }
        session.finish().unwrap();
    }

    #[test]
    fn golden_fixtures_cover_built_ins_and_external_adapter() {
        for raw in [
            include_str!("../tests/fixtures/harness-contract/v1/codex.json"),
            include_str!("../tests/fixtures/harness-contract/v1/claude.json"),
            include_str!("../tests/fixtures/harness-contract/v1/copilot.json"),
            include_str!("../tests/fixtures/harness-contract/v1/coven-code.json"),
            include_str!("../tests/fixtures/harness-contract/v1/external.json"),
        ] {
            assert_fixture(raw);
        }
    }

    #[test]
    fn unknown_optional_extension_and_field_are_forward_compatible() {
        let mut adapter = ContractOffer::v1();
        adapter
            .optional_extensions
            .push("future.telemetry.v9".to_string());
        let negotiated = negotiate(&ContractOffer::v1(), &adapter).unwrap();
        assert!(negotiated.extensions.is_empty());

        let frame = parse_adapter_frame(
            r#"{"type":"output","session_id":"s","sequence":1,"kind":"text","text":"ok","futureField":{"safe":true}}"#,
        )
        .unwrap();
        assert!(matches!(frame, AdapterFrame::Output { .. }));
    }

    #[test]
    fn unsupported_version_and_unknown_required_extension_fail_closed() {
        let mut adapter = ContractOffer::v1();
        adapter.versions = vec!["coven.harness.v2".to_string()];
        assert_eq!(
            negotiate(&ContractOffer::v1(), &adapter).unwrap_err().code,
            "unsupported_contract_version"
        );

        let mut adapter = ContractOffer::v1();
        adapter
            .required_extensions
            .push("future.telemetry.v9".to_string());
        assert_eq!(
            negotiate(&ContractOffer::v1(), &adapter).unwrap_err().code,
            "unknown_required_extension"
        );
    }

    #[test]
    fn missing_required_frame_fields_and_partial_failure_are_explicit() {
        assert_eq!(
            parse_adapter_frame(
                r#"{"type":"output","session_id":"s","sequence":1,"text":"missing kind"}"#
            )
            .unwrap_err()
            .code,
            "invalid_adapter_frame"
        );

        let mut session = SessionConformance::new("s");
        session
            .accept(&parse_adapter_frame(r#"{"type":"ready","session_id":"s"}"#).unwrap())
            .unwrap();
        session
            .accept(&parse_adapter_frame(r#"{"type":"output","session_id":"s","sequence":1,"kind":"text","text":"partial output survives"}"#).unwrap())
            .unwrap();
        session
            .accept(&parse_adapter_frame(r#"{"type":"terminal","session_id":"s","sequence":2,"outcome":"failed","error":{"code":"provider_error","message":"upstream failed"}}"#).unwrap())
            .unwrap();
        assert_eq!(session.finish().unwrap(), TerminalOutcome::Failed);
    }

    #[test]
    fn structured_input_and_cancellation_have_required_session_and_request_ids() {
        let input = parse_runner_frame(
            r#"{"type":"input","session_id":"s","request_id":"turn-1","content":{"type":"text","text":"hello"}}"#,
        )
        .unwrap();
        assert!(matches!(input, RunnerFrame::Input { .. }));
        assert_eq!(
            parse_runner_frame(r#"{"type":"cancel","session_id":"s"}"#)
                .unwrap_err()
                .code,
            "invalid_runner_frame"
        );
    }
}
