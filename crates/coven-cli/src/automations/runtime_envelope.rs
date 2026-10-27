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
//!
//! `r0-read.2` replaces it for scoped reads (coven#857): claude in bare mode
//! with no native tools at all, whose only MCP server is the daemon's scoped
//! read service. Its descriptor also binds the exact Claude Code versions it
//! was proven against, the service's tools and policy, and how the launch is
//! isolated. It is described here so its stream can be classified, but it is
//! not launchable until the daemon's admission for it lands; `r0-read.1`
//! stays launchable until missions move to `r0-read.2`. A descriptor never
//! changes once signed, so `r0-read.1`'s stays byte for byte what it was.

use std::time::Duration;

use serde_json::{json, Value};

use super::authority_projection::AutomationAuthorityConsumerProjection;
use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::types::SideEffectClass;
use super::scoped_read::service::{R0_OPERATION_TIME, SERVER_NAME, TOOLS_VERSION};
use super::scoped_read::{ReadPolicy, R0_READ_POLICY};

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
    /// The harness versions the envelope was proven against. The init event
    /// must report one of them. Empty for an envelope from before the check.
    pub harness_versions: &'static [&'static str],
    /// The scoped read service, when the session's only MCP server is it.
    pub service: Option<ServiceBinding>,
}

/// The scoped read service an envelope's session reaches, and how its launch
/// is isolated.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ServiceBinding {
    /// The MCP server's name. The init event must list exactly this server.
    pub server: &'static str,
    pub tools_version: &'static str,
    pub policy: ReadPolicy,
    pub operation_time: Duration,
    /// The only environment variables the harness is launched with.
    pub environment: &'static [&'static str],
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
    harness_versions: &[],
    service: None,
};

const SCOPED_READ_TOOLS: &[EnvelopeTool] = &[
    EnvelopeTool {
        name: "mcp__coven__read_file",
        capability: "analysis.read",
        side_effect: SideEffectClass::LocalRead,
    },
    EnvelopeTool {
        name: "mcp__coven__list_directory",
        capability: "analysis.read",
        side_effect: SideEffectClass::LocalRead,
    },
    EnvelopeTool {
        name: "mcp__coven__search",
        capability: "analysis.read",
        side_effect: SideEffectClass::LocalRead,
    },
];

/// R0 `analysis.read` on claude through the scoped read service. The launch
/// adds `--mcp-config` with the attempt's channel configuration after these
/// arguments.
pub(crate) const CLAUDE_R0_SCOPED_READ: RuntimeEnvelope = RuntimeEnvelope {
    harness: "claude",
    descriptor_version: "r0-read.2",
    args: &[
        "--bare",
        "--print",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--allowedTools",
        "mcp__coven__read_file,mcp__coven__list_directory,mcp__coven__search",
        "--no-session-persistence",
        "--disable-slash-commands",
    ],
    stream: StreamFormat::ClaudeStreamJson,
    tools: SCOPED_READ_TOOLS,
    permission_mode: "default",
    capabilities: &["analysis.read"],
    // Each was proven with the scoped read service's opt-in claude tests: the
    // init event and every model request carried only the service's tools,
    // and no ambient memory reached the model. Maintainer decision,
    // 2026-10-20 (#857): any other version refuses.
    harness_versions: &["2.1.305"],
    service: Some(ServiceBinding {
        server: SERVER_NAME,
        tools_version: TOOLS_VERSION,
        policy: R0_READ_POLICY,
        operation_time: R0_OPERATION_TIME,
        environment: &[
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            "DISABLE_AUTOUPDATER",
            "HOME",
            "PATH",
        ],
    }),
};

/// Every envelope a binding may pin, for classifying its stream.
const ENVELOPES: &[&RuntimeEnvelope] = &[&CLAUDE_R0_READ, &CLAUDE_R0_SCOPED_READ];
/// The envelopes the daemon launches.
const LAUNCHABLE: &[&RuntimeEnvelope] = &[&CLAUDE_R0_READ];

impl RuntimeEnvelope {
    /// The descriptor the binding's digest covers: everything that decides
    /// what the session can do and how it reports it. An envelope with a
    /// service is described by the v2 profile, which adds the versions, the
    /// service and the launch; v1's shape is unchanged.
    pub(crate) fn descriptor(&self) -> Value {
        let mut descriptor = json!({
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
        });
        if let Some(service) = &self.service {
            let policy = &service.policy;
            descriptor["profile"] = json!("coven.automations.runtime-envelope.v2");
            descriptor["harnessVersions"] = json!(self.harness_versions);
            descriptor["service"] = json!({
                "server": service.server,
                "transport": "coven.scoped-read.channel.v1",
                "tools": service.tools_version,
                "policy": {
                    "fileBytes": policy.file_bytes,
                    "attemptBytes": policy.attempt_bytes,
                    "scanBytes": policy.scan_bytes,
                    "calls": policy.calls,
                    "depth": policy.depth,
                    "pathBytes": policy.path_bytes,
                    "pageEntries": policy.page_entries,
                    "pageMatches": policy.page_matches,
                    "patternBytes": policy.pattern_bytes,
                    "matchBytes": policy.match_bytes,
                    "operationMillis": u64::try_from(service.operation_time.as_millis())
                        .expect("an operation limit fits in u64 milliseconds"),
                },
            });
            descriptor["launch"] = json!({
                "cwd": "empty",
                "home": "isolated",
                "environment": service.environment,
                "credential": "owner-api-key",
            });
        }
        descriptor
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
/// the launchable envelope the projection's runtime binding pins, by
/// descriptor digest and version, for the same harness, holding every
/// capability the projection grants.
pub(crate) fn for_projection(
    projection: &AutomationAuthorityConsumerProjection,
    harness: &str,
) -> Option<&'static RuntimeEnvelope> {
    let digest = projection.runtime.descriptor_digest.value.as_str();
    let envelope = LAUNCHABLE
        .iter()
        .copied()
        .find(|envelope| envelope.descriptor_digest() == digest)?;
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
#[allow(dead_code)]
pub(crate) fn for_grant(harness: &str, capabilities: &[&str]) -> Option<&'static RuntimeEnvelope> {
    exactly(LAUNCHABLE, harness, capabilities)
}

/// The scoped read envelope for a grant of exactly `capabilities` on
/// `harness`. The trusted adapter (coven#857 slice 6) pins this one, so a
/// routine's file access is its granted scopes and nothing else. Missions
/// keep [`for_grant`] until they move to `r0-read.2` in their own change.
pub(crate) fn for_scoped_grant(
    harness: &str,
    capabilities: &[&str],
) -> Option<&'static RuntimeEnvelope> {
    exactly(&[&CLAUDE_R0_SCOPED_READ], harness, capabilities)
        .filter(|envelope| envelope.service.is_some())
}

