//! Additive engine run-budget v1 negotiation and terminal receipts (#1779).
//! Engine sessions and Coven ledger sessions are distinct identities.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Instant;

pub mod input;
pub mod state;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Adapter {
    pub version: u8,
    pub flag: &'static str,
}

#[derive(Deserialize)]
struct Capabilities {
    engine_contract_version: u8,
    run_budget_version: u8,
    flag: String,
    minimum: u32,
    maximum: u32,
    default: u32,
    exhausted_outcome: String,
    one_shot_exhausted_exit_code: i32,
    stream_eof_exit_code: i32,
}

pub struct Negotiated {
    pub(crate) program: String,
}

/// Probe the executable actually selected by the managed engine resolver.
/// No package-version inference, model request or prompt-text fallback.
pub fn negotiate(harness: &str, limit: Option<u32>) -> Result<Negotiated> {
    if let Some(limit) = limit {
        ensure!(limit > 0, "Model-round budget must be positive and finite");
    }
    let declared = crate::harness::configured_harness_specs()?
        .into_iter()
        .find(|spec| spec.id == harness)
        .and_then(|spec| spec.run_budget());
    ensure!(
        declared.is_some(),
        "Harness `{harness}` does not support explicit model-round budgets"
    );
    let resolved = crate::engine::resolve().context("Run-budget engine is unavailable")?;
    let program = resolved
        .path
        .canonicalize()
        .context("Resolved run-budget engine is unavailable")?;
    let (status, bytes) = crate::engine::probe_stdout(
        &program,
        &["--run-budget-capabilities"],
        Instant::now() + crate::engine::PROBE_TIMEOUT,
        16 * 1024,
    )
    .context("Engine run-budget v1 capability is unavailable; update the engine")?;
    ensure!(
        status.success(),
        "Engine run-budget v1 probe failed; update the engine"
    );
    let caps: Capabilities = serde_json::from_slice(&bytes)
        .context("Engine run-budget v1 capability is invalid; update the engine")?;
    ensure!(
        caps.engine_contract_version == 1
            && caps.run_budget_version == 1
            && caps.flag == "--max-turns"
            && caps.minimum == 1
            && caps.maximum == u32::MAX
            && caps.default > 0
            && caps.exhausted_outcome == "turn_budget_exhausted"
            && caps.one_shot_exhausted_exit_code == 1
            && caps.stream_eof_exit_code == 0,
        "Engine does not provide the required run-budget v1 contract; update the engine"
    );
    Ok(Negotiated {
        program: program
            .to_str()
            .context("Run-budget engine path is not UTF-8")?
            .to_owned(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Explicit,
    Agent,
    Configured,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub max_turns: u32,
    pub source: Source,
    pub consumed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Continuation {
    pub session_id: String,
    pub resumable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub run_budget_version: u8,
    pub turn_budget: Applied,
    pub num_turns: u32,
    // Existing one-shot successful engine results omit this field. Error
    // results retain the explicit v1 error envelope.
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub continuation: Continuation,
    #[serde(skip)]
    pub native_session_id: String,
}

pub fn validate_applied(
    receipt: &Receipt,
    requested: Option<u32>,
    claim: Option<&state::Claim>,
) -> Result<()> {
    if let Some(limit) = requested {
        ensure!(receipt.turn_budget.max_turns == limit && receipt.turn_budget.source == Source::Explicit,
            "The engine did not confirm the requested model-round budget; unfinished work is retained.");
    }
    if let Some(claim) = claim {
        ensure!(receipt.native_session_id == claim.engine_session_id,
            "The engine returned a different continuation identity; unfinished work cannot be replayed as a new conversation.");
    }
    Ok(())
}

impl Receipt {
    pub fn from_native(object: &Map<String, Value>) -> Result<Option<Self>> {
        if !object.contains_key("run_budget_version") {
            return Ok(None);
        }
        let mut receipt: Self = serde_json::from_value(Value::Object(object.clone()))
            .context("Invalid engine run-budget result")?;
        let session = object
            .get("session_id")
            .and_then(Value::as_str)
            .context("Engine run-budget result has no session identity")?;
        if !object.contains_key("is_error") {
            ensure!(
                object.get("error").is_none_or(Value::is_null)
                    && object
                        .get("subtype")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s == "success"),
                "Engine error result is missing its explicit error envelope"
            );
        }
        ensure!(
            receipt.run_budget_version == 1
                && receipt.turn_budget.max_turns > 0
                && receipt.turn_budget.consumed <= receipt.turn_budget.max_turns
                && receipt.num_turns == receipt.turn_budget.consumed
                && !session.is_empty()
                && session.len() <= 4096
                && receipt.continuation.session_id == session,
            "Invalid engine run-budget result or continuation identity"
        );
        if receipt.outcome.as_deref() == Some("turn_budget_exhausted") {
            ensure!(
                receipt.is_error && receipt.turn_budget.consumed == receipt.turn_budget.max_turns,
                "Invalid engine exhaustion receipt"
            );
        }
        receipt.native_session_id = session.to_owned();
        Ok(Some(receipt))
    }

    pub fn emit(&self, result: crate::stream_json::RunResult) -> Result<()> {
        let terminal_error = result.is_error || self.is_error;
        let mut value = serde_json::to_value(crate::stream_json::Event::Result(result))?;
        let fields = serde_json::to_value(self)?;
        let target = value.as_object_mut().context("Invalid terminal result")?;
        target.extend(
            fields
                .as_object()
                .context("Invalid budget receipt")?
                .clone(),
        );
        // A successful model receipt cannot erase a later process failure.
        target.insert("is_error".into(), terminal_error.into());
        target.insert(
            "harness_session_id".into(),
            self.native_session_id.clone().into(),
        );
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        use std::io::Write;
        serde_json::to_writer(&mut out, &value)?;
        writeln!(out)?;
        out.flush()?;
        Ok(())
    }
}

/// Ordinary budgeted runs use the same receipt-bearing engine protocol as
/// machine clients, but expose only response text on the human stdout path.
pub struct HumanOutput<W> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: std::io::Write> HumanOutput<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }
}

impl<W: std::io::Write> std::io::Write for HumanOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.pending.drain(..=end).collect();
            let frame: Value = serde_json::from_slice(&line)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            if frame.get("type").and_then(Value::as_str) == Some("text_delta") {
                if let Some(text) = frame.get("text").and_then(Value::as_str) {
                    self.inner.write_all(text.as_bytes())?;
                }
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
