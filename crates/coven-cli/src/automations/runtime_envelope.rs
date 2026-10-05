//! Runtime envelopes for Runtime Authority launches (coven#857).
//!
//! An envelope is the exact launch shape the daemon uses for one kind of grant.
//! Its descriptor's digest is what the execution binding pins as
//! `runtime.descriptorDigest`. The binding is signed with the
//! `dispatch-authority` key and pinned to the attempt, so the terminal observer
//! can trust it to say how the session was launched, and so whether the
//! session's output is a structured stream it may classify. A plain-text
//! launch can print lines that look like stream events; only the pinned
//! envelope says they are the harness's own.
//!
//! v1 has one envelope, by the maintainer's 2026-10-04 decision: an R0
//! `analysis.read` grant on claude. It is launched with only the read tools,
//! in plan (read-only) permission mode, in restricted mode with no MCP
//! servers, emitting stream-json.

use serde_json::{json, Value};

use super::authority_projection::AutomationAuthorityConsumerProjection;
use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::types::SideEffectClass;

/// How an envelope's session reports what it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamFormat {
    /// claude `--output-format stream-json --verbose`, with stderr lines
    /// wrapped as `{"type":"system","subtype":"stderr"}` events.
    ClaudeStreamJson,
}

/// A tool the envelope makes available, and what using it exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EnvelopeTool {
    pub name: &'static str,
    pub capability: &'static str,
    pub side_effect: SideEffectClass,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuntimeEnvelope {
    pub harness: &'static str,
    pub descriptor_version: &'static str,
    /// The harness arguments, placed before the model and `-- <prompt>`.
    pub args: &'static [&'static str],
    pub stream: StreamFormat,
    /// Every tool the session may have. The stream's init event must list no
    /// other.
    pub tools: &'static [EnvelopeTool],
    /// The permission mode the init event must report.
    pub permission_mode: &'static str,
    /// The capabilities a grant must hold for this envelope, and the most it
    /// can exercise.
    pub capabilities: &'static [&'static str],
}

const READ_TOOLS: &[EnvelopeTool] = &[
    EnvelopeTool {
        name: "Glob",
        capability: "analysis.read",
        side_effect: SideEffectClass::LocalRead,
    },
    EnvelopeTool {
        name: "Grep",
        capability: "analysis.read",
        side_effect: SideEffectClass::LocalRead,
    },
    EnvelopeTool {
        name: "Read",
        capability: "analysis.read",
        side_effect: SideEffectClass::LocalRead,
    },
];

/// R0 `analysis.read` on claude.
pub(crate) const CLAUDE_R0_READ: RuntimeEnvelope = RuntimeEnvelope {
    harness: "claude",
    descriptor_version: "r0-read.1",
    args: &[
        "--print",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "Glob,Grep,Read",
        "--permission-mode",
        "plan",
        "--restricted",
        "--strict-mcp-config",
    ],
    stream: StreamFormat::ClaudeStreamJson,
    tools: READ_TOOLS,
    permission_mode: "plan",
    capabilities: &["analysis.read"],
};

const ENVELOPES: &[&RuntimeEnvelope] = &[&CLAUDE_R0_READ];

impl RuntimeEnvelope {
    /// The descriptor the binding's digest covers: everything that decides
    /// what the session can do and how it reports it.
    pub(crate) fn descriptor(&self) -> Value {
        json!({
            "profile": "coven.automations.runtime-envelope.v1",
            "harness": self.harness,
            "descriptorVersion": self.descriptor_version,
            "args": self.args,
            "stream": match self.stream {
                StreamFormat::ClaudeStreamJson => "claude-stream-json",
            },
            "stderr": "wrapped",
            "tools": self.tools.iter().map(|tool| json!({
                "name": tool.name,
                "capability": tool.capability,
                "sideEffect": tool.side_effect,
            })).collect::<Vec<_>>(),
            "permissionMode": self.permission_mode,
            "capabilities": self.capabilities,
        })
    }

    /// The lowercase SHA-256 of the descriptor's JCS text.
    pub(crate) fn descriptor_digest(&self) -> String {
        sha256_hex(&canonicalize(&self.descriptor()).expect("an envelope descriptor is I-JSON"))
    }

    pub(crate) fn tool(&self, name: &str) -> Option<&EnvelopeTool> {
        self.tools.iter().find(|tool| tool.name == name)
    }
}

/// The envelope whose descriptor has `digest`.
pub(crate) fn by_descriptor_digest(digest: &str) -> Option<&'static RuntimeEnvelope> {
    ENVELOPES
        .iter()
        .copied()
        .find(|envelope| envelope.descriptor_digest() == digest)
}