fn exactly(
    envelopes: &[&'static RuntimeEnvelope],
    harness: &str,
    capabilities: &[&str],
) -> Option<&'static RuntimeEnvelope> {
    let mut wanted: Vec<&str> = capabilities.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    envelopes.iter().copied().find(|envelope| {
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
    fn the_adapter_pins_only_the_scoped_read_envelope() {
        assert_eq!(
            for_scoped_grant("claude", &["analysis.read"]),
            Some(&CLAUDE_R0_SCOPED_READ)
        );
        assert_eq!(
            for_scoped_grant("claude", &["analysis.read", "artifact.write"]),
            None
        );
        assert_eq!(for_scoped_grant("claude", &[]), None);
        assert_eq!(for_scoped_grant("codex", &["analysis.read"]), None);
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

    #[test]
    fn the_r0_read_1_descriptor_is_unchanged() {
        // The mission-commands conformance vectors and every signed binding
        // that pins `r0-read.1` carry this digest.
        assert_eq!(
            CLAUDE_R0_READ.descriptor_digest(),
            "3cace37dc76d6e28453698da0b925c028075d9c2360b0e220e3dc087b1507e5b"
        );
        assert_eq!(
            CLAUDE_R0_READ.descriptor()["profile"],
            "coven.automations.runtime-envelope.v1"
        );
    }

    #[test]
    fn the_scoped_read_envelope_reaches_only_the_service() {
        use crate::automations::scoped_read::service::TOOL_NAMES;

        let envelope = &CLAUDE_R0_SCOPED_READ;
        assert!(envelope.args.contains(&"--bare"));
        assert!(envelope.args.windows(2).any(|pair| pair == ["--tools", ""]));
        assert!(envelope
            .args
            .windows(2)
            .any(|pair| pair == ["--setting-sources", ""]));
        assert!(envelope.args.contains(&"--strict-mcp-config"));
        assert!(envelope.args.contains(&"--disable-slash-commands"));
        assert!(envelope.args.contains(&"--no-session-persistence"));
        // The launch adds the attempt's own MCP configuration.
        assert!(!envelope.args.contains(&"--mcp-config"));
        let names: Vec<String> = TOOL_NAMES
            .iter()
            .map(|tool| format!("mcp__coven__{tool}"))
            .collect();
        let listed: Vec<&str> = envelope.tools.iter().map(|tool| tool.name).collect();
        assert_eq!(listed, names);
        assert!(envelope
            .args
            .windows(2)
            .any(|pair| pair == ["--allowedTools", names.join(",").as_str()]));
        assert!(envelope
            .tools
            .iter()
            .all(|tool| tool.capability == "analysis.read"
                && tool.side_effect == SideEffectClass::LocalRead));

        let descriptor = envelope.descriptor();
        assert_eq!(
            descriptor["profile"],
            "coven.automations.runtime-envelope.v2"
        );
        assert_eq!(descriptor["descriptorVersion"], "r0-read.2");
        assert_eq!(descriptor["harnessVersions"], json!(["2.1.305"]));
        assert_eq!(descriptor["service"]["server"], "coven");
        assert_eq!(descriptor["service"]["tools"], "coven.scoped-read.tools.v1");
        assert_eq!(descriptor["service"]["policy"]["operationMillis"], 5000);
        assert_eq!(descriptor["service"]["policy"]["calls"], 1024);
        assert_eq!(descriptor["launch"]["credential"], "owner-api-key");
        assert_ne!(
            envelope.descriptor_digest(),
            CLAUDE_R0_READ.descriptor_digest()
        );
    }

    #[test]
    fn the_scoped_read_envelope_is_described_but_not_yet_launched() {
        let digest = CLAUDE_R0_SCOPED_READ.descriptor_digest();
        assert_eq!(by_descriptor_digest(&digest), Some(&CLAUDE_R0_SCOPED_READ));
        assert_eq!(
            for_grant("claude", &["analysis.read"]),
            Some(&CLAUDE_R0_READ),
            "missions move to r0-read.2 in their own change"
        );
        let pinned = projection(|binding| {
            binding["runtime"]["descriptorDigest"]["value"] = json!(digest);
            binding["runtime"]["descriptorVersion"] = json!("r0-read.2");
        });
        assert_eq!(for_projection(&pinned, "claude"), None);
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