/// The envelope a launch on `harness` may run under for `projection`. That is
/// the envelope the projection's runtime binding pins, by descriptor digest and
/// version, for the same harness, holding every capability the projection
/// grants.
pub(crate) fn for_projection(
    projection: &AutomationAuthorityConsumerProjection,
    harness: &str,
) -> Option<&'static RuntimeEnvelope> {
    let envelope = by_descriptor_digest(projection.runtime.descriptor_digest.value.as_str())?;
    let granted = projection.granted_capabilities.as_slice();
    let grant_matches = granted.len() == envelope.capabilities.len()
        && granted
            .iter()
            .all(|capability| envelope.capabilities.contains(&capability.as_str()));
    (envelope.harness == harness
        && projection.runtime.descriptor_version.as_str() == envelope.descriptor_version
        && grant_matches)
        .then_some(envelope)
}

/// The envelope for a grant of exactly `capabilities` on `harness`, if v1
/// launches one.
// The trusted adapter (coven#857 slice 6) picks the envelope it pins with this.
#[allow(dead_code)]
pub(crate) fn for_grant(harness: &str, capabilities: &[&str]) -> Option<&'static RuntimeEnvelope> {
    let mut wanted: Vec<&str> = capabilities.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    ENVELOPES.iter().copied().find(|envelope| {
        let mut held: Vec<&str> = envelope.capabilities.to_vec();
        held.sort_unstable();
        envelope.harness == harness && held == wanted
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_r0_envelope_is_found_by_its_digest_and_its_grant() {
        let digest = CLAUDE_R0_READ.descriptor_digest();
        assert_eq!(digest.len(), 64);
        assert_eq!(by_descriptor_digest(&digest), Some(&CLAUDE_R0_READ));
        assert_eq!(by_descriptor_digest(&"0".repeat(64)), None);
        assert_eq!(
            for_grant("claude", &["analysis.read"]),
            Some(&CLAUDE_R0_READ)
        );
        // v1 launches nothing broader, and nothing else.
        assert_eq!(
            for_grant("claude", &["analysis.read", "artifact.write"]),
            None
        );
        assert_eq!(for_grant("claude", &[]), None);
        assert_eq!(for_grant("codex", &["analysis.read"]), None);
    }

    #[test]
    fn the_r0_envelope_only_reads() {
        assert!(CLAUDE_R0_READ
            .args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "plan"]));
        assert!(CLAUDE_R0_READ
            .args
            .windows(2)
            .any(|pair| pair == ["--tools", "Glob,Grep,Read"]));
        assert!(CLAUDE_R0_READ.args.contains(&"--restricted"));
        assert!(CLAUDE_R0_READ.args.contains(&"--strict-mcp-config"));
        let listed: Vec<&str> = CLAUDE_R0_READ.tools.iter().map(|tool| tool.name).collect();
        assert_eq!(listed.join(","), "Glob,Grep,Read");
        assert!(CLAUDE_R0_READ
            .tools
            .iter()
            .all(|tool| tool.side_effect == SideEffectClass::LocalRead));
    }

    fn projection(
        edit: impl FnOnce(&mut serde_json::Value),
    ) -> AutomationAuthorityConsumerProjection {
        use crate::automations::contract::authority::test_support::{
            authority_extensions_value, resign_binding,
        };
        use crate::automations::contract::authority::AUTHORITY_EXTENSION_KEY;
        let mut extension = authority_extensions_value()[AUTHORITY_EXTENSION_KEY].clone();
        let binding = &mut extension["executionBinding"];
        binding["runtime"]["descriptorDigest"]["value"] =
            serde_json::json!(CLAUDE_R0_READ.descriptor_digest());
        binding["runtime"]["descriptorVersion"] = serde_json::json!("r0-read.1");
        binding["capabilities"]["granted"] = serde_json::json!(["analysis.read"]);
        edit(binding);
        resign_binding(binding);
        AutomationAuthorityConsumerProjection::from_validated(
            &serde_json::from_value(extension).unwrap(),
        )
    }

    #[test]
    fn a_projection_runs_under_the_envelope_its_binding_pins() {
        assert_eq!(
            for_projection(&projection(|_| {}), "claude"),
            Some(&CLAUDE_R0_READ)
        );
        // Another harness, version, unknown envelope or broader grant has none.
        assert_eq!(for_projection(&projection(|_| {}), "codex"), None);
        let refused: [fn(&mut serde_json::Value); 4] = [
            |binding| binding["runtime"]["descriptorVersion"] = serde_json::json!("r0-read.2"),
            |binding| {
                binding["runtime"]["descriptorDigest"]["value"] = serde_json::json!("0".repeat(64))
            },
            |binding| {
                binding["capabilities"]["granted"] =
                    serde_json::json!(["analysis.read", "artifact.write"])
            },
            |binding| binding["capabilities"]["granted"] = serde_json::json!([]),
        ];
        for edit in refused {
            assert_eq!(for_projection(&projection(edit), "claude"), None);
        }
    }
}
