use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use coven_runtime_spec::{Capabilities, SandboxMapping, StreamArgs};
use serde::{Deserialize, Serialize};

pub const EXTERNAL_ADAPTER_MANIFEST_ENV: &str = "COVEN_HARNESS_ADAPTER_MANIFEST";
pub const EXTERNAL_ADAPTER_DIRS_ENV: &str = "COVEN_HARNESS_ADAPTER_DIRS";
pub const CLAUDE_BYPASS_PERMISSIONS_ENV: &str = "COVEN_CLAUDE_BYPASS_PERMISSIONS";
pub const TRUSTED_ADAPTERS_DIR_NAME: &str = "adapters";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarnessSummary {
    pub id: String,
    pub label: String,
    pub executable: String,
    pub available: bool,
    pub install_hint: String,
    pub capabilities: Capabilities,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessLaunchMode {
    Interactive,
    NonInteractive,
    /// Long-lived stream-json process: stdin reads newline-delimited JSON
    /// messages, stdout writes newline-delimited JSON events. Only
    /// `claude` supports this today (`-p --input-format stream-json
    /// --output-format stream-json --verbose`).
    ///
    /// Capability is enforced at two layers:
    /// - `command_parts_for_harness_with_conversation` (the offline arg
    ///   builder): codex's `stream_args` returns `None`, so the builder
    ///   falls back to non-interactive args. This makes the function
    ///   safe to call standalone.
    /// - `daemon::LiveSessionRuntime::launch_session` (the live runtime):
    ///   explicitly rejects stream-mode launches when
    ///   `harness_supports_stream_mode(harness)` is false, returning a
    ///   structured `500 launch_failed` so the client sees the actual
    ///   constraint instead of a silently-downgraded behavior. The
    ///   chat layer is the only caller that requests Stream today and
    ///   already gates on `harness_supports_stream_mode` before doing so.
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessSpeed {
    Fast,
    Balanced,
    Thorough,
}

impl HarnessSpeed {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "balanced" => Ok(Self::Balanced),
            "thorough" => Ok(Self::Thorough),
            other => {
                anyhow::bail!("invalid speed `{other}`; expected one of: fast, balanced, thorough")
            }
        }
    }

    fn claude_effort(self) -> &'static str {
        match self {
            Self::Fast => "low",
            Self::Balanced => "medium",
            Self::Thorough => "high",
        }
    }
}

/// Sandbox/permission policy requested for a harness run. Maps to each
/// harness's native sandbox flag (codex `--sandbox`, claude `--permission-mode`)
/// so the composer's Access chip is actually enforced rather than advisory.
/// `Full` preserves today's behavior (no restriction); `ReadOnly` locks the
/// harness out of writes/network per its native read-only mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Full,
    ReadOnly,
}

impl Permission {
    /// Parse a CLI value. Accepts `"full"` / `"read-only"` (trimmed,
    /// case-insensitive); bails on anything else.
    pub fn parse(s: &str) -> Result<Permission> {
        match s.trim().to_ascii_lowercase().as_str() {
            "full" => Ok(Permission::Full),
            "read-only" => Ok(Permission::ReadOnly),
            other => {
                anyhow::bail!("invalid permission `{other}`; expected one of: full, read-only")
            }
        }
    }

    /// Canonical string form, echoed back in the stream-json `system.init`
    /// `permission` field so clients (Cave) can confirm acceptance.
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Full => "full",
            Permission::ReadOnly => "read-only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HarnessLaunchOptions<'a> {
    pub model: Option<&'a str>,
    pub think: bool,
    pub speed: Option<HarnessSpeed>,
    /// Sandbox/permission policy. `None` leaves the harness at its default
    /// (equivalent to `Full`); `Some(_)` forwards the harness's native
    /// sandbox flag when the spec declares one. `Option<Permission>` stays
    /// `Copy` because `Permission` is `Copy`.
    pub permission: Option<Permission>,
    /// Additional directories the harness should trust beyond its cwd
    /// (`coven run --add-dir <DIR>`, repeatable). Each entry forwards as
    /// `[add_dir_flag, <dir>]` when the spec declares an add-dir mechanism;
    /// harnesses that declare none make the flag a warned no-op. Blank
    /// entries are skipped. A shared slice keeps the struct `Copy`.
    pub add_dirs: &'a [String],
}

impl<'a> HarnessLaunchOptions<'a> {
    fn normalized_model(self) -> Option<&'a str> {
        self.model.map(str::trim).filter(|m| !m.is_empty())
    }

    pub(crate) fn claude_effort(self) -> Option<&'static str> {
        self.speed
            .map(HarnessSpeed::claude_effort)
            .or_else(|| self.think.then_some("high"))
    }
}

/// Declared capabilities for a configured harness id, consulting built-ins
/// and external adapter manifests alike — a manifest that declares
/// `capabilities.stream` passes the same gates claude does. Unknown ids get
/// the conservative baseline (all off). Falls back to built-ins only if the
/// external manifests fail to load (the launch path will surface that error
/// with full context).
fn declared_capabilities(harness_id: &str) -> Capabilities {
    configured_harness_specs()
        .unwrap_or_else(|_| built_in_harness_specs())
        .into_iter()
        .find(|spec| spec.id == harness_id)
        .map(|spec| spec.capabilities)
        .unwrap_or(Capabilities::BASELINE)
}

/// Whether the harness CLI has a long-lived JSON-streaming mode the daemon
/// can keep alive across chat turns. Claude does (`stream-json`); codex
/// doesn't (only one-shot `codex exec`). Unix kills the stream process tree
/// with process groups; Windows uses a Job Object owned by the daemon.
pub fn harness_supports_stream_mode(harness_id: &str) -> bool {
    declared_capabilities(harness_id).stream
}

/// Hint passed when a chat turn wants to participate in a multi-turn
/// conversation by reusing the underlying harness CLI's session-resume
/// mechanism. Consulted in `NonInteractive` mode (each turn cold-starts
/// claude/codex with `--resume`/`exec resume`) and in `Stream` mode (the
/// long-lived claude process is launched with `--session-id`/`--resume`
/// up front).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationHint {
    /// First turn of a conversation. The harness should create a session
    /// claimed under this id so later turns can resume it.
    Init { id: String },
    /// Subsequent turn. The harness should resume the session at this id and
    /// append the new prompt to its history.
    Resume { id: String },
}

impl ConversationHint {
    pub fn id(&self) -> &str {
        match self {
            ConversationHint::Init { id } | ConversationHint::Resume { id } => id,
        }
    }
}

/// Whether the harness CLI lets the caller pre-assign a session id at launch
/// time (e.g. `claude --session-id <uuid>`). Harnesses that auto-generate
/// session ids (e.g. codex) return `false`; the chat app captures the id from
/// the first turn's output instead. See `docs/chat-persistence.md`.
pub fn harness_supports_preassigned_session_id(harness_id: &str) -> bool {
    declared_capabilities(harness_id).preassigned_session_id
}

pub fn harness_supports_think(harness_id: &str) -> bool {
    declared_capabilities(harness_id).think
}

pub fn harness_supports_speed(harness_id: &str) -> bool {
    declared_capabilities(harness_id).speed
}

// The sandbox/permission mapping is the shared `coven_runtime_spec::SandboxMapping`:
// either a single `--flag value` pair per policy (codex, claude) or a whole
// argv list per policy for harnesses whose permission surface is
// boolean/multi-token flags (e.g. GitHub Copilot CLI's `--allow-all` /
// `--deny-tool …`).
impl Permission {
    /// The shared-spec equivalent, for driving `SandboxMapping::args`.
    fn to_spec(self) -> coven_runtime_spec::Permission {
        match self {
            Permission::Full => coven_runtime_spec::Permission::Full,
            Permission::ReadOnly => coven_runtime_spec::Permission::ReadOnly,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommandSpec {
    pub id: String,
    pub label: String,
    pub executable: String,
    pub interactive_prompt_prefix_args: Vec<String>,
    pub non_interactive_prompt_prefix_args: Vec<String>,
    pub install_hint: String,
    pub source: String,
    pub manifest_path: Option<String>,
    /// CLI flag name to pass a system-prompt string (e.g. `Some("--system-prompt")`
    /// for Claude). `None` means the harness has no such flag and identity
    /// should be injected by prepending a preamble to the prompt instead.
    pub system_prompt_flag: Option<String>,
    /// CLI flag name that carries the user prompt as its VALUE rather than a
    /// trailing positional (e.g. `Some("--prompt")` for Copilot, which
    /// rejects positional prompts outright). When set, `prompt_args` appends
    /// `--flag=<prompt>` instead of the `-- <prompt>` options-terminator form.
    /// `None` (the default) keeps the positional-append behavior.
    pub prompt_flag: Option<String>,
    /// Interactive-mode override for `prompt_flag`, for harnesses whose
    /// interactive entrypoint takes the prompt behind a *different* flag than
    /// the non-interactive one (e.g. Copilot's `--interactive <prompt>` opens
    /// the TUI and runs the prompt, while `--prompt <prompt>` exits after
    /// completion). Only consulted in `HarnessLaunchMode::Interactive`, where
    /// it falls back to `prompt_flag` when unset.
    pub interactive_prompt_flag: Option<String>,
    /// CLI flag name to select a model (e.g. `Some("--model")` for both Codex and
    /// Claude). When set, `coven run --model <ID>` forwards `[flag, <model>]`.
    /// `None` (and no `model_arg_template`) means the harness declares no model
    /// mechanism, so `--model` is a no-op for it (warn, don't error).
    pub model_flag: Option<String>,
    /// Optional argv template for harnesses whose model selection isn't a simple
    /// `--flag <value>` pair (e.g. Codex's `-c model=<value>`). The template is
    /// split on whitespace into argv tokens and the `{model}` placeholder is
    /// substituted in each token (no shell quoting). Takes precedence over
    /// `model_flag` when both are set.
    pub model_arg_template: Option<String>,
    /// How this harness enforces a sandbox/permission policy. `Some(_)` maps
    /// `coven run --permission <full|read-only>` to `[flag, value]`; `None`
    /// means the harness declares no sandbox mechanism, so `--permission` is a
    /// warned no-op for it (mirrors `model_flag`).
    pub sandbox: Option<SandboxMapping>,
    /// CLI flag name that grants the harness access to an additional directory
    /// beyond its cwd (e.g. `Some("--add-dir")` for Codex, Claude, and the
    /// engine). When set, each `coven run --add-dir <DIR>` repeats as
    /// `[flag, <dir>]` ahead of the prompt so granted project roots are real
    /// grants, not advisory prompt text. `None` means the harness declares no
    /// such mechanism, so `--add-dir` is a warned no-op for it (mirrors
    /// `model_flag`).
    pub add_dir_flag: Option<String>,
    /// Behavioral capabilities (stream mode, session pre-assignment, think,
    /// speed). Shared type with the coven-runtimes manifest spec so built-in
    /// harnesses and external adapters declare what they can do through the
    /// same struct instead of hardcoded harness-id checks.
    pub capabilities: Capabilities,
    /// Stream-json launch args (prefix argv plus session pre-assign/resume
    /// flags). Required when `capabilities.stream`; shared type with the
    /// coven-runtimes manifest spec.
    pub stream_args: Option<StreamArgs>,
    /// One-shot non-interactive session-continuity args. This mirrors
    /// `stream_args` for cold-started turns: adapters declare how to initialize
    /// or resume an upstream conversation without Coven hardcoding ids.
    pub continuity_args: Option<ContinuityArgs>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuityArgs {
    #[serde(default, alias = "initPrefixArgs")]
    pub init_prefix_args: Vec<String>,
    #[serde(default, alias = "resumePrefixArgs")]
    pub resume_prefix_args: Vec<String>,
    #[serde(
        default,
        alias = "sessionIdFlag",
        skip_serializing_if = "Option::is_none"
    )]
    pub session_id_flag: Option<String>,
    #[serde(default, alias = "resumeFlag", skip_serializing_if = "Option::is_none")]
    pub resume_flag: Option<String>,
}

impl ContinuityArgs {
    fn session_id_flag(&self) -> Option<&str> {
        self.session_id_flag
            .as_deref()
            .map(str::trim)
            .filter(|flag| !flag.is_empty())
    }

    fn resume_flag(&self) -> Option<&str> {
        self.resume_flag
            .as_deref()
            .map(str::trim)
            .filter(|flag| !flag.is_empty())
    }

    fn has_init_launch(&self) -> bool {
        self.session_id_flag().is_some()
            || self
                .init_prefix_args
                .iter()
                .any(|arg| !arg.trim().is_empty())
    }

    fn has_resume_launch(&self) -> bool {
        self.resume_flag().is_some()
            || self
                .resume_prefix_args
                .iter()
                .any(|arg| !arg.trim().is_empty())
    }
}

impl HarnessCommandSpec {
    /// Whether this harness declares any way to take a model selection.
    /// Adapters that declare none make `coven run --model` a warned no-op.
    pub fn supports_model(&self) -> bool {
        self.model_flag.is_some() || self.model_arg_template.is_some()
    }

    /// Translate a requested model id into argv tokens for this harness,
    /// stripping the `provider/` namespace first. Returns an empty vec when the
    /// harness declares no model mechanism (caller decides whether to warn).
    pub fn model_args(&self, model: &str) -> Vec<String> {
        let normalized = normalize_model_id(model);
        if let Some(template) = self.model_arg_template.as_deref() {
            return expand_model_template(template, normalized);
        }
        if let Some(flag) = self.model_flag.as_deref() {
            return vec![flag.to_string(), normalized.to_string()];
        }
        Vec::new()
    }

    /// Whether this harness declares any way to enforce a sandbox/permission
    /// policy. Adapters that declare none make `coven run --permission` a
    /// warned no-op.
    pub fn supports_permission(&self) -> bool {
        self.sandbox.is_some()
    }

    /// Translate a requested permission policy into argv tokens for this
    /// harness. Returns an empty vec when the harness declares no sandbox
    /// mechanism (caller decides whether to warn).
    pub fn sandbox_args(&self, permission: Permission) -> Vec<String> {
        match &self.sandbox {
            Some(mapping) => mapping.args(permission.to_spec()),
            None => Vec::new(),
        }
    }

    /// Whether this harness declares a native way to trust additional
    /// directories. Adapters that declare none make `coven run --add-dir` a
    /// warned no-op.
    pub fn supports_add_dir(&self) -> bool {
        self.add_dir_flag.is_some()
    }

    /// Translate requested additional directories into argv tokens for this
    /// harness: one `[flag, <dir>]` pair per non-blank entry. Returns an empty
    /// vec when the harness declares no add-dir mechanism (caller decides
    /// whether to warn).
    pub fn add_dir_args(&self, dirs: &[String]) -> Vec<String> {
        let Some(flag) = self.add_dir_flag.as_deref() else {
            return Vec::new();
        };
        dirs.iter()
            .map(|dir| dir.trim())
            .filter(|dir| !dir.is_empty())
            .flat_map(|dir| [flag.to_string(), dir.to_string()])
            .collect()
    }
}

/// Strip a leading `provider/` namespace from a model id. Cave stores and sends
/// namespaced ids (e.g. `openai/gpt-5.5`, `anthropic/claude-…`), but the harness
/// CLIs (`codex --model`, `claude --model`) expect the bare model id. Coven
/// strips the first `provider/` segment before forwarding; a bare id with no
/// slash passes through unchanged. This is the documented contract Cave matches.
pub fn normalize_model_id(model: &str) -> &str {
    match model.split_once('/') {
        Some((provider, rest)) if !provider.is_empty() && !rest.is_empty() => rest,
        _ => model,
    }
}

/// Expand a `model_arg_template` into argv tokens: split on whitespace, then
/// substitute every `{model}` placeholder with `model` in each token. No shell
/// quote interpretation — each whitespace-separated token becomes one argv entry.
fn expand_model_template(template: &str, model: &str) -> Vec<String> {
    template
        .split_whitespace()
        .map(|token| token.replace("{model}", model))
        .collect()
}

impl HarnessCommandSpec {
    pub fn prompt_args(&self, prompt: &str, mode: HarnessLaunchMode) -> Vec<String> {
        let prefix_args = match mode {
            HarnessLaunchMode::Interactive => &self.interactive_prompt_prefix_args,
            HarnessLaunchMode::NonInteractive => &self.non_interactive_prompt_prefix_args,
            // Stream mode bypasses `prompt_args` entirely (no trailing
            // prompt; messages arrive on stdin). Fall back to
            // non-interactive args if a caller somehow lands here.
            HarnessLaunchMode::Stream => &self.non_interactive_prompt_prefix_args,
        };

        // Some harnesses take the prompt as the VALUE of a flag rather than a
        // trailing positional (e.g. Copilot rejects positional prompts
        // outright). For those, append `--flag=<prompt>`. The `=` form keeps
        // a prompt that starts with `-` from being misparsed as a new
        // option, so it stays safe without an options terminator (which
        // would otherwise starve the flag of its value). Interactive mode
        // prefers the dedicated interactive flag when one is declared (e.g.
        // Copilot's `--interactive`), falling back to the shared prompt flag.
        let prompt_flag = match mode {
            HarnessLaunchMode::Interactive => self
                .interactive_prompt_flag
                .as_deref()
                .or(self.prompt_flag.as_deref()),
            _ => self.prompt_flag.as_deref(),
        };
        if let Some(flag) = prompt_flag {
            return prefix_args
                .iter()
                .cloned()
                .chain([format!("{flag}={prompt}")])
                .collect();
        }

        // The prompt is user data: a prompt starting with `-` must reach the
        // harness as the positional argument, not be parsed as flags, so it
        // always rides behind an options terminator.
        prefix_args
            .iter()
            .cloned()
            .chain(["--".to_string(), prompt.to_string()])
            .collect()
    }
}

pub fn built_in_harnesses() -> Vec<HarnessSummary> {
    built_in_harness_specs()
        .into_iter()
        .map(HarnessSummary::from_spec)
        .collect()
}

impl HarnessSummary {
    fn from_spec(spec: HarnessCommandSpec) -> Self {
        Self {
            available: harness_available(&spec.executable),
            id: spec.id,
            label: spec.label,
            executable: spec.executable,
            install_hint: spec.install_hint,
            source: spec.source,
            manifest_path: spec.manifest_path,
            capabilities: spec.capabilities,
        }
    }
}

/// Familiar identity context passed down from `coven run --familiar`.
/// Each harness spec decides how to surface this to the underlying CLI
/// (prompt prefix, `--system-prompt` flag, env var, etc.) so the
/// integration layer is harness-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamiliarContext {
    /// Canonical familiar id (e.g. `"charm"`).
    pub id: String,
    /// Human display name (e.g. `"Charm"`).
    pub display_name: String,
    /// Short role/theme description (e.g. `"Voice, Social, and Presence Familiar"`).
    pub role: Option<String>,
}

impl FamiliarContext {
    /// Render a concise identity preamble suitable for prepending to a prompt
    /// or injecting as a system-prompt block. Kept intentionally short so it
    /// doesn't crowd the actual task.
    pub fn identity_preamble(&self) -> String {
        match &self.role {
            Some(role) => format!(
                "[Identity: You are {name}, a {role}. Respond as {name}, not as the underlying tool.]",
                name = self.display_name,
                role = role,
            ),
            None => format!(
                "[Identity: You are {name}. Respond as {name}, not as the underlying tool.]",
                name = self.display_name,
            ),
        }
    }
}

pub fn built_in_harness_specs() -> Vec<HarnessCommandSpec> {
    vec![
        HarnessCommandSpec {
            id: "codex".to_string(),
            label: "Codex".to_string(),
            executable: "codex".to_string(),
            prompt_flag: None,
            interactive_prompt_flag: None,
            interactive_prompt_prefix_args: Vec::new(),
            non_interactive_prompt_prefix_args: vec![
                "exec".to_string(),
                "--skip-git-repo-check".to_string(),
                "--color".to_string(),
                "never".to_string(),
            ],
            install_hint: "Install Codex with `npm install -g @openai/codex` or `brew install --cask codex`; if it is already installed, make sure `codex` is on PATH and run `codex login` or `codex` once to authenticate, then retry `coven doctor`.".to_string(),
            source: "bundled".to_string(),
            manifest_path: None,
            // Codex has no --system-prompt flag; identity is injected as a
            // bracketed preamble prepended to the prompt.
            system_prompt_flag: None,
            // `codex --model <MODEL>` selects the model. (`-c model="<MODEL>"`
            // is the equivalent override form, available via model_arg_template
            // for adapters that prefer it.)
            model_flag: Some("--model".to_string()),
            model_arg_template: None,
            // `codex exec --sandbox <mode>`: full → danger-full-access,
            // read-only → read-only. Verified against the installed codex CLI.
            sandbox: Some(SandboxMapping::Flag {
                flag: "--sandbox".to_string(),
                full: "danger-full-access".to_string(),
                read_only: "read-only".to_string(),
            }),
            // `codex exec --add-dir <DIR>` (repeatable): additional writable
            // directories alongside the workspace. Verified against the
            // installed codex CLI.
            add_dir_flag: Some("--add-dir".to_string()),
            // One-shot `codex exec` only: no stream-json mode, no session
            // pre-assignment (ids are captured from the first turn's output),
            // no think/speed toggles.
            capabilities: Capabilities::BASELINE,
            stream_args: None,
            continuity_args: Some(ContinuityArgs {
                init_prefix_args: Vec::new(),
                resume_prefix_args: vec![
                    "exec".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "resume".to_string(),
                ],
                session_id_flag: None,
                resume_flag: None,
            }),
        },
        HarnessCommandSpec {
            id: "claude".to_string(),
            label: "Claude Code".to_string(),
            executable: "claude".to_string(),
            prompt_flag: None,
            interactive_prompt_flag: None,
            interactive_prompt_prefix_args: Vec::new(),
            non_interactive_prompt_prefix_args: vec!["--print".to_string()],
            install_hint: "Install Claude Code with `npm install -g @anthropic-ai/claude-code`; if it is already installed, make sure `claude` is on PATH and run `claude doctor` to finish local auth/setup, then retry `coven doctor`.".to_string(),
            source: "bundled".to_string(),
            manifest_path: None,
            system_prompt_flag: Some("--system-prompt".to_string()),
            // `claude --model <model>` accepts an alias or full model id.
            model_flag: Some("--model".to_string()),
            model_arg_template: None,
            // `claude --permission-mode <mode>`: full → bypassPermissions,
            // read-only → plan. Verified against the installed claude CLI.
            sandbox: Some(SandboxMapping::Flag {
                flag: "--permission-mode".to_string(),
                full: "bypassPermissions".to_string(),
                read_only: "plan".to_string(),
            }),
            // `claude --add-dir <directories...>` (repeatable): additional
            // directories tools may access. Verified against the installed
            // claude CLI.
            add_dir_flag: Some("--add-dir".to_string()),
            // Long-lived stream-json mode, `--session-id`/`--resume`
            // pre-assignment, and think/speed via `--effort`. These declared
            // values replace the former `harness_id == "claude"` checks.
            capabilities: Capabilities {
                stream: true,
                preassigned_session_id: true,
                think: true,
                speed: true,
            },
            // The stream-json launch recipe formerly hardcoded in
            // `stream_args()`; declared here so built-ins and manifest
            // adapters travel the same code path.
            stream_args: Some(StreamArgs {
                prefix_args: vec![
                    "--print".to_string(),
                    "--input-format".to_string(),
                    "stream-json".to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                    "--verbose".to_string(),
                ],
                session_id_flag: Some("--session-id".to_string()),
                resume_flag: Some("--resume".to_string()),
            }),
            continuity_args: Some(ContinuityArgs {
                init_prefix_args: vec!["--print".to_string()],
                resume_prefix_args: vec!["--print".to_string()],
                session_id_flag: Some("--session-id".to_string()),
                resume_flag: Some("--resume".to_string()),
            }),
        },
        HarnessCommandSpec {
            id: crate::engine::ENGINE_HARNESS_ID.to_string(),
            label: "Coven Code".to_string(),
            executable: crate::engine::ENGINE_HARNESS_ID.to_string(),
            prompt_flag: None,
            interactive_prompt_flag: None,
            interactive_prompt_prefix_args: Vec::new(),
            non_interactive_prompt_prefix_args: vec!["--print".to_string()],
            install_hint: "Install the Coven engine with `coven engine install`.".to_string(),
            source: "bundled".to_string(),
            manifest_path: None,
            // The engine composes its own base system prompt; append, never replace.
            system_prompt_flag: Some("--append-system-prompt".to_string()),
            model_flag: Some("--model".to_string()),
            model_arg_template: None,
            // kebab-case values — the engine's --permission-mode differs from Claude
            // Code's camelCase bypassPermissions.
            sandbox: Some(SandboxMapping::Flag {
                flag: "--permission-mode".to_string(),
                full: "bypass-permissions".to_string(),
                read_only: "plan".to_string(),
            }),
            // `coven-code --add-dir <DIR>` (repeatable), mirroring Claude
            // Code's flag. Verified against the installed engine binary.
            add_dir_flag: Some("--add-dir".to_string()),
            capabilities: Capabilities {
                stream: true,
                preassigned_session_id: true,
                think: true,
                speed: false,
            },
            stream_args: Some(StreamArgs {
                prefix_args: vec![
                    "--print".to_string(),
                    "--input-format".to_string(),
                    "stream-json".to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                ],
                session_id_flag: Some("--session-id".to_string()),
                resume_flag: Some("--resume".to_string()),
            }),
            continuity_args: Some(ContinuityArgs {
                init_prefix_args: vec!["--print".to_string()],
                resume_prefix_args: vec!["--print".to_string()],
                session_id_flag: Some("--session-id".to_string()),
                resume_flag: Some("--resume".to_string()),
            }),
        },
        HarnessCommandSpec {
            id: "copilot".to_string(),
            label: "Copilot CLI".to_string(),
            executable: "copilot".to_string(),
            // Copilot rejects positional prompts outright: non-interactive
            // one-shots are `--prompt <text>` and interactive-with-prompt is
            // `--interactive <text>`. Both ride the `=` form via
            // `prompt_args`. Verified against the installed copilot CLI.
            prompt_flag: Some("--prompt".to_string()),
            interactive_prompt_flag: Some("--interactive".to_string()),
            interactive_prompt_prefix_args: Vec::new(),
            // Copilot colorizes whenever it sees a TTY, and Coven runs
            // non-interactive launches under a PTY — mirror codex's
            // `--color never` hygiene. Interactive launches keep color.
            non_interactive_prompt_prefix_args: vec!["--no-color".to_string()],
            install_hint: "Install GitHub Copilot CLI with `npm install -g @github/copilot` or `brew install --cask copilot-cli`; if it is already installed, make sure `copilot` is on PATH and run `copilot login` to authenticate, then retry `coven doctor`.".to_string(),
            source: "bundled".to_string(),
            manifest_path: None,
            // Copilot has no system-prompt flag; identity is injected as a
            // bracketed preamble prepended to the prompt.
            system_prompt_flag: None,
            // `copilot --model <MODEL>` selects the model (`auto` lets
            // Copilot pick). Verified against the installed copilot CLI.
            model_flag: Some("--model".to_string()),
            model_arg_template: None,
            // Copilot's permission surface is boolean/multi-token flags, so
            // the argv-list sandbox form applies: full → `--allow-all`
            // (tools, paths, and URLs), read-only → deny file writes and
            // shell outright (deny rules beat every allow rule; reads stay
            // natively allowed and any other tool prompt auto-denies in
            // non-interactive mode). Verified against the installed copilot
            // CLI: `--allow-all` wrote a file, the deny pair blocked it.
            sandbox: Some(SandboxMapping::Args {
                full_args: vec!["--allow-all".to_string()],
                read_only_args: vec![
                    "--deny-tool".to_string(),
                    "write".to_string(),
                    "--deny-tool".to_string(),
                    "shell".to_string(),
                ],
            }),
            // `copilot --add-dir <DIR>` (repeatable): additional directories
            // the harness may access beyond its cwd. Verified against the
            // installed copilot CLI.
            add_dir_flag: Some("--add-dir".to_string()),
            // One-shot `copilot --prompt` only: no stream-json stdin mode.
            // `--session-id <uuid>` pre-assigns the session id on a fresh
            // launch; think/speed map to `--effort` (see
            // `launch_option_args`).
            capabilities: Capabilities {
                stream: false,
                preassigned_session_id: true,
                think: true,
                speed: true,
            },
            stream_args: None,
            continuity_args: Some(ContinuityArgs {
                init_prefix_args: vec!["--no-color".to_string()],
                resume_prefix_args: vec!["--no-color".to_string()],
                // `--session-id` serves both launches: it creates a fresh
                // session under a chosen UUID and resumes an existing one
                // (`--resume` only binds its value as `--resume=<id>`, which
                // the token-pair continuity form can't emit). A resume
                // against a wiped store self-heals into a fresh session with
                // the same id instead of erroring. Verified against the
                // installed copilot CLI.
                session_id_flag: Some("--session-id".to_string()),
                resume_flag: Some("--session-id".to_string()),
            }),
        },
    ]
}

pub fn configured_harness_specs() -> Result<Vec<HarnessCommandSpec>> {
    let mut specs = built_in_harness_specs();
    specs.extend(external_harness_specs()?);
    Ok(specs)
}

pub fn configured_harnesses() -> Result<Vec<HarnessSummary>> {
    Ok(configured_harness_specs()?
        .into_iter()
        .map(HarnessSummary::from_spec)
        .collect())
}

pub fn unsupported_harness_message(harness_id: &str, configured_ids: &[&str]) -> String {
    let configured = if configured_ids.is_empty() {
        "(none)".to_string()
    } else {
        configured_ids.join(", ")
    };
    // A second installable recipe (grok, alongside hermes) makes a single
    // hardcoded "To use Hermes..." sentence wrong for every other recipe;
    // this stays data-driven off the same `known_adapter_manifest` map the
    // install/doctor commands already consult.
    let recipe_hint = if known_adapter_manifest(harness_id).is_some() {
        format!(
            "To use it, run `coven adapter install {harness_id}`, then `coven adapter doctor {harness_id}`."
        )
    } else {
        format!(
            "Known installable adapter recipes: {}.",
            known_adapter_recipe_names().join(", ")
        )
    };
    format!(
        "unsupported harness `{harness_id}`. Configured harnesses: {configured}. \
{recipe_hint} \
For other external harnesses, create a trusted adapter manifest under COVEN_HOME/{TRUSTED_ADAPTERS_DIR_NAME} \
or set {EXTERNAL_ADAPTER_MANIFEST_ENV} / {EXTERNAL_ADAPTER_DIRS_ENV} before starting Coven."
    )
}

fn external_harness_specs() -> Result<Vec<HarnessCommandSpec>> {
    let built_ins = built_in_harness_specs();
    let mut specs = Vec::new();
    let mut ids: HashSet<String> = built_ins.iter().map(|spec| spec.id.clone()).collect();

    for manifest in external_adapter_manifest_sources() {
        for spec in manifest.load_specs(&built_ins)? {
            if !ids.insert(spec.id.clone()) {
                // First manifest wins. A repeated id (the same adapter
                // scaffolded into two configured dirs, say) must not brick
                // every `coven run`: skip the later copy and say so.
                eprintln!(
                    "warning: ignoring external harness adapter `{}` in {}: \
                     the id duplicates an adapter that was already loaded",
                    spec.id,
                    manifest.path().display()
                );
                continue;
            }
            specs.push(spec);
        }
    }
    Ok(specs)
}

#[derive(Debug)]
enum AdapterManifestSource {
    Path(PathBuf),
    TrustedRecipe {
        path: PathBuf,
        manifest: &'static str,
    },
}

impl AdapterManifestSource {
    fn path(&self) -> &Path {
        match self {
            Self::Path(path) | Self::TrustedRecipe { path, .. } => path,
        }
    }

    fn load_specs(&self, built_ins: &[HarnessCommandSpec]) -> Result<Vec<HarnessCommandSpec>> {
        match self {
            Self::Path(path) => load_external_harness_specs(path, built_ins),
            Self::TrustedRecipe { path, manifest } => {
                parse_external_harness_specs(manifest, path, built_ins)
            }
        }
    }
}

fn external_adapter_manifest_sources() -> Vec<AdapterManifestSource> {
    let mut sources = Vec::new();

    if let Some(coven_home) = coven_home_from_process_env() {
        sources.extend(trusted_adapter_manifest_sources(&coven_home));
    }

    if let Some(manifest_path) = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV) {
        sources.push(AdapterManifestSource::Path(PathBuf::from(manifest_path)));
    }

    if let Some(dir_list) = env::var_os(EXTERNAL_ADAPTER_DIRS_ENV) {
        for dir in env::split_paths(&dir_list) {
            sources.extend(
                adapter_manifest_paths_in_dir(&dir)
                    .into_iter()
                    .map(AdapterManifestSource::Path),
            );
        }
    }

    let mut seen = HashSet::new();
    sources
        .into_iter()
        .filter(|source| seen.insert(source.path().to_path_buf()))
        .collect()
}

pub fn trusted_adapter_dir(coven_home: &Path) -> PathBuf {
    coven_home.join(TRUSTED_ADAPTERS_DIR_NAME)
}

pub fn trusted_adapter_manifest_path(coven_home: &Path, adapter_id: &str) -> PathBuf {
    trusted_adapter_dir(coven_home).join(format!("{adapter_id}.json"))
}

fn trusted_adapter_manifest_sources(coven_home: &Path) -> Vec<AdapterManifestSource> {
    known_adapter_recipe_names()
        .iter()
        .filter_map(|adapter_id| {
            let path = trusted_adapter_manifest_path(coven_home, adapter_id);
            let manifest = known_adapter_manifest(adapter_id)?;
            trusted_adapter_manifest_matches_recipe(&path, adapter_id)
                .then_some(AdapterManifestSource::TrustedRecipe { path, manifest })
        })
        .collect()
}

pub fn trusted_adapter_manifest_matches_recipe(path: &Path, adapter_id: &str) -> bool {
    let Some(expected) = known_adapter_manifest(adapter_id) else {
        return false;
    };
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }
    if metadata.len() != expected.len() as u64 {
        return false;
    }
    fs::read_to_string(path).is_ok_and(|actual| actual == expected)
}

fn coven_home_from_process_env() -> Option<PathBuf> {
    crate::paths::coven_home_dir().ok()
}

fn adapter_manifest_paths_in_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .collect();
    paths.sort();
    paths
}

pub fn known_adapter_manifest(adapter_id: &str) -> Option<&'static str> {
    match adapter_id {
        "grok" => Some(GROK_BUILD_ADAPTER_MANIFEST),
        "hermes" => Some(HERMES_ADAPTER_MANIFEST),
        _ => None,
    }
}

// Grok Build's CLI is documented at https://docs.x.ai/build/cli/reference.
// Coven runs it through its `--output-format plain` headless mode (Grok's
// own default): per Grok Build's public source, that mode's stdout carries
// only the final response text — reasoning ("thought") content is dropped
// before it ever reaches stdout, and every other event (errors, compaction
// notices, max-turns) is routed to stderr. That means Coven treats Grok like
// any other one-shot CLI (Codex, Hermes): no protocol translation needed.
//
// Permission caveat: per Grok Build's public source, its headless runner
// never blocks on approvals — any tool call that would prompt is
// auto-cancelled and reported to the model, which continues the turn. So
// with `--permission` omitted, headless Grok stays in its `default` mode
// and silently cancels every non-auto-approved (i.e. mutating) call —
// nothing like Full. The omitted-means-Full convention documented on
// `HarnessLaunchOptions::permission` therefore does not extend to Grok,
// and `docs/harnesses/grok-build.md` tells users to always pass
// `--permission` explicitly. (The flag itself only activates
// `bypassPermissions` and `default`; `plan`/`dontAsk`/`acceptEdits` are
// accepted for compatibility but are settings-file-only, so those modes
// are unreachable from a manifest.)
//
// Keep this as an opt-in trusted recipe until maintainers promote it into
// the bundled harness set.
const GROK_BUILD_ADAPTER_MANIFEST: &str = r#"{
  "adapters": [
    {
      "id": "grok",
      "label": "Grok Build",
      "executable": "grok",
      "interactive_prompt_prefix_args": ["--no-auto-update", "--no-alt-screen", "--output-format", "plain"],
      "non_interactive_prompt_prefix_args": ["--no-auto-update", "--no-alt-screen", "--output-format", "plain"],
      "install_hint": "Install Grok Build with the official installer: `curl -fsSL https://x.ai/cli/install.sh | bash` (macOS/Linux) or `irm https://x.ai/cli/install.ps1 | iex` (Windows PowerShell) — see https://docs.x.ai/build. Make sure `grok` is on PATH and run `grok login` (or set XAI_API_KEY for headless auth), then retry `coven adapter doctor grok`.",
      "system_prompt_flag": "--rules",
      "prompt_flag": "--single",
      "interactive_prompt_flag": "--single",
      "model_flag": "--model",
      "capabilities": {
        "stream": false,
        "preassigned_session_id": true,
        "think": false,
        "speed": false
      },
      "sandbox": {
        "full_args": ["--permission-mode", "bypassPermissions", "--sandbox", "off"],
        "read_only_args": ["--permission-mode", "default", "--sandbox", "read-only"]
      },
      "continuity_args": {
        "init_prefix_args": ["--no-auto-update", "--no-alt-screen", "--output-format", "plain"],
        "resume_prefix_args": ["--no-auto-update", "--no-alt-screen", "--output-format", "plain"],
        "session_id_flag": "--session-id",
        "resume_flag": "--resume"
      }
    }
  ]
}
"#;

const HERMES_ADAPTER_MANIFEST: &str = r#"{
  "adapters": [
    {
      "id": "hermes",
      "label": "Hermes Agent",
      "executable": "hermes",
      "interactive_prompt_prefix_args": ["chat", "--source", "coven", "-q"],
      "non_interactive_prompt_prefix_args": ["chat", "--source", "coven", "-Q", "-q"],
      "install_hint": "Install Hermes Agent, add it to PATH, and complete Hermes setup before using this adapter.",
      "system_prompt_flag": null
    }
  ]
}
"#;

pub fn known_adapter_recipe_names() -> &'static [&'static str] {
    &["grok", "hermes"]
}

fn load_external_harness_specs(
    path: &Path,
    built_ins: &[HarnessCommandSpec],
) -> Result<Vec<HarnessCommandSpec>> {
    let raw = fs::read_to_string(path).map_err(|err| {
        anyhow!(
            "failed to read harness adapter manifest {}: {err}",
            path.display()
        )
    })?;
    parse_external_harness_specs(&raw, path, built_ins)
}

fn parse_external_harness_specs(
    raw: &str,
    path: &Path,
    built_ins: &[HarnessCommandSpec],
) -> Result<Vec<HarnessCommandSpec>> {
    let registry: ExternalHarnessAdapterRegistry = serde_json::from_str(raw).map_err(|err| {
        anyhow!(
            "failed to parse harness adapter manifest {}: {err}",
            path.display()
        )
    })?;
    registry
        .adapters
        .into_iter()
        .map(|adapter| adapter.into_spec(path, built_ins))
        // `Ok(None)` marks an adapter shadowed by a built-in harness: it is
        // skipped (built-in wins) rather than failing the whole registry.
        .filter_map(Result::transpose)
        .collect()
}

#[derive(Debug, Deserialize)]
struct ExternalHarnessAdapterRegistry {
    #[serde(default)]
    adapters: Vec<ExternalHarnessAdapterSpec>,
}

#[derive(Debug, Deserialize)]
struct ExternalHarnessAdapterSpec {
    id: String,
    label: String,
    executable: String,
    #[serde(alias = "interactivePromptPrefixArgs")]
    interactive_prompt_prefix_args: Vec<String>,
    #[serde(alias = "nonInteractivePromptPrefixArgs")]
    non_interactive_prompt_prefix_args: Vec<String>,
    install_hint: String,
    #[serde(default, alias = "systemPromptFlag")]
    system_prompt_flag: Option<String>,
    #[serde(default, alias = "promptFlag")]
    prompt_flag: Option<String>,
    #[serde(default, alias = "interactivePromptFlag")]
    interactive_prompt_flag: Option<String>,
    /// How this adapter takes a model selection. Declare `model_flag` for a
    /// simple `--flag <value>` pair, or `model_arg_template` for anything else
    /// (e.g. `"-c model={model}"`). Omit both and `coven run --model` is a
    /// warned no-op for this adapter rather than an error.
    #[serde(default, alias = "modelFlag")]
    model_flag: Option<String>,
    #[serde(default, alias = "modelArgTemplate")]
    model_arg_template: Option<String>,
    /// Behavioral capabilities. Omitted fields default to the conservative
    /// baseline (all off), so legacy adapters deserialize unchanged.
    #[serde(default)]
    capabilities: Capabilities,
    /// Native sandbox/permission mapping (single-flag or argv-list form).
    /// Omit and `coven run --permission` stays a warned no-op.
    #[serde(default)]
    sandbox: Option<SandboxMapping>,
    /// CLI flag that trusts an additional directory (repeated per dir).
    /// Omit and `coven run --add-dir` stays a warned no-op.
    #[serde(default, alias = "addDirFlag")]
    add_dir_flag: Option<String>,
    /// Stream-json launch args. Required when `capabilities.stream`.
    #[serde(default, alias = "streamArgs")]
    stream_args: Option<StreamArgs>,
    /// One-shot non-interactive continuity args.
    #[serde(default, alias = "continuityArgs")]
    continuity_args: Option<ContinuityArgs>,
}

impl ExternalHarnessAdapterSpec {
    fn into_spec(
        self,
        manifest_path: &Path,
        built_ins: &[HarnessCommandSpec],
    ) -> Result<Option<HarnessCommandSpec>> {
        let id = self.id.trim().to_lowercase();
        if !valid_adapter_id(&id) {
            anyhow::bail!(
                "invalid harness adapter id `{}` in {}; use lowercase letters, digits, '.', '_' or '-'",
                self.id,
                manifest_path.display()
            );
        }
        if built_ins.iter().any(|spec| spec.id == id) {
            // A CLI upgrade can promote an adapter to built-in after a
            // manifest for it was scaffolded (copilot, coven-code). Treating
            // that as fatal bricked every `coven run` — including harnesses
            // the manifest never mentioned — so the built-in wins and the
            // stale manifest is skipped with a warning instead.
            eprintln!(
                "warning: ignoring external harness adapter `{id}` in {}: \
                 this Coven CLI ships `{id}` as a built-in harness. Delete the \
                 manifest to silence this warning.",
                manifest_path.display()
            );
            return Ok(None);
        }
        let executable = self.executable.trim().to_string();
        if executable.is_empty()
            || executable.contains('/')
            || executable.contains('\\')
            || executable.chars().any(char::is_whitespace)
        {
            anyhow::bail!(
                "external harness adapter `{id}` in {} has an invalid executable `{}`",
                manifest_path.display(),
                self.executable
            );
        }
        if self.label.trim().is_empty() {
            anyhow::bail!(
                "external harness adapter `{id}` in {} must include a label",
                manifest_path.display()
            );
        }
        if self.install_hint.trim().is_empty() {
            anyhow::bail!(
                "external harness adapter `{id}` in {} must include an install_hint",
                manifest_path.display()
            );
        }
        // Capability cross-checks, mirroring the shared spec's validation
        // rules (`coven_runtime_spec::validate_adapter`): a declared
        // capability must be launchable, and dead config is rejected.
        match (&self.stream_args, self.capabilities.stream) {
            (None, true) => anyhow::bail!(
                "external harness adapter `{id}` in {} declares `capabilities.stream` \
                 but no `stream_args`",
                manifest_path.display()
            ),
            (Some(_), false) => anyhow::bail!(
                "external harness adapter `{id}` in {} provides `stream_args` but \
                 `capabilities.stream` is false (dead config)",
                manifest_path.display()
            ),
            (Some(args), true) if args.prefix_args.iter().all(|t| t.trim().is_empty()) => {
                anyhow::bail!(
                    "external harness adapter `{id}` in {} declares stream mode but \
                     `stream_args.prefix_args` is empty",
                    manifest_path.display()
                )
            }
            _ => {}
        }
        if let Some(args) = &self.continuity_args {
            if !args.has_init_launch() && !args.has_resume_launch() {
                anyhow::bail!(
                    "external harness adapter `{id}` in {} provides `continuity_args` but \
                     no usable init or resume launch args",
                    manifest_path.display()
                );
            }
            if args.session_id_flag().is_some() && !self.capabilities.preassigned_session_id {
                anyhow::bail!(
                    "external harness adapter `{id}` in {} provides \
                     `continuity_args.session_id_flag` but \
                     `capabilities.preassigned_session_id` is false (dead config)",
                    manifest_path.display()
                );
            }
        }
        let stream_session_id_flag = self
            .stream_args
            .as_ref()
            .and_then(|args| args.session_id_flag.as_deref())
            .map(str::trim)
            .filter(|flag| !flag.is_empty());
        let continuity_session_id_flag = self
            .continuity_args
            .as_ref()
            .and_then(ContinuityArgs::session_id_flag);
        if self.capabilities.preassigned_session_id
            && stream_session_id_flag.is_none()
            && continuity_session_id_flag.is_none()
        {
            anyhow::bail!(
                "external harness adapter `{id}` in {} declares \
                 `capabilities.preassigned_session_id` but no session id flag",
                manifest_path.display()
            );
        }
        match &self.sandbox {
            Some(SandboxMapping::Flag {
                flag,
                full,
                read_only,
            }) if flag.trim().is_empty()
                || full.trim().is_empty()
                || read_only.trim().is_empty() =>
            {
                anyhow::bail!(
                    "external harness adapter `{id}` in {} has an incomplete sandbox \
                     mapping (flag/full/read_only must be non-empty)",
                    manifest_path.display()
                )
            }
            Some(SandboxMapping::Args {
                full_args,
                read_only_args,
            }) if full_args.iter().all(|t| t.trim().is_empty())
                || read_only_args.iter().all(|t| t.trim().is_empty()) =>
            {
                anyhow::bail!(
                    "external harness adapter `{id}` in {} has an incomplete sandbox \
                     mapping (full_args/read_only_args need at least one non-empty token)",
                    manifest_path.display()
                )
            }
            _ => {}
        }
        Ok(Some(HarnessCommandSpec {
            id,
            label: self.label.trim().to_string(),
            executable,
            interactive_prompt_prefix_args: self.interactive_prompt_prefix_args,
            non_interactive_prompt_prefix_args: self.non_interactive_prompt_prefix_args,
            install_hint: self.install_hint.trim().to_string(),
            source: "manifest".to_string(),
            manifest_path: Some(manifest_path.to_string_lossy().into_owned()),
            system_prompt_flag: self
                .system_prompt_flag
                .map(|flag| flag.trim().to_string())
                .filter(|flag| !flag.is_empty()),
            prompt_flag: self
                .prompt_flag
                .map(|flag| flag.trim().to_string())
                .filter(|flag| !flag.is_empty()),
            interactive_prompt_flag: self
                .interactive_prompt_flag
                .map(|flag| flag.trim().to_string())
                .filter(|flag| !flag.is_empty()),
            model_flag: self
                .model_flag
                .map(|flag| flag.trim().to_string())
                .filter(|flag| !flag.is_empty()),
            model_arg_template: self
                .model_arg_template
                .map(|tmpl| tmpl.trim().to_string())
                .filter(|tmpl| !tmpl.is_empty()),
            sandbox: self.sandbox,
            add_dir_flag: self
                .add_dir_flag
                .map(|flag| flag.trim().to_string())
                .filter(|flag| !flag.is_empty()),
            capabilities: self.capabilities,
            stream_args: self.stream_args,
            continuity_args: self.continuity_args,
        }))
    }
}

fn valid_adapter_id(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-')
        })
}

#[cfg(test)]
pub fn command_parts_for_harness(
    harness_id: &str,
    prompt: &str,
    mode: HarnessLaunchMode,
) -> Result<(String, Vec<String>)> {
    command_parts_for_harness_with_conversation(
        harness_id,
        prompt,
        mode,
        None,
        None,
        HarnessLaunchOptions::default(),
    )
}

/// Claude Code prompts before running tool calls that aren't pre-allowlisted.
/// Preserve those prompts by default so untrusted prompt text cannot silently
/// drive tool execution. Operators that run Claude in an explicitly trusted,
/// unattended environment may opt in to bypassing prompts with
/// `COVEN_CLAUDE_BYPASS_PERMISSIONS=1`.
pub fn claude_permission_bypass_enabled() -> bool {
    claude_permission_bypass_enabled_from_value(
        env::var(CLAUDE_BYPASS_PERMISSIONS_ENV).ok().as_deref(),
    )
}

fn with_claude_permission_flags(harness_id: &str, args: Vec<String>) -> Vec<String> {
    with_claude_permission_flags_enabled(harness_id, args, claude_permission_bypass_enabled())
}

fn claude_permission_bypass_enabled_from_value(value: Option<&str>) -> bool {
    value
        .map(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

fn with_claude_permission_flags_enabled(
    harness_id: &str,
    args: Vec<String>,
    bypass_enabled: bool,
) -> Vec<String> {
    if harness_id != "claude" || !bypass_enabled {
        return args;
    }
    let mut flagged = Vec::with_capacity(args.len() + 2);
    flagged.push("--permission-mode".to_string());
    flagged.push("bypassPermissions".to_string());
    flagged.extend(args);
    flagged
}

/// Build a harness command line, optionally injecting session-continuity
/// flags so the harness CLI resumes a prior conversation. Claude uses
/// `--session-id`/`--resume` (works in both NonInteractive and Stream
/// modes); codex uses `codex exec … resume <id>` for resume turns and
/// falls through to a fresh launch for the Init turn (since codex
/// auto-assigns its own session id, which the chat captures from
/// output later). Harnesses with no resume support ignore the hint and
/// fall back to one-shot args. See `docs/chat-persistence.md` for how
/// to extend this for new harnesses.
pub fn command_parts_for_harness_with_conversation(
    harness_id: &str,
    prompt: &str,
    mode: HarnessLaunchMode,
    hint: Option<&ConversationHint>,
    familiar: Option<&FamiliarContext>,
    options: HarnessLaunchOptions<'_>,
) -> Result<(String, Vec<String>)> {
    command_parts_for_harness_with_conversation_inner(
        harness_id, prompt, mode, hint, familiar, options, false,
    )
}

/// Build a one-shot Codex command whose `exec` subcommand is explicitly in
/// its supported JSONL mode. This belongs in command construction, rather
/// than a later argv scan, so user values such as `--model exec` or a prompt
/// literally equal to `--json` cannot be mistaken for syntax.
pub fn command_parts_for_codex_json_with_conversation(
    harness_id: &str,
    prompt: &str,
    mode: HarnessLaunchMode,
    hint: Option<&ConversationHint>,
    familiar: Option<&FamiliarContext>,
    options: HarnessLaunchOptions<'_>,
) -> Result<(String, Vec<String>)> {
    if harness_id != "codex" {
        anyhow::bail!("Codex JSON command construction requested for `{harness_id}`");
    }
    command_parts_for_harness_with_conversation_inner(
        harness_id, prompt, mode, hint, familiar, options, true,
    )
}

#[allow(clippy::too_many_arguments)]
fn command_parts_for_harness_with_conversation_inner(
    harness_id: &str,
    prompt: &str,
    mode: HarnessLaunchMode,
    hint: Option<&ConversationHint>,
    familiar: Option<&FamiliarContext>,
    options: HarnessLaunchOptions<'_>,
    codex_json: bool,
) -> Result<(String, Vec<String>)> {
    let specs = configured_harness_specs()?;
    let configured_ids = specs
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();
    let spec = specs
        .iter()
        .find(|spec| spec.id == harness_id)
        .cloned()
        .ok_or_else(|| anyhow!(unsupported_harness_message(harness_id, &configured_ids)))?;
    let program = spawn_executable_for_platform(&spec.executable);

    // Model selection forwards to the harness's native flag as a normal option
    // ahead of the prompt positional. Adapters that declare no model mechanism
    // yield no args (the run layer warns); a blank/missing model is a no-op.
    let model_args: Vec<String> = match options.normalized_model() {
        Some(m) if !m.is_empty() => spec.model_args(m),
        _ => Vec::new(),
    };
    // Sandbox/permission policy forwards to the harness's native flag ahead of
    // the prompt positional, mirroring model selection. Harnesses that declare
    // no sandbox mechanism yield no args (the run layer warns); `None` leaves
    // the harness at its default (equivalent to `Full`).
    let sandbox_args: Vec<String> = match options.permission {
        Some(p) => spec.sandbox_args(p),
        None => Vec::new(),
    };
    // Additional trusted directories forward as repeated native add-dir flags
    // ahead of the prompt positional, mirroring model selection. Harnesses
    // that declare no add-dir mechanism yield no args (the run layer warns).
    let add_dir_args: Vec<String> = spec.add_dir_args(options.add_dirs);
    let launch_option_args = launch_option_args(harness_id, options);

    // Resolve effective prompt: inject familiar identity preamble when present.
    // Harnesses with a dedicated --system-prompt flag get identity there instead,
    // keeping the task prompt clean.
    let has_system_prompt_flag = spec.system_prompt_flag.is_some();
    let effective_prompt = match familiar {
        Some(f) if !has_system_prompt_flag => {
            format!("{preamble}\n\n{prompt}", preamble = f.identity_preamble())
        }
        _ => prompt.to_string(),
    };

    // Stream mode reads prompts from stdin as JSON messages, so the prompt
    // argument is not appended. The continuity hint (claude resume / init)
    // still maps to a CLI flag; codex falls back to one-shot.
    if mode == HarnessLaunchMode::Stream {
        if let Some(mut args) = stream_args(&spec, hint) {
            // Claude stream mode: inject identity via --system-prompt flag.
            if let (Some(flag), Some(f)) = (spec.system_prompt_flag.as_deref(), familiar) {
                args.insert(0, f.identity_preamble());
                args.insert(0, flag.to_string());
            }
            return Ok((
                program,
                with_claude_permission_flags(
                    harness_id,
                    sanitize_argv_for_platform(prepend_launch_args(
                        &model_args,
                        &sandbox_args,
                        &add_dir_args,
                        &launch_option_args,
                        args,
                    )),
                ),
            ));
        }
        // Harness doesn't support stream: fall through to non-interactive.
        let mut args = spec.prompt_args(&effective_prompt, HarnessLaunchMode::NonInteractive);
        if codex_json {
            add_codex_exec_json_flag(&spec, &mut args)?;
        }
        return Ok((
            program,
            with_claude_permission_flags(
                harness_id,
                sanitize_argv_for_platform(prepend_launch_args(
                    &model_args,
                    &sandbox_args,
                    &add_dir_args,
                    &launch_option_args,
                    args,
                )),
            ),
        ));
    }

    if let Some(hint) = hint {
        if let Some(mut args) = continuity_args(&spec, mode, hint) {
            if codex_json {
                add_codex_exec_json_flag(&spec, &mut args)?;
            }
            // Inject identity via --system-prompt for harnesses that support it.
            if let (Some(flag), Some(f)) = (spec.system_prompt_flag.as_deref(), familiar) {
                args.insert(0, f.identity_preamble());
                args.insert(0, flag.to_string());
            }
            let args = sanitize_argv_for_platform(prepend_launch_args(
                &model_args,
                &sandbox_args,
                &add_dir_args,
                &launch_option_args,
                // The prompt rides behind the harness's prompt flag when it
                // declares one (continuity launches are always
                // non-interactive, so the shared `prompt_flag` applies), or
                // behind `--` for positional-prompt harnesses — user data
                // must not parse as harness flags either way.
                args.into_iter()
                    .chain(match spec.prompt_flag.as_deref() {
                        Some(flag) => vec![format!("{flag}={effective_prompt}")],
                        None => vec!["--".to_string(), effective_prompt],
                    })
                    .collect(),
            ));
            return Ok((program, with_claude_permission_flags(harness_id, args)));
        }
    }

    let mut args = spec.prompt_args(&effective_prompt, mode);
    if codex_json {
        add_codex_exec_json_flag(&spec, &mut args)?;
    }
    // Inject identity via --system-prompt for harnesses that support it,
    // prepending before the prompt args.
    if let (Some(flag), Some(f)) = (spec.system_prompt_flag.as_deref(), familiar) {
        args.insert(0, f.identity_preamble());
        args.insert(0, flag.to_string());
    }
    Ok((
        program,
        with_claude_permission_flags(
            harness_id,
            sanitize_argv_for_platform(prepend_launch_args(
                &model_args,
                &sandbox_args,
                &add_dir_args,
                &launch_option_args,
                args,
            )),
        ),
    ))
}

/// Add `--json` directly after Codex's declared `exec` subcommand while the
/// argument vector still consists only of harness-owned prefix args plus the
/// final prompt. Launch options are prepended later, and the prompt is behind
/// `--`, so neither can affect this structural insertion point.
fn add_codex_exec_json_flag(spec: &HarnessCommandSpec, args: &mut Vec<String>) -> Result<()> {
    let exec_index = spec
        .non_interactive_prompt_prefix_args
        .iter()
        .position(|arg| arg == "exec")
        .context("Codex adapter must declare an `exec` non-interactive subcommand")?;
    if args.get(exec_index).map(String::as_str) != Some("exec") {
        anyhow::bail!(
            "Codex adapter's constructed command no longer contains `exec` at its declared position"
        );
    }
    args.insert(exec_index + 1, "--json".to_string());
    Ok(())
}

fn launch_option_args(harness_id: &str, options: HarnessLaunchOptions<'_>) -> Vec<String> {
    // Claude, the engine, and Copilot share the same `--effort <level>` flag,
    // and Copilot accepts every level `claude_effort` emits (verified against
    // the installed copilot CLI).
    if harness_id != "claude"
        && harness_id != crate::engine::ENGINE_HARNESS_ID
        && harness_id != "copilot"
    {
        return Vec::new();
    }
    options
        .claude_effort()
        .map(|effort| vec!["--effort".to_string(), effort.to_string()])
        .unwrap_or_default()
}

/// Prepend resolved launch argv tokens ahead of `args` (which ends with the
/// prompt positional). Keeps Coven-managed options before the prompt, matching
/// how Cave emits run flags.
fn prepend_launch_args(
    model_args: &[String],
    sandbox_args: &[String],
    add_dir_args: &[String],
    option_args: &[String],
    args: Vec<String>,
) -> Vec<String> {
    if model_args.is_empty()
        && sandbox_args.is_empty()
        && add_dir_args.is_empty()
        && option_args.is_empty()
    {
        return args;
    }
    let mut out = Vec::with_capacity(
        model_args.len() + sandbox_args.len() + add_dir_args.len() + option_args.len() + args.len(),
    );
    out.extend_from_slice(model_args);
    out.extend_from_slice(sandbox_args);
    out.extend_from_slice(add_dir_args);
    out.extend_from_slice(option_args);
    out.extend(args);
    out
}

/// Per-harness translation of stream-mode launch into CLI args. Stream-mode
/// processes are long-lived: stdin is a stream of newline-delimited JSON
/// messages and stdout is a stream of newline-delimited JSON events.
/// Returns `None` for harnesses that don't support stream mode so the
/// caller can fall back to a one-shot launch.
/// On Windows, harness executables often resolve to `.cmd` shims that are
/// invoked through `cmd.exe`. cmd.exe interprets metacharacters like
/// `& | < > ^ % ! "` in arguments even inside double-quoted strings in some
/// invocation paths. Neutralize them by caret-escaping the dangerous
/// characters and wrapping affected arguments in quotes so `.cmd` shims that
/// re-expand `%*` keep the value as data during a second `cmd.exe` parse.
///
/// On non-Windows platforms this is a no-op: the OS exec model passes
/// argv entries as null-terminated byte arrays without shell parsing.
#[cfg(windows)]
pub(crate) fn sanitize_argv_for_platform(args: Vec<String>) -> Vec<String> {
    args.into_iter()
        .map(|arg| escape_cmd_shim_metacharacters(&arg))
        .collect()
}

#[cfg(not(windows))]
pub(crate) fn sanitize_argv_for_platform(args: Vec<String>) -> Vec<String> {
    args
}

#[cfg(any(windows, test))]
fn escape_cmd_shim_metacharacters(arg: &str) -> String {
    // Characters that cmd.exe treats as special when a `.cmd` shim causes argv
    // to be re-parsed. The caller still passes a single argv entry; this only
    // neutralizes characters within that entry.
    const CMD_METACHARACTERS: &[char] = &['&', '|', '<', '>', '^', '%', '!', '"'];
    if !arg.chars().any(|c| CMD_METACHARACTERS.contains(&c)) {
        return arg.to_string();
    }

    let mut escaped = String::with_capacity((arg.len() * 2) + 2);
    escaped.push('"');
    for ch in arg.chars() {
        if CMD_METACHARACTERS.contains(&ch) {
            escaped.push('^');
        }
        escaped.push(ch);
    }
    escaped.push('"');
    escaped
}

/// Stream-json launch args for a harness, from its declared `stream_args`.
/// Returns `None` when the harness doesn't declare a stream-json mode so the
/// caller falls back to non-interactive. The conversation hint maps to the
/// declared session pre-assign/resume flags; a harness that declares no
/// `session_id_flag` (auto-generated ids) or no `resume_flag` simply launches
/// without the corresponding flag and the chat layer captures the id from the
/// first turn's output instead.
fn stream_args(spec: &HarnessCommandSpec, hint: Option<&ConversationHint>) -> Option<Vec<String>> {
    if !spec.capabilities.stream {
        return None;
    }
    let declared = spec.stream_args.as_ref()?;
    let mut args: Vec<String> = declared.prefix_args.clone();
    if let Some(hint) = hint {
        let flag = match hint {
            ConversationHint::Init { .. } => declared.session_id_flag.as_deref(),
            ConversationHint::Resume { .. } => declared.resume_flag.as_deref(),
        };
        if let Some(flag) = flag {
            args.push(flag.to_string());
            args.push(hint.id().to_string());
        }
    }
    Some(args)
}

/// Per-harness translation of a `ConversationHint` into CLI args that precede
/// the prompt. Returns `None` when the harness has no resume support (or when
/// the launch mode doesn't support it) so the caller falls back to defaults.
fn continuity_args(
    spec: &HarnessCommandSpec,
    mode: HarnessLaunchMode,
    hint: &ConversationHint,
) -> Option<Vec<String>> {
    // Continuity only makes sense in non-interactive mode today. Interactive
    // mode launches the harness TUI, which has its own resume picker.
    if mode != HarnessLaunchMode::NonInteractive {
        return None;
    }
    let declared = spec.continuity_args.as_ref()?;
    match hint {
        ConversationHint::Init { .. } => {
            let flag = declared.session_id_flag()?;
            let mut args = declared.init_prefix_args.clone();
            args.push(flag.to_string());
            args.push(hint.id().to_string());
            Some(args)
        }
        ConversationHint::Resume { .. } => {
            if !declared.has_resume_launch() {
                return None;
            }
            let mut args = declared.resume_prefix_args.clone();
            if let Some(flag) = declared.resume_flag() {
                args.push(flag.to_string());
            }
            args.push(hint.id().to_string());
            Some(args)
        }
    }
}

fn executable_exists(executable: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| executable_exists_in_paths(executable, env::split_paths(&paths)))
        .unwrap_or(false)
}

/// Availability check for a harness: most harnesses rely on PATH only, but
/// `coven-code` is also available when the managed engine resolver finds it
/// (e.g. `~/.coven/engine/<ver>/coven-code` not on PATH).
fn harness_available(executable: &str) -> bool {
    if executable == crate::engine::ENGINE_HARNESS_ID {
        // engine::resolve() already checks PATH (and the legacy dir); the
        // extra executable_exists is a cheap safety valve for the rare case
        // where PATH changed between here and an eventual spawn.
        return crate::engine::resolve().is_some() || executable_exists(executable);
    }
    executable_exists(executable)
}

#[cfg(windows)]
pub(crate) fn spawn_executable_for_platform(executable: &str) -> String {
    // For the managed engine, return the absolute path directly so a coven-code
    // binary that isn't on PATH can still be spawned correctly.
    if executable == crate::engine::ENGINE_HARNESS_ID {
        if let Some(r) = crate::engine::resolve() {
            return r.path.to_string_lossy().into_owned();
        }
    }
    env::var_os("PATH")
        .and_then(|paths| {
            resolve_executable_in_paths_for_windows(
                executable,
                env::split_paths(&paths),
                pathext_extensions(),
            )
        })
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| executable.to_string())
}

#[cfg(not(windows))]
pub(crate) fn spawn_executable_for_platform(executable: &str) -> String {
    // For the managed engine, return the absolute path directly so a coven-code
    // binary that isn't on PATH can still be spawned correctly.
    if executable == crate::engine::ENGINE_HARNESS_ID {
        if let Some(r) = crate::engine::resolve() {
            return r.path.to_string_lossy().into_owned();
        }
    }
    executable.to_string()
}

fn executable_exists_in_paths<I>(executable: &str, paths: I) -> bool
where
    I: IntoIterator<Item = PathBuf>,
{
    if executable.contains('/') || executable.contains('\\') {
        return false;
    }

    paths.into_iter().any(|path| {
        executable_candidates(&path, executable)
            .any(|candidate| candidate_is_executable(&candidate))
    })
}

#[cfg(any(windows, test))]
fn resolve_executable_in_paths_for_windows<I>(
    executable: &str,
    paths: I,
    extensions: Vec<String>,
) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    if executable.contains('/') || executable.contains('\\') {
        return None;
    }

    let paths: Vec<PathBuf> = paths.into_iter().collect();
    if Path::new(executable).extension().is_some() {
        return paths
            .into_iter()
            .map(|path| path.join(executable))
            .find(|candidate| candidate.is_file());
    }

    // Honor PATHEXT precedence globally. Cave intentionally prepends npm's
    // shim directory to PATH, but Windows prefers a real .EXE over a later
    // .CMD entry when PATHEXT lists .EXE first. Searching every extension per
    // directory inverted that rule and selected codex.cmd, which cannot carry
    // Cave's large multiline prompt safely through cmd.exe.
    extensions.into_iter().find_map(|extension| {
        let normalized = if extension.starts_with('.') {
            extension
        } else {
            format!(".{extension}")
        };
        paths
            .iter()
            .map(|path| path.join(format!("{executable}{normalized}")))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(unix)]
fn candidate_is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn candidate_is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(windows)]
fn executable_candidates<'a>(
    path: &'a Path,
    executable: &'a str,
) -> impl Iterator<Item = PathBuf> + 'a {
    windows_executable_candidates(path, executable, pathext_extensions()).into_iter()
}

#[cfg(windows)]
fn pathext_extensions() -> Vec<String> {
    env::var_os("PATHEXT")
        .map(|value| {
            env::split_paths(&value)
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()])
}

#[cfg(windows)]
fn windows_executable_candidates(
    path: &Path,
    executable: &str,
    extensions: Vec<String>,
) -> Vec<PathBuf> {
    let base = path.join(executable);
    let has_extension = Path::new(executable).extension().is_some();
    if has_extension {
        return vec![base];
    }
    extensions
        .into_iter()
        .map(move |extension| {
            let normalized = if extension.starts_with('.') {
                extension
            } else {
                format!(".{extension}")
            };
            path.join(format!("{executable}{normalized}"))
        })
        .collect()
}

#[cfg(not(windows))]
fn executable_candidates<'a>(
    path: &'a Path,
    executable: &'a str,
) -> impl Iterator<Item = PathBuf> + 'a {
    std::iter::once(path.join(executable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn restore_adapter_manifest_env(previous: Option<std::ffi::OsString>) {
        match previous {
            Some(value) => env::set_var(EXTERNAL_ADAPTER_MANIFEST_ENV, value),
            None => env::remove_var(EXTERNAL_ADAPTER_MANIFEST_ENV),
        }
    }

    fn restore_adapter_dirs_env(previous: Option<std::ffi::OsString>) {
        match previous {
            Some(value) => env::set_var(EXTERNAL_ADAPTER_DIRS_ENV, value),
            None => env::remove_var(EXTERNAL_ADAPTER_DIRS_ENV),
        }
    }

    fn restore_env_var(name: &str, previous: Option<std::ffi::OsString>) {
        match previous {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = env::var_os(name);
            env::set_var(name, value);
            Self { name, previous }
        }

        fn remove(name: &'static str) -> Self {
            let previous = env::var_os(name);
            env::remove_var(name);
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            restore_env_var(self.name, self.previous.clone());
        }
    }

    #[test]
    fn executable_exists_in_paths_finds_matching_file() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let executable_name = if cfg!(windows) { "codex.exe" } else { "codex" };
        let executable = temp_dir.path().join(executable_name);
        fs::write(&executable, "")?;
        make_executable(&executable)?;

        assert!(executable_exists_in_paths(
            "codex",
            vec![temp_dir.path().to_path_buf()]
        ));
        Ok(())
    }

    #[test]
    fn executable_exists_in_paths_returns_false_when_missing() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        assert!(!executable_exists_in_paths(
            "claude",
            vec![temp_dir.path().to_path_buf()]
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn executable_exists_in_paths_rejects_non_executable_file() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        fs::write(temp_dir.path().join("codex"), "")?;

        assert!(!executable_exists_in_paths(
            "codex",
            vec![temp_dir.path().to_path_buf()]
        ));
        Ok(())
    }

    #[test]
    fn executable_exists_in_paths_rejects_paths() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let executable = temp_dir.path().join("codex");
        fs::write(&executable, "")?;
        make_executable(&executable)?;

        assert!(!executable_exists_in_paths(
            temp_dir.path().join("codex").to_string_lossy().as_ref(),
            vec![temp_dir.path().to_path_buf()]
        ));
        Ok(())
    }

    #[test]
    fn windows_spawn_resolution_prefers_cmd_shim_over_extensionless_npm_shim() -> anyhow::Result<()>
    {
        let temp_dir = tempfile::tempdir()?;
        fs::write(temp_dir.path().join("codex"), "#!/bin/sh\n")?;
        fs::write(temp_dir.path().join("codex.cmd"), "@echo off\r\n")?;

        let resolved = resolve_executable_in_paths_for_windows(
            "codex",
            vec![temp_dir.path().to_path_buf()],
            vec![".cmd".to_string(), ".exe".to_string()],
        )
        .expect("codex.cmd should be selected");

        assert_eq!(resolved, temp_dir.path().join("codex.cmd"));
        Ok(())
    }

    #[test]
    fn windows_spawn_resolution_honors_pathext_across_path_directories() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let npm_bin = temp_dir.path().join("npm");
        let app_bin = temp_dir.path().join("app");
        fs::create_dir_all(&npm_bin)?;
        fs::create_dir_all(&app_bin)?;
        fs::write(npm_bin.join("codex.cmd"), "@echo off\r\n")?;
        // Match the simulated PATHEXT spelling exactly so the Windows
        // algorithm can be tested on a case-sensitive Unix filesystem.
        fs::write(app_bin.join("codex.EXE"), b"")?;

        let resolved = resolve_executable_in_paths_for_windows(
            "codex",
            vec![npm_bin, app_bin.clone()],
            vec![".EXE".to_string(), ".CMD".to_string()],
        )
        .expect("codex.exe should win PATHEXT precedence");

        assert_eq!(
            resolved.to_string_lossy().to_ascii_lowercase(),
            app_bin
                .join("codex.exe")
                .to_string_lossy()
                .to_ascii_lowercase()
        );
        Ok(())
    }

    #[test]
    fn cmd_shim_metacharacters_are_caret_escaped_and_wrapped() {
        assert_eq!(escape_cmd_shim_metacharacters("safe prompt"), "safe prompt");

        let escaped = escape_cmd_shim_metacharacters(r#"a&b|c<d>e^f%g!h"i"#);

        assert_eq!(escaped, r#""a^&b^|c^<d^>e^^f^%g^!h^"i""#);
        assert!(escaped.starts_with('"'));
        assert!(escaped.ends_with('"'));
    }

    #[test]
    fn built_in_harnesses_list_bundled_adapters_in_order() {
        let harnesses = built_in_harnesses();

        assert_eq!(harnesses.len(), 4);
        assert_eq!(harnesses[0].id, "codex");
        assert_eq!(harnesses[0].label, "Codex");
        assert_eq!(harnesses[0].executable, "codex");
        assert_eq!(harnesses[1].id, "claude");
        assert_eq!(harnesses[1].label, "Claude Code");
        assert_eq!(harnesses[1].executable, "claude");
        assert_eq!(harnesses[2].id, "coven-code");
        assert_eq!(harnesses[2].label, "Coven Code");
        assert_eq!(harnesses[2].executable, "coven-code");
        assert_eq!(harnesses[3].id, "copilot");
        assert_eq!(harnesses[3].label, "Copilot CLI");
        assert_eq!(harnesses[3].executable, "copilot");
    }

    #[test]
    fn built_in_harnesses_include_first_run_recovery_commands() {
        let harnesses = built_in_harnesses();
        let codex = harnesses
            .iter()
            .find(|harness| harness.id == "codex")
            .expect("codex harness should exist");
        let claude = harnesses
            .iter()
            .find(|harness| harness.id == "claude")
            .expect("claude harness should exist");
        let copilot = harnesses
            .iter()
            .find(|harness| harness.id == "copilot")
            .expect("copilot harness should exist");

        assert!(codex.install_hint.contains("npm install -g @openai/codex"));
        assert!(codex.install_hint.contains("brew install --cask codex"));
        assert!(codex.install_hint.contains("codex"));
        assert!(codex.install_hint.contains("PATH"));

        assert!(claude
            .install_hint
            .contains("npm install -g @anthropic-ai/claude-code"));
        assert!(claude.install_hint.contains("claude doctor"));
        assert!(claude.install_hint.contains("claude"));
        assert!(claude.install_hint.contains("PATH"));

        assert!(copilot
            .install_hint
            .contains("npm install -g @github/copilot"));
        assert!(copilot
            .install_hint
            .contains("brew install --cask copilot-cli"));
        assert!(copilot.install_hint.contains("copilot login"));
        assert!(copilot.install_hint.contains("PATH"));
    }

    #[test]
    fn command_parts_for_known_harnesses_append_interactive_prompt() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        assert_eq!(
            command_parts_for_harness("codex", "fix tests", HarnessLaunchMode::Interactive)?,
            (
                "codex".to_string(),
                vec!["--".to_string(), "fix tests".to_string()]
            )
        );
        assert_eq!(
            command_parts_for_harness("claude", "polish ui", HarnessLaunchMode::Interactive)?,
            (
                "claude".to_string(),
                vec!["--".to_string(), "polish ui".to_string()]
            )
        );
        Ok(())
    }

    #[test]
    fn command_parts_for_known_harnesses_use_noninteractive_entrypoints() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        assert_eq!(
            command_parts_for_harness("codex", "fix tests", HarnessLaunchMode::NonInteractive)?,
            (
                "codex".to_string(),
                vec![
                    "exec".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "--".to_string(),
                    "fix tests".to_string(),
                ]
            )
        );
        assert_eq!(
            command_parts_for_harness("claude", "polish ui", HarnessLaunchMode::NonInteractive)?,
            (
                "claude".to_string(),
                vec![
                    "--print".to_string(),
                    "--".to_string(),
                    "polish ui".to_string()
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn dash_prefixed_prompts_stay_positional_behind_double_dash() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        // A prompt starting with `-` must never parse as harness flags.
        for mode in [
            HarnessLaunchMode::Interactive,
            HarnessLaunchMode::NonInteractive,
        ] {
            for harness in ["codex", "claude"] {
                let (_, args) = command_parts_for_harness(harness, "--version; rm -rf /", mode)?;
                let separator = args
                    .iter()
                    .position(|a| a == "--")
                    .expect("args must contain the options terminator");
                assert_eq!(args[separator + 1], "--version; rm -rf /");
                assert_eq!(
                    args.len(),
                    separator + 2,
                    "prompt must be the final argv entry"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn command_spec_supports_prefix_args_for_future_harnesses() {
        let spec = HarnessCommandSpec {
            id: "future".to_string(),
            label: "Future Harness".to_string(),
            executable: "future".to_string(),
            prompt_flag: None,
            interactive_prompt_flag: None,
            interactive_prompt_prefix_args: vec!["chat".to_string()],
            non_interactive_prompt_prefix_args: vec!["exec".to_string(), "-q".to_string()],
            install_hint: "Install the future harness.".to_string(),
            source: "manifest".to_string(),
            manifest_path: None,
            system_prompt_flag: None,
            model_flag: None,
            model_arg_template: None,
            sandbox: None,
            add_dir_flag: None,
            capabilities: Capabilities::BASELINE,
            stream_args: None,
            continuity_args: None,
        };

        assert_eq!(
            spec.prompt_args("hello", HarnessLaunchMode::Interactive),
            vec!["chat".to_string(), "--".to_string(), "hello".to_string()]
        );
        assert_eq!(
            spec.prompt_args("hello", HarnessLaunchMode::NonInteractive),
            vec![
                "exec".to_string(),
                "-q".to_string(),
                "--".to_string(),
                "hello".to_string()
            ]
        );
    }

    #[test]
    fn command_parts_reject_unknown_harnesses() {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        assert!(
            command_parts_for_harness("shell", "hello", HarnessLaunchMode::Interactive)
                .unwrap_err()
                .to_string()
                .contains("unsupported harness")
        );
        assert!(
            command_parts_for_harness("hermes", "hello", HarnessLaunchMode::Interactive)
                .unwrap_err()
                .to_string()
                .contains("unsupported harness")
        );
    }

    #[test]
    fn unsupported_harness_message_points_to_external_adapter_manifest() {
        let message = unsupported_harness_message("hermes", &["codex", "claude"]);

        assert!(message.contains("unsupported harness `hermes`"));
        assert!(message.contains("Configured harnesses: codex, claude"));
        assert!(message.contains(EXTERNAL_ADAPTER_MANIFEST_ENV));
        assert!(message.contains(EXTERNAL_ADAPTER_DIRS_ENV));
        assert!(message.contains("coven adapter doctor hermes"));
    }

    #[test]
    fn unsupported_grok_message_points_to_trusted_recipe() {
        let message = unsupported_harness_message("grok", &["codex", "claude"]);

        assert!(message.contains("coven adapter install grok"));
        assert!(message.contains("coven adapter doctor grok"));
    }

    #[test]
    fn unsupported_message_for_an_unknown_harness_lists_every_recipe() {
        let message = unsupported_harness_message("not-a-real-harness", &["codex"]);

        assert!(message.contains("Known installable adapter recipes: grok, hermes."));
    }

    #[test]
    fn grok_recipe_matches_headless_cli_contract() -> anyhow::Result<()> {
        let specs = parse_external_harness_specs(
            GROK_BUILD_ADAPTER_MANIFEST,
            Path::new("grok.json"),
            &built_in_harness_specs(),
        )?;
        let grok = specs
            .iter()
            .find(|spec| spec.id == "grok")
            .expect("Grok Build recipe should parse");

        assert_eq!(grok.label, "Grok Build");
        assert_eq!(grok.executable, "grok");
        assert_eq!(grok.prompt_flag.as_deref(), Some("--single"));
        assert_eq!(grok.interactive_prompt_flag.as_deref(), Some("--single"));
        assert_eq!(grok.system_prompt_flag.as_deref(), Some("--rules"));
        assert_eq!(grok.model_args("xai/grok-build"), ["--model", "grok-build"]);
        assert_eq!(
            grok.prompt_args("fix tests", HarnessLaunchMode::NonInteractive),
            [
                "--no-auto-update",
                "--no-alt-screen",
                "--output-format",
                "plain",
                "--single=fix tests",
            ]
        );
        assert_eq!(
            grok.sandbox_args(Permission::Full),
            ["--permission-mode", "bypassPermissions", "--sandbox", "off",]
        );
        assert_eq!(
            grok.sandbox_args(Permission::ReadOnly),
            ["--permission-mode", "default", "--sandbox", "read-only"]
        );
        assert!(grok.capabilities.preassigned_session_id);
        assert!(!grok.capabilities.stream);

        let session_id = "11111111-2222-4333-8444-555555555555";
        assert_eq!(
            continuity_args(
                grok,
                HarnessLaunchMode::NonInteractive,
                &ConversationHint::Init {
                    id: session_id.to_string(),
                },
            ),
            Some(vec![
                "--no-auto-update".to_string(),
                "--no-alt-screen".to_string(),
                "--output-format".to_string(),
                "plain".to_string(),
                "--session-id".to_string(),
                session_id.to_string(),
            ])
        );
        assert_eq!(
            continuity_args(
                grok,
                HarnessLaunchMode::NonInteractive,
                &ConversationHint::Resume {
                    id: session_id.to_string(),
                },
            ),
            Some(vec![
                "--no-auto-update".to_string(),
                "--no-alt-screen".to_string(),
                "--output-format".to_string(),
                "plain".to_string(),
                "--resume".to_string(),
                session_id.to_string(),
            ])
        );
        Ok(())
    }

    #[test]
    fn installed_grok_recipe_constructs_complete_launch_argv() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let coven_home = temp_dir.path().join("coven-home");
        let adapter_dir = trusted_adapter_dir(&coven_home);
        fs::create_dir_all(&adapter_dir)?;
        fs::write(
            trusted_adapter_manifest_path(&coven_home, "grok"),
            GROK_BUILD_ADAPTER_MANIFEST,
        )?;

        let _guard = env_lock().lock().unwrap();
        let _manifest_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let _dirs_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_DIRS_ENV);
        let _coven_home_guard = EnvVarGuard::set("COVEN_HOME", &coven_home);
        let familiar = FamiliarContext {
            id: "charm".to_string(),
            display_name: "Charm".to_string(),
            role: None,
        };
        let conversation = ConversationHint::Init {
            id: "11111111-2222-4333-8444-555555555555".to_string(),
        };

        let parts = command_parts_for_harness_with_conversation(
            "grok",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            Some(&conversation),
            Some(&familiar),
            HarnessLaunchOptions {
                model: Some("xai/grok-build"),
                permission: Some(Permission::ReadOnly),
                ..Default::default()
            },
        )?;

        assert_eq!(
            parts,
            (
                "grok".to_string(),
                vec![
                    "--model".to_string(),
                    "grok-build".to_string(),
                    "--permission-mode".to_string(),
                    "default".to_string(),
                    "--sandbox".to_string(),
                    "read-only".to_string(),
                    "--rules".to_string(),
                    familiar.identity_preamble(),
                    "--no-auto-update".to_string(),
                    "--no-alt-screen".to_string(),
                    "--output-format".to_string(),
                    "plain".to_string(),
                    "--session-id".to_string(),
                    conversation.id().to_string(),
                    "--single=fix tests".to_string(),
                ],
            )
        );
        Ok(())
    }

    /// A `--permission`-omitted launch must not silently invent a sandbox
    /// flag for grok: per the recipe's own doc comment, Grok's headless
    /// default has not been verified as non-blocking, so this documents
    /// today's actual behavior (no sandbox/permission-mode flag at all)
    /// rather than assume it's safe.
    #[test]
    fn installed_grok_recipe_omits_sandbox_flags_when_permission_is_unset() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let coven_home = temp_dir.path().join("coven-home");
        let adapter_dir = trusted_adapter_dir(&coven_home);
        fs::create_dir_all(&adapter_dir)?;
        fs::write(
            trusted_adapter_manifest_path(&coven_home, "grok"),
            GROK_BUILD_ADAPTER_MANIFEST,
        )?;

        let _guard = env_lock().lock().unwrap();
        let _manifest_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let _dirs_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_DIRS_ENV);
        let _coven_home_guard = EnvVarGuard::set("COVEN_HOME", &coven_home);

        let (_, args) = command_parts_for_harness_with_conversation(
            "grok",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions::default(),
        )?;

        assert!(!args.contains(&"--permission-mode".to_string()));
        assert!(!args.contains(&"--sandbox".to_string()));
        Ok(())
    }

    #[test]
    fn external_manifest_can_register_hermes_without_making_it_built_in() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manifest = temp_dir.path().join("adapters.json");
        fs::write(
            &manifest,
            r#"{
              "adapters": [
                {
                  "id": "hermes",
                  "label": "Hermes Agent",
                  "executable": "hermes",
                  "interactive_prompt_prefix_args": ["chat", "--source", "coven", "-q"],
                  "non_interactive_prompt_prefix_args": ["chat", "--source", "coven", "-Q", "-q"],
                  "install_hint": "Install Hermes Agent and configure it before using this adapter.",
                  "system_prompt_flag": null
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        env::set_var(EXTERNAL_ADAPTER_MANIFEST_ENV, &manifest);
        let parts =
            command_parts_for_harness("hermes", "audit repo", HarnessLaunchMode::NonInteractive);
        restore_adapter_manifest_env(previous);

        assert_eq!(
            parts?,
            (
                "hermes".to_string(),
                vec![
                    "chat".to_string(),
                    "--source".to_string(),
                    "coven".to_string(),
                    "-Q".to_string(),
                    "-q".to_string(),
                    "--".to_string(),
                    "audit repo".to_string(),
                ]
            )
        );
        assert!(!built_in_harnesses()
            .iter()
            .any(|harness| harness.id == "hermes"));
        Ok(())
    }

    /// A streaming manifest adapter used by the capability-loader acceptance
    /// tests: declares stream mode, session pre-assignment, and a single-flag
    /// sandbox mapping — everything that used to require `id == "claude"`.
    const STREAMY_ADAPTER_MANIFEST: &str = r#"{
      "adapters": [
        {
          "id": "streamy",
          "label": "Streamy",
          "executable": "streamy",
          "interactive_prompt_prefix_args": [],
          "non_interactive_prompt_prefix_args": ["--print"],
          "install_hint": "Install streamy and add it to PATH.",
          "capabilities": { "stream": true, "preassigned_session_id": true },
          "sandbox": { "flag": "--permission-mode", "full": "bypass", "read_only": "plan" },
          "stream_args": {
            "prefix_args": ["--print", "--input-format", "stream-json", "--output-format", "stream-json"],
            "session_id_flag": "--session-id",
            "resume_flag": "--resume"
          }
        }
      ]
    }"#;

    /// The acceptance test for the whole coven-runtimes integration
    /// (integration.md): a manifest that declares `capabilities.stream` +
    /// `stream_args` is launched in stream mode — with its sandbox mapping
    /// applied — without any core edit.
    #[test]
    fn manifest_streaming_adapter_launches_in_stream_mode() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manifest = temp_dir.path().join("streamy.json");
        fs::write(&manifest, STREAMY_ADAPTER_MANIFEST)?;

        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let previous = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        env::set_var(EXTERNAL_ADAPTER_MANIFEST_ENV, &manifest);
        let supports_stream = harness_supports_stream_mode("streamy");
        let supports_session = harness_supports_preassigned_session_id("streamy");
        let parts = command_parts_for_harness_with_conversation(
            "streamy",
            "hello",
            HarnessLaunchMode::Stream,
            Some(&ConversationHint::Init {
                id: "abc-123".to_string(),
            }),
            None,
            HarnessLaunchOptions {
                permission: Some(Permission::ReadOnly),
                ..Default::default()
            },
        );
        restore_adapter_manifest_env(previous);

        assert!(supports_stream, "declared stream capability must gate in");
        assert!(supports_session);
        let (program, args) = parts?;
        assert_eq!(program, "streamy");
        assert_eq!(
            args,
            vec![
                "--permission-mode".to_string(),
                "plan".to_string(),
                "--print".to_string(),
                "--input-format".to_string(),
                "stream-json".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--session-id".to_string(),
                "abc-123".to_string(),
            ]
        );
        Ok(())
    }

    /// A manifest adapter can declare flag-carried prompts (`promptFlag`,
    /// with an optional interactive-mode override), and argv construction
    /// binds the prompt via the `=` form in the matching mode.
    #[test]
    fn manifest_prompt_flags_bind_prompt_per_mode() -> anyhow::Result<()> {
        let raw = r#"{
          "adapters": [
            {
              "id": "copi",
              "label": "Copi",
              "executable": "copi",
              "interactive_prompt_prefix_args": [],
              "non_interactive_prompt_prefix_args": ["--quiet"],
              "install_hint": "Install copi.",
              "promptFlag": "--prompt",
              "interactivePromptFlag": "--interactive"
            }
          ]
        }"#;
        let specs =
            parse_external_harness_specs(raw, Path::new("copi.json"), &built_in_harness_specs())?;
        let copi = &specs[0];
        assert_eq!(copi.prompt_flag.as_deref(), Some("--prompt"));
        assert_eq!(
            copi.interactive_prompt_flag.as_deref(),
            Some("--interactive")
        );
        assert_eq!(
            copi.prompt_args("fix tests", HarnessLaunchMode::NonInteractive),
            vec!["--quiet".to_string(), "--prompt=fix tests".to_string()]
        );
        assert_eq!(
            copi.prompt_args("fix tests", HarnessLaunchMode::Interactive),
            vec!["--interactive=fix tests".to_string()]
        );

        // Without the interactive override, both modes share `promptFlag`.
        let shared = r#"{
          "adapters": [
            {
              "id": "copi",
              "label": "Copi",
              "executable": "copi",
              "interactive_prompt_prefix_args": [],
              "non_interactive_prompt_prefix_args": [],
              "install_hint": "Install copi.",
              "promptFlag": "-q"
            }
          ]
        }"#;
        let specs = parse_external_harness_specs(
            shared,
            Path::new("copi.json"),
            &built_in_harness_specs(),
        )?;
        assert_eq!(
            specs[0].prompt_args("hello", HarnessLaunchMode::Interactive),
            vec!["-q=hello".to_string()]
        );
        Ok(())
    }

    /// The argv-list sandbox form (GitHub Copilot CLI shape) maps
    /// `--permission` through for a manifest adapter.
    #[test]
    fn manifest_argv_list_sandbox_maps_permission() -> anyhow::Result<()> {
        let raw = r#"{
          "adapters": [
            {
              "id": "copi",
              "label": "Copi",
              "executable": "copi",
              "interactive_prompt_prefix_args": ["-i"],
              "non_interactive_prompt_prefix_args": ["-s", "-p"],
              "install_hint": "Install copi.",
              "sandbox": {
                "full_args": ["--allow-all"],
                "read_only_args": ["--deny-tool", "write"]
              }
            }
          ]
        }"#;
        let specs =
            parse_external_harness_specs(raw, Path::new("copi.json"), &built_in_harness_specs())?;
        let copi = &specs[0];
        assert!(copi.supports_permission());
        assert_eq!(
            copi.sandbox_args(Permission::Full),
            vec!["--allow-all".to_string()]
        );
        assert_eq!(
            copi.sandbox_args(Permission::ReadOnly),
            vec!["--deny-tool".to_string(), "write".to_string()]
        );
        Ok(())
    }

    /// A manifest adapter can declare its native add-dir trust flag (either
    /// key casing); omitting it keeps `--add-dir` a warned no-op.
    #[test]
    fn manifest_add_dir_flag_maps_repeatable_dirs() -> anyhow::Result<()> {
        let raw = r#"{
          "adapters": [
            {
              "id": "copi",
              "label": "Copi",
              "executable": "copi",
              "interactive_prompt_prefix_args": ["-i"],
              "non_interactive_prompt_prefix_args": ["-s", "-p"],
              "install_hint": "Install copi.",
              "addDirFlag": "--allow-dir"
            }
          ]
        }"#;
        let specs =
            parse_external_harness_specs(raw, Path::new("copi.json"), &built_in_harness_specs())?;
        let copi = &specs[0];
        assert!(copi.supports_add_dir());
        assert_eq!(
            copi.add_dir_args(&["/tmp/a".to_string(), "/tmp/b".to_string()]),
            vec![
                "--allow-dir".to_string(),
                "/tmp/a".to_string(),
                "--allow-dir".to_string(),
                "/tmp/b".to_string(),
            ]
        );

        let without = r#"{
          "adapters": [
            {
              "id": "plain",
              "label": "Plain",
              "executable": "plain",
              "interactive_prompt_prefix_args": [],
              "non_interactive_prompt_prefix_args": ["run"],
              "install_hint": "Install plain."
            }
          ]
        }"#;
        let specs = parse_external_harness_specs(
            without,
            Path::new("plain.json"),
            &built_in_harness_specs(),
        )?;
        assert!(!specs[0].supports_add_dir());
        Ok(())
    }

    #[test]
    fn manifest_capability_cross_checks_reject_undeclarable_configs() {
        let built_ins = built_in_harness_specs();
        let cases: &[(&str, &str)] = &[
            (
                // stream declared but no launch recipe
                r#"{"adapters":[{"id":"x","label":"X","executable":"x",
                    "interactive_prompt_prefix_args":[],
                    "non_interactive_prompt_prefix_args":["run"],
                    "install_hint":"hint",
                    "capabilities":{"stream":true}}]}"#,
                "declares `capabilities.stream` but no `stream_args`",
            ),
            (
                // stream_args without the capability is dead config
                r#"{"adapters":[{"id":"x","label":"X","executable":"x",
                    "interactive_prompt_prefix_args":[],
                    "non_interactive_prompt_prefix_args":["run"],
                    "install_hint":"hint",
                    "stream_args":{"prefix_args":["-p"]}}]}"#,
                "dead config",
            ),
            (
                // preassigned session id needs a flag to pass it with
                r#"{"adapters":[{"id":"x","label":"X","executable":"x",
                    "interactive_prompt_prefix_args":[],
                    "non_interactive_prompt_prefix_args":["run"],
                    "install_hint":"hint",
                    "capabilities":{"stream":true,"preassigned_session_id":true},
                    "stream_args":{"prefix_args":["-p"]}}]}"#,
                "no session id flag",
            ),
        ];
        for (raw, expected) in cases {
            let err = parse_external_harness_specs(raw, Path::new("x.json"), &built_ins)
                .expect_err("invalid capability config must be rejected");
            assert!(
                err.to_string().contains(expected),
                "expected `{expected}` in: {err:#}"
            );
        }
    }

    /// A manifest id that shadows a built-in harness must not fail the whole
    /// registry. A CLI upgrade can promote an adapter to built-in after a
    /// manifest for it was scaffolded (copilot, coven-code), and the old
    /// fatal error bricked every `coven run` — harnesses the manifest never
    /// mentioned included. The built-in wins; siblings still load.
    #[test]
    fn manifest_shadowing_built_in_is_skipped_not_fatal() -> anyhow::Result<()> {
        let raw = r#"{
          "adapters": [
            {
              "id": "codex",
              "label": "Codex (external)",
              "executable": "codex",
              "interactive_prompt_prefix_args": [],
              "non_interactive_prompt_prefix_args": ["exec"],
              "install_hint": "Install codex."
            },
            {
              "id": "sidecar",
              "label": "Sidecar",
              "executable": "sidecar",
              "interactive_prompt_prefix_args": [],
              "non_interactive_prompt_prefix_args": ["run"],
              "install_hint": "Install sidecar."
            }
          ]
        }"#;
        let specs = parse_external_harness_specs(
            raw,
            Path::new("adapters.json"),
            &built_in_harness_specs(),
        )?;
        let ids: Vec<&str> = specs.iter().map(|spec| spec.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["sidecar"],
            "shadowed built-in id must be skipped while the rest of the manifest loads"
        );
        Ok(())
    }

    /// End-to-end guard for the Cave regression: with a stale manifest that
    /// shadows `codex` configured through the adapter-dirs env, launching the
    /// built-in codex harness must still work instead of erroring at registry
    /// load.
    #[test]
    fn shadowing_manifest_does_not_break_built_in_launch() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        fs::write(
            temp_dir.path().join("codex.json"),
            r#"{
              "adapters": [
                {
                  "id": "codex",
                  "label": "Codex (stale scaffold)",
                  "executable": "codex",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": ["exec"],
                  "install_hint": "Install codex."
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous_manifest = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let previous_dirs = env::var_os(EXTERNAL_ADAPTER_DIRS_ENV);
        env::remove_var(EXTERNAL_ADAPTER_MANIFEST_ENV);
        env::set_var(EXTERNAL_ADAPTER_DIRS_ENV, temp_dir.path());

        let specs = configured_harness_specs();
        let launch = command_parts_for_harness("codex", "hello", HarnessLaunchMode::NonInteractive);

        restore_adapter_manifest_env(previous_manifest);
        restore_adapter_dirs_env(previous_dirs);

        let specs = specs?;
        let codex = specs
            .iter()
            .find(|spec| spec.id == "codex")
            .expect("built-in codex spec present");
        assert_eq!(
            codex.source, "bundled",
            "built-in must win over the manifest"
        );
        let (program, args) = launch?;
        assert_eq!(program, "codex");
        assert!(!args.is_empty());
        Ok(())
    }

    /// The same adapter id appearing twice (e.g. one manifest scaffolded into
    /// two configured dirs) keeps the first copy instead of failing the whole
    /// registry.
    #[test]
    fn duplicate_external_adapter_id_keeps_first_copy() -> anyhow::Result<()> {
        let first_dir = tempfile::tempdir()?;
        let second_dir = tempfile::tempdir()?;
        let manifest_for = |label: &str| {
            format!(
                r#"{{
                  "adapters": [
                    {{
                      "id": "sidecar",
                      "label": "{label}",
                      "executable": "sidecar",
                      "interactive_prompt_prefix_args": [],
                      "non_interactive_prompt_prefix_args": ["run"],
                      "install_hint": "Install sidecar."
                    }}
                  ]
                }}"#
            )
        };
        fs::write(first_dir.path().join("sidecar.json"), manifest_for("First"))?;
        fs::write(
            second_dir.path().join("sidecar.json"),
            manifest_for("Second"),
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous_manifest = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let previous_dirs = env::var_os(EXTERNAL_ADAPTER_DIRS_ENV);
        env::remove_var(EXTERNAL_ADAPTER_MANIFEST_ENV);
        env::set_var(
            EXTERNAL_ADAPTER_DIRS_ENV,
            env::join_paths([first_dir.path(), second_dir.path()])?,
        );

        let specs = configured_harness_specs();

        restore_adapter_manifest_env(previous_manifest);
        restore_adapter_dirs_env(previous_dirs);

        let specs = specs?;
        let sidecars: Vec<&HarnessCommandSpec> =
            specs.iter().filter(|spec| spec.id == "sidecar").collect();
        assert_eq!(sidecars.len(), 1, "duplicate id must collapse to one spec");
        assert_eq!(sidecars[0].label, "First", "first manifest wins");
        Ok(())
    }

    #[test]
    fn manifest_continuity_adapter_uses_declared_noninteractive_args() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manifest = temp_dir.path().join("continuity.json");
        fs::write(
            &manifest,
            r#"{
              "adapters": [
                {
                  "id": "threaded",
                  "label": "Threaded",
                  "executable": "threaded",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": ["run"],
                  "install_hint": "Install threaded.",
                  "capabilities": { "preassigned_session_id": true },
                  "continuity_args": {
                    "init_prefix_args": ["run", "--json"],
                    "resume_prefix_args": ["run", "--json"],
                    "session_id_flag": "--session",
                    "resume_flag": "--resume"
                  }
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous_manifest = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let previous_dirs = env::var_os(EXTERNAL_ADAPTER_DIRS_ENV);
        env::set_var(EXTERNAL_ADAPTER_MANIFEST_ENV, &manifest);
        env::remove_var(EXTERNAL_ADAPTER_DIRS_ENV);

        let init = command_parts_for_harness_with_conversation(
            "threaded",
            "hello",
            HarnessLaunchMode::NonInteractive,
            Some(&ConversationHint::Init {
                id: "session-1".to_string(),
            }),
            None,
            HarnessLaunchOptions::default(),
        );
        let resume = command_parts_for_harness_with_conversation(
            "threaded",
            "again",
            HarnessLaunchMode::NonInteractive,
            Some(&ConversationHint::Resume {
                id: "session-1".to_string(),
            }),
            None,
            HarnessLaunchOptions::default(),
        );

        restore_adapter_manifest_env(previous_manifest);
        restore_adapter_dirs_env(previous_dirs);

        assert_eq!(
            init?,
            (
                "threaded".to_string(),
                vec![
                    "run".to_string(),
                    "--json".to_string(),
                    "--session".to_string(),
                    "session-1".to_string(),
                    "--".to_string(),
                    "hello".to_string(),
                ]
            )
        );
        assert_eq!(
            resume?,
            (
                "threaded".to_string(),
                vec![
                    "run".to_string(),
                    "--json".to_string(),
                    "--resume".to_string(),
                    "session-1".to_string(),
                    "--".to_string(),
                    "again".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn manifest_resume_only_continuity_allows_generated_session_ids() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manifest = temp_dir.path().join("resume-only.json");
        fs::write(
            &manifest,
            r#"{
              "adapters": [
                {
                  "id": "resume-only",
                  "label": "Resume Only",
                  "executable": "resume-only",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": ["exec"],
                  "install_hint": "Install resume-only.",
                  "continuity_args": {
                    "resume_prefix_args": ["exec", "resume"]
                  }
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous_manifest = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let previous_dirs = env::var_os(EXTERNAL_ADAPTER_DIRS_ENV);
        env::set_var(EXTERNAL_ADAPTER_MANIFEST_ENV, &manifest);
        env::remove_var(EXTERNAL_ADAPTER_DIRS_ENV);

        let init = command_parts_for_harness_with_conversation(
            "resume-only",
            "first",
            HarnessLaunchMode::NonInteractive,
            Some(&ConversationHint::Init {
                id: "ignored".to_string(),
            }),
            None,
            HarnessLaunchOptions::default(),
        );
        let resume = command_parts_for_harness_with_conversation(
            "resume-only",
            "again",
            HarnessLaunchMode::NonInteractive,
            Some(&ConversationHint::Resume {
                id: "upstream-1".to_string(),
            }),
            None,
            HarnessLaunchOptions::default(),
        );

        restore_adapter_manifest_env(previous_manifest);
        restore_adapter_dirs_env(previous_dirs);

        assert_eq!(
            init?,
            (
                "resume-only".to_string(),
                vec!["exec".to_string(), "--".to_string(), "first".to_string()]
            )
        );
        assert_eq!(
            resume?,
            (
                "resume-only".to_string(),
                vec![
                    "exec".to_string(),
                    "resume".to_string(),
                    "upstream-1".to_string(),
                    "--".to_string(),
                    "again".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn manifest_continuity_rejects_dead_preassigned_session_config() {
        let built_ins = built_in_harness_specs();
        let cases: &[(&str, &str)] = &[
            (
                r#"{"adapters":[{"id":"x","label":"X","executable":"x",
                    "interactive_prompt_prefix_args":[],
                    "non_interactive_prompt_prefix_args":["run"],
                    "install_hint":"hint",
                    "continuity_args":{"init_prefix_args":["run"],"session_id_flag":"--session"}}]}"#,
                "`continuity_args.session_id_flag` but `capabilities.preassigned_session_id` is false",
            ),
            (
                r#"{"adapters":[{"id":"x","label":"X","executable":"x",
                    "interactive_prompt_prefix_args":[],
                    "non_interactive_prompt_prefix_args":["run"],
                    "install_hint":"hint",
                    "capabilities":{"preassigned_session_id":true},
                    "continuity_args":{"init_prefix_args":["run"]}}]}"#,
                "declares `capabilities.preassigned_session_id` but no session id flag",
            ),
        ];

        for (raw, expected) in cases {
            let err = parse_external_harness_specs(raw, Path::new("x.json"), &built_ins)
                .expect_err("invalid continuity config must be rejected");
            assert!(
                err.to_string().contains(expected),
                "expected `{expected}` in: {err:#}"
            );
        }
    }

    /// Claude's stream launch is byte-for-byte what the old hardcoded
    /// `stream_args()` produced, now read from its declared `stream_args`.
    #[test]
    fn claude_stream_launch_args_unchanged_after_declaration() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (program, args) = command_parts_for_harness_with_conversation(
            "claude",
            "hello",
            HarnessLaunchMode::Stream,
            Some(&ConversationHint::Init {
                id: "abc-123".to_string(),
            }),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(program, "claude");
        assert_eq!(
            args,
            vec![
                "--print".to_string(),
                "--input-format".to_string(),
                "stream-json".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
                "--session-id".to_string(),
                "abc-123".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn configured_harness_specs_load_adapter_manifests_from_directory_env() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manifest_dir = temp_dir.path().join("adapters");
        fs::create_dir(&manifest_dir)?;
        fs::write(
            manifest_dir.join("codex-compatible.json"),
            r#"{
              "adapters": [
                {
                  "id": "codex-compatible",
                  "label": "Codex Compatible",
                  "executable": "codex-compatible",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": ["exec", "--color", "never"],
                  "install_hint": "Install the Codex-compatible CLI and put it on PATH.",
                  "system_prompt_flag": null
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous_manifest = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let previous_dirs = env::var_os(EXTERNAL_ADAPTER_DIRS_ENV);
        env::remove_var(EXTERNAL_ADAPTER_MANIFEST_ENV);
        env::set_var(EXTERNAL_ADAPTER_DIRS_ENV, &manifest_dir);

        let specs = configured_harness_specs()?;

        restore_adapter_manifest_env(previous_manifest);
        restore_adapter_dirs_env(previous_dirs);

        let custom = specs
            .iter()
            .find(|spec| spec.id == "codex-compatible")
            .expect("directory manifest adapter should load");
        assert_eq!(custom.label, "Codex Compatible");
        assert_eq!(custom.executable, "codex-compatible");
        Ok(())
    }

    #[test]
    fn configured_harness_specs_load_trusted_coven_home_adapter_directory() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let coven_home = temp_dir.path().join("coven-home");
        let adapter_dir = coven_home.join("adapters");
        fs::create_dir_all(&adapter_dir)?;
        fs::write(adapter_dir.join("hermes.json"), HERMES_ADAPTER_MANIFEST)?;

        let _guard = env_lock().lock().unwrap();
        let _manifest_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let _dirs_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_DIRS_ENV);
        let _coven_home_guard = EnvVarGuard::set("COVEN_HOME", &coven_home);

        let specs = configured_harness_specs()?;

        let hermes = specs
            .iter()
            .find(|spec| spec.id == "hermes")
            .expect("trusted COVEN_HOME adapter should load");
        assert_eq!(hermes.label, "Hermes Agent");
        assert_eq!(
            hermes.manifest_path.as_deref(),
            Some(adapter_dir.join("hermes.json").to_string_lossy().as_ref())
        );
        Ok(())
    }

    #[test]
    fn trusted_adapter_manifest_rejects_size_mismatches_before_content_match() -> anyhow::Result<()>
    {
        let temp_dir = tempfile::tempdir()?;
        let coven_home = temp_dir.path().join("coven-home");
        let adapter_dir = coven_home.join("adapters");
        fs::create_dir_all(&adapter_dir)?;
        let manifest_path = adapter_dir.join("hermes.json");
        fs::write(&manifest_path, format!("{HERMES_ADAPTER_MANIFEST}\n"))?;

        assert!(!trusted_adapter_manifest_matches_recipe(
            &manifest_path,
            "hermes"
        ));
        Ok(())
    }

    #[test]
    fn trusted_adapter_manifest_source_parses_bundled_recipe_after_validation() -> anyhow::Result<()>
    {
        let temp_dir = tempfile::tempdir()?;
        let coven_home = temp_dir.path().join("coven-home");
        let adapter_dir = coven_home.join("adapters");
        fs::create_dir_all(&adapter_dir)?;
        let manifest_path = adapter_dir.join("hermes.json");
        fs::write(&manifest_path, HERMES_ADAPTER_MANIFEST)?;

        let mut sources = trusted_adapter_manifest_sources(&coven_home);
        assert_eq!(sources.len(), 1);
        fs::write(
            &manifest_path,
            r#"{"adapters":[{"id":"hermes","label":"Planted","executable":"sh","interactive_prompt_prefix_args":["-c"],"non_interactive_prompt_prefix_args":["-c"],"install_hint":"planted"}]}"#,
        )?;

        let specs = sources.remove(0).load_specs(&built_in_harness_specs())?;
        let hermes = specs
            .iter()
            .find(|spec| spec.id == "hermes")
            .expect("trusted source should parse bundled hermes recipe");

        assert_eq!(hermes.executable, "hermes");
        assert_eq!(
            hermes.manifest_path.as_deref(),
            Some(manifest_path.to_string_lossy().as_ref())
        );
        Ok(())
    }

    #[test]
    fn configured_harness_specs_ignore_coven_home_adapter_manifest_that_differs_from_recipe(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let coven_home = temp_dir.path().join("coven-home");
        let adapter_dir = coven_home.join("adapters");
        fs::create_dir_all(&adapter_dir)?;
        fs::write(
            adapter_dir.join("hermes.json"),
            r#"{
              "adapters": [
                {
                  "id": "hermes",
                  "label": "Planted Hermes",
                  "executable": "sh",
                  "interactive_prompt_prefix_args": ["-c", "echo planted"],
                  "non_interactive_prompt_prefix_args": ["-c", "echo planted"],
                  "install_hint": "planted"
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let _manifest_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let _dirs_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_DIRS_ENV);
        let _coven_home_guard = EnvVarGuard::set("COVEN_HOME", &coven_home);

        let specs = configured_harness_specs()?;

        assert!(specs.iter().all(|spec| spec.id != "hermes"));
        Ok(())
    }

    #[test]
    fn configured_harness_specs_ignore_home_and_xdg_adapter_dirs_without_explicit_env(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let home_adapters = temp_dir.path().join("home").join(".coven").join("adapters");
        fs::create_dir_all(&home_adapters)?;
        fs::write(
            home_adapters.join("home.json"),
            r#"{
              "adapters": [
                {
                  "id": "home-implicit",
                  "label": "Home Implicit",
                  "executable": "home-implicit",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": [],
                  "install_hint": "Do not load this implicitly.",
                  "system_prompt_flag": null
                }
              ]
            }"#,
        )?;

        let xdg_adapters = temp_dir
            .path()
            .join("config")
            .join("coven")
            .join("adapters");
        fs::create_dir_all(&xdg_adapters)?;
        fs::write(
            xdg_adapters.join("xdg.json"),
            r#"{
              "adapters": [
                {
                  "id": "xdg-implicit",
                  "label": "XDG Implicit",
                  "executable": "xdg-implicit",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": [],
                  "install_hint": "Do not load this implicitly.",
                  "system_prompt_flag": null
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let _manifest_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let _dirs_guard = EnvVarGuard::remove(EXTERNAL_ADAPTER_DIRS_ENV);
        let _coven_home_guard =
            EnvVarGuard::set("COVEN_HOME", temp_dir.path().join("empty-coven-home"));
        let _home_guard = EnvVarGuard::set("HOME", temp_dir.path().join("home"));
        let _xdg_config_home_guard =
            EnvVarGuard::set("XDG_CONFIG_HOME", temp_dir.path().join("config"));

        let specs = configured_harness_specs()?;

        assert!(!specs.iter().any(|spec| spec.id == "home-implicit"));
        assert!(!specs.iter().any(|spec| spec.id == "xdg-implicit"));
        Ok(())
    }

    #[test]
    fn configured_harnesses_include_adapter_source_metadata() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manifest = temp_dir.path().join("adapters.json");
        fs::write(
            &manifest,
            r#"{
              "adapters": [
                {
                  "id": "solo-codex",
                  "label": "Solo Codex",
                  "executable": "codex",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": ["exec"],
                  "install_hint": "Install Codex.",
                  "system_prompt_flag": null
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous_manifest = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        let previous_dirs = env::var_os(EXTERNAL_ADAPTER_DIRS_ENV);
        env::set_var(EXTERNAL_ADAPTER_MANIFEST_ENV, &manifest);
        env::remove_var(EXTERNAL_ADAPTER_DIRS_ENV);

        let harnesses = configured_harnesses()?;

        restore_adapter_manifest_env(previous_manifest);
        restore_adapter_dirs_env(previous_dirs);

        let codex = harnesses
            .iter()
            .find(|harness| harness.id == "codex")
            .unwrap();
        assert_eq!(codex.source, "bundled");
        assert!(codex.manifest_path.is_none());
        assert_eq!(codex.capabilities, Capabilities::BASELINE);

        let custom = harnesses
            .iter()
            .find(|harness| harness.id == "solo-codex")
            .unwrap();
        assert_eq!(custom.source, "manifest");
        assert_eq!(custom.capabilities, Capabilities::BASELINE);
        assert_eq!(
            custom.manifest_path.as_deref(),
            Some(manifest.to_string_lossy().as_ref())
        );
        Ok(())
    }

    #[test]
    fn claude_init_hint_attaches_session_id_flag_in_print_mode() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let hint = ConversationHint::Init {
            id: "abc-123".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "claude",
            "hello",
            HarnessLaunchMode::NonInteractive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(
            parts,
            (
                "claude".to_string(),
                vec![
                    "--print".to_string(),
                    "--session-id".to_string(),
                    "abc-123".to_string(),
                    "--".to_string(),
                    "hello".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn claude_resume_hint_attaches_resume_flag_in_print_mode() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let hint = ConversationHint::Resume {
            id: "abc-123".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "claude",
            "follow up",
            HarnessLaunchMode::NonInteractive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(
            parts,
            (
                "claude".to_string(),
                vec![
                    "--print".to_string(),
                    "--resume".to_string(),
                    "abc-123".to_string(),
                    "--".to_string(),
                    "follow up".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn coven_code_init_hint_attaches_session_id_flag_in_print_mode() -> anyhow::Result<()> {
        // `spawn_executable_for_platform("coven-code")` consults engine::resolve(),
        // which reads HOME/USERPROFILE. Pin env (and clear any managed-engine home)
        // so this deterministic-argv test does not race the managed-engine test.
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let empty = tempfile::tempdir()?;
        let _home_guard = EnvVarGuard::set("HOME", empty.path());
        #[cfg(windows)]
        let _userprofile_guard = EnvVarGuard::set("USERPROFILE", empty.path());
        let hint = ConversationHint::Init {
            id: "cc-session-42".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "coven-code",
            "hello",
            HarnessLaunchMode::NonInteractive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(
            parts,
            (
                spawn_executable_for_platform("coven-code"),
                vec![
                    "--print".to_string(),
                    "--session-id".to_string(),
                    "cc-session-42".to_string(),
                    "--".to_string(),
                    "hello".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn coven_code_resume_hint_attaches_resume_flag_in_print_mode() -> anyhow::Result<()> {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let empty = tempfile::tempdir()?;
        let _home_guard = EnvVarGuard::set("HOME", empty.path());
        #[cfg(windows)]
        let _userprofile_guard = EnvVarGuard::set("USERPROFILE", empty.path());
        let hint = ConversationHint::Resume {
            id: "cc-session-42".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "coven-code",
            "follow up",
            HarnessLaunchMode::NonInteractive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(
            parts,
            (
                spawn_executable_for_platform("coven-code"),
                vec![
                    "--print".to_string(),
                    "--resume".to_string(),
                    "cc-session-42".to_string(),
                    "--".to_string(),
                    "follow up".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn interactive_mode_ignores_conversation_hint() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let hint = ConversationHint::Init {
            id: "abc-123".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "claude",
            "hello",
            HarnessLaunchMode::Interactive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        );
        assert_eq!(
            parts.unwrap(),
            (
                "claude".to_string(),
                vec!["--".to_string(), "hello".to_string()]
            )
        );
        Ok(())
    }

    #[test]
    fn codex_init_hint_falls_through_to_default_args_so_codex_can_assign_its_own_id(
    ) -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let hint = ConversationHint::Init {
            id: "abc-123".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(
            parts,
            (
                "codex".to_string(),
                vec![
                    "exec".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "--".to_string(),
                    "fix tests".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn codex_resume_hint_uses_exec_resume_subcommand_with_id() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let hint = ConversationHint::Resume {
            id: "019e5998-7130-7872-8d96-a6b67c5b6406".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "codex",
            "follow up",
            HarnessLaunchMode::NonInteractive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(
            parts,
            (
                "codex".to_string(),
                vec![
                    "exec".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "resume".to_string(),
                    "019e5998-7130-7872-8d96-a6b67c5b6406".to_string(),
                    "--".to_string(),
                    "follow up".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn claude_stream_mode_preserves_permission_prompts_by_default() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (program, args) = command_parts_for_harness_with_conversation(
            "claude",
            "hello",
            HarnessLaunchMode::Stream,
            None,
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(program, "claude");
        assert!(!args
            .iter()
            .any(|a| a == "--permission-mode" || a == "bypassPermissions"));
        assert!(args.iter().any(|a| a == "--output-format"));
        Ok(())
    }

    #[test]
    fn claude_permission_bypass_requires_explicit_opt_in_values() {
        assert!(claude_permission_bypass_enabled_from_value(Some("1")));
        assert!(claude_permission_bypass_enabled_from_value(Some("true")));
        assert!(claude_permission_bypass_enabled_from_value(Some(" TRUE ")));
        assert!(!claude_permission_bypass_enabled_from_value(None));
        assert!(!claude_permission_bypass_enabled_from_value(Some("0")));
        assert!(!claude_permission_bypass_enabled_from_value(Some("false")));
    }

    #[test]
    fn claude_permission_bypass_opt_in_adds_flags() {
        let args = with_claude_permission_flags_enabled("claude", vec!["hello".to_string()], true);
        assert_eq!(
            args,
            vec![
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
                "hello".to_string()
            ]
        );
    }

    #[test]
    fn non_claude_harnesses_do_not_get_permission_bypass() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (_, args) =
            command_parts_for_harness("codex", "fix tests", HarnessLaunchMode::NonInteractive)?;
        assert!(!args
            .iter()
            .any(|a| a == "--permission-mode" || a == "bypassPermissions"));
        Ok(())
    }

    #[test]
    fn preassigned_session_id_support_is_per_harness() {
        assert!(harness_supports_preassigned_session_id("claude"));
        assert!(!harness_supports_preassigned_session_id("codex"));
        assert!(!harness_supports_preassigned_session_id("hermes"));
        assert!(!harness_supports_preassigned_session_id("unknown"));
    }

    #[test]
    fn normalize_model_id_strips_provider_prefix() {
        assert_eq!(normalize_model_id("openai/gpt-5.5"), "gpt-5.5");
        assert_eq!(
            normalize_model_id("anthropic/claude-sonnet-4"),
            "claude-sonnet-4"
        );
        // Bare ids pass through unchanged.
        assert_eq!(normalize_model_id("gpt-5.5"), "gpt-5.5");
        assert_eq!(normalize_model_id("sonnet"), "sonnet");
        // Only the first segment is treated as the provider namespace.
        assert_eq!(normalize_model_id("a/b/c"), "b/c");
        // Degenerate leading/trailing slash is left as-is rather than emptied.
        assert_eq!(normalize_model_id("/gpt"), "/gpt");
        assert_eq!(normalize_model_id("gpt/"), "gpt/");
    }

    #[test]
    fn built_in_harnesses_declare_a_model_flag() {
        for id in ["codex", "claude"] {
            let spec = built_in_harness_specs()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap();
            assert!(spec.supports_model(), "{id} should support --model");
            assert_eq!(
                spec.model_args("anything"),
                vec!["--model".to_string(), "anything".to_string()]
            );
        }
    }

    /// The capability model replaces the former `harness_id == "claude"`
    /// string checks: claude declares everything, codex stays baseline, and
    /// anything else (external adapters, unknown ids) is baseline until the
    /// manifest loader learns to read `capabilities` (integration.md, PR 2).
    #[test]
    fn built_in_capabilities_match_former_string_checks() {
        let claude = declared_capabilities("claude");
        assert!(claude.stream);
        assert!(claude.preassigned_session_id);
        assert!(claude.think);
        assert!(claude.speed);

        assert!(declared_capabilities("codex").is_baseline());
        assert!(declared_capabilities("hermes").is_baseline());
        assert!(declared_capabilities("unknown").is_baseline());
    }

    #[test]
    fn harness_summary_serializes_declared_capabilities() {
        let harnesses = built_in_harnesses();
        let json = serde_json::to_value(&harnesses).unwrap();

        assert_eq!(json[0]["id"], "codex");
        assert_eq!(json[0]["capabilities"]["stream"], false);
        assert_eq!(json[0]["capabilities"]["preassigned_session_id"], false);
        assert_eq!(json[1]["id"], "claude");
        assert_eq!(json[1]["capabilities"]["stream"], true);
        assert_eq!(json[1]["capabilities"]["preassigned_session_id"], true);
        assert_eq!(json[1]["capabilities"]["think"], true);
        assert_eq!(json[1]["capabilities"]["speed"], true);
    }

    #[test]
    fn codex_forwards_model_before_prompt_with_prefix_stripped() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (program, args) = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                model: Some("openai/gpt-5.5"),
                ..Default::default()
            },
        )?;
        assert_eq!(program, "codex");
        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "gpt-5.5".to_string(),
                "exec".to_string(),
                "--skip-git-repo-check".to_string(),
                "--color".to_string(),
                "never".to_string(),
                "--".to_string(),
                "fix tests".to_string(),
            ]
        );
        // The model flag/value sit ahead of the trailing prompt positional.
        let model_pos = args.iter().position(|a| a == "--model").unwrap();
        let prompt_pos = args.iter().position(|a| a == "fix tests").unwrap();
        assert!(model_pos < prompt_pos);
        Ok(())
    }

    #[test]
    fn codex_json_mode_inserts_flag_at_the_declared_subcommand_not_user_values(
    ) -> anyhow::Result<()> {
        // Both values deliberately look like CLI syntax. The model value comes
        // before the real `exec` subcommand, while the prompt is protected by
        // the trailing `--` separator. JSON mode must be constructed from the
        // harness spec, never by scanning those user-controlled entries.
        let (_, args) = command_parts_for_codex_json_with_conversation(
            "codex",
            "--json",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                model: Some("openai/exec"),
                ..Default::default()
            },
        )?;
        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "exec".to_string(),
                "exec".to_string(),
                "--json".to_string(),
                "--skip-git-repo-check".to_string(),
                "--color".to_string(),
                "never".to_string(),
                "--".to_string(),
                "--json".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn claude_forwards_model_with_prefix_stripped() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (_, args) = command_parts_for_harness_with_conversation(
            "claude",
            "hi",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                model: Some("anthropic/claude-sonnet-4"),
                ..Default::default()
            },
        )?;
        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "claude-sonnet-4".to_string(),
                "--print".to_string(),
                "--".to_string(),
                "hi".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn claude_think_maps_to_effort_high() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (_, args) = command_parts_for_harness_with_conversation(
            "claude",
            "hi",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                think: true,
                ..Default::default()
            },
        )?;
        assert_eq!(
            args,
            vec![
                "--effort".to_string(),
                "high".to_string(),
                "--print".to_string(),
                "--".to_string(),
                "hi".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn claude_speed_maps_to_effort_levels() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (_, fast_args) = command_parts_for_harness_with_conversation(
            "claude",
            "hi",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                speed: Some(HarnessSpeed::Fast),
                ..Default::default()
            },
        )?;
        let (_, thorough_args) = command_parts_for_harness_with_conversation(
            "claude",
            "hi",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                speed: Some(HarnessSpeed::Thorough),
                ..Default::default()
            },
        )?;

        assert_eq!(fast_args[0..2], ["--effort", "low"]);
        assert_eq!(thorough_args[0..2], ["--effort", "high"]);
        Ok(())
    }

    #[test]
    fn codex_ignores_think_and_speed_launch_hints() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let with_hints = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                think: true,
                speed: Some(HarnessSpeed::Thorough),
                ..Default::default()
            },
        )?;
        let legacy =
            command_parts_for_harness("codex", "fix tests", HarnessLaunchMode::NonInteractive)?;

        assert_eq!(with_hints, legacy);
        Ok(())
    }

    #[test]
    fn no_model_leaves_args_unchanged() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let with_model = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions::default(),
        )?;
        let blank_model = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                model: Some("   "),
                ..Default::default()
            },
        )?;
        let legacy =
            command_parts_for_harness("codex", "fix tests", HarnessLaunchMode::NonInteractive)?;
        assert_eq!(with_model, legacy);
        assert_eq!(blank_model, legacy, "a blank model is a no-op");
        Ok(())
    }

    #[test]
    fn permission_parse_round_trips_and_rejects_unknown() {
        assert_eq!(Permission::parse("full").unwrap(), Permission::Full);
        assert_eq!(
            Permission::parse("read-only").unwrap(),
            Permission::ReadOnly
        );
        // Trimmed + case-insensitive.
        assert_eq!(Permission::parse("  Full ").unwrap(), Permission::Full);
        assert_eq!(
            Permission::parse("READ-ONLY").unwrap(),
            Permission::ReadOnly
        );
        assert_eq!(Permission::Full.as_str(), "full");
        assert_eq!(Permission::ReadOnly.as_str(), "read-only");
        assert_eq!(
            Permission::parse(Permission::Full.as_str()).unwrap(),
            Permission::Full
        );
        let err = Permission::parse("write").unwrap_err().to_string();
        assert!(err.contains("invalid permission"), "{err}");
        assert!(err.contains("full, read-only"), "{err}");
    }

    #[test]
    fn built_in_harnesses_map_permission_to_native_sandbox_flag() {
        let codex = built_in_harness_specs()
            .into_iter()
            .find(|s| s.id == "codex")
            .unwrap();
        assert!(codex.supports_permission());
        assert_eq!(
            codex.sandbox_args(Permission::Full),
            vec!["--sandbox".to_string(), "danger-full-access".to_string()]
        );
        assert_eq!(
            codex.sandbox_args(Permission::ReadOnly),
            vec!["--sandbox".to_string(), "read-only".to_string()]
        );

        let claude = built_in_harness_specs()
            .into_iter()
            .find(|s| s.id == "claude")
            .unwrap();
        assert!(claude.supports_permission());
        assert_eq!(
            claude.sandbox_args(Permission::Full),
            vec![
                "--permission-mode".to_string(),
                "bypassPermissions".to_string()
            ]
        );
        assert_eq!(
            claude.sandbox_args(Permission::ReadOnly),
            vec!["--permission-mode".to_string(), "plan".to_string()]
        );
    }

    #[test]
    fn spec_without_sandbox_mechanism_is_a_permission_noop() {
        let spec = HarnessCommandSpec {
            id: "future".to_string(),
            label: "Future Harness".to_string(),
            executable: "future".to_string(),
            prompt_flag: None,
            interactive_prompt_flag: None,
            interactive_prompt_prefix_args: Vec::new(),
            non_interactive_prompt_prefix_args: vec!["run".to_string()],
            install_hint: "Install the future harness.".to_string(),
            source: "manifest".to_string(),
            manifest_path: None,
            system_prompt_flag: None,
            model_flag: None,
            model_arg_template: None,
            sandbox: None,
            add_dir_flag: None,
            capabilities: Capabilities::BASELINE,
            stream_args: None,
            continuity_args: None,
        };
        assert!(!spec.supports_permission());
        assert!(spec.sandbox_args(Permission::Full).is_empty());
        assert!(spec.sandbox_args(Permission::ReadOnly).is_empty());
    }

    #[test]
    fn spec_without_add_dir_mechanism_is_an_add_dir_noop() {
        let spec = HarnessCommandSpec {
            id: "future".to_string(),
            label: "Future Harness".to_string(),
            executable: "future".to_string(),
            prompt_flag: None,
            interactive_prompt_flag: None,
            interactive_prompt_prefix_args: Vec::new(),
            non_interactive_prompt_prefix_args: vec!["run".to_string()],
            install_hint: "Install the future harness.".to_string(),
            source: "manifest".to_string(),
            manifest_path: None,
            system_prompt_flag: None,
            model_flag: None,
            model_arg_template: None,
            sandbox: None,
            add_dir_flag: None,
            capabilities: Capabilities::BASELINE,
            stream_args: None,
            continuity_args: None,
        };
        assert!(!spec.supports_add_dir());
        assert!(spec.add_dir_args(&["/tmp/other".to_string()]).is_empty());
    }

    #[test]
    fn codex_forwards_add_dirs_before_prompt() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let add_dirs = vec!["/tmp/roots/a".to_string(), "/tmp/roots/b".to_string()];
        let (_, args) = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                add_dirs: &add_dirs,
                ..Default::default()
            },
        )?;
        assert_eq!(
            args,
            vec![
                "--add-dir".to_string(),
                "/tmp/roots/a".to_string(),
                "--add-dir".to_string(),
                "/tmp/roots/b".to_string(),
                "exec".to_string(),
                "--skip-git-repo-check".to_string(),
                "--color".to_string(),
                "never".to_string(),
                "--".to_string(),
                "fix tests".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn claude_stream_mode_forwards_add_dirs() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        // Blank entries are skipped; real dirs forward as repeated
        // `--add-dir <DIR>` pairs ahead of the declared stream args,
        // matching the non-stream path (`HarnessCommandSpec::add_dir_args`).
        let add_dirs = vec!["/tmp/roots/a".to_string(), "  ".to_string()];
        let (_, args) = command_parts_for_harness_with_conversation(
            "claude",
            "hello prompt",
            HarnessLaunchMode::Stream,
            Some(&ConversationHint::Init {
                id: "session-123".to_string(),
            }),
            None,
            HarnessLaunchOptions {
                add_dirs: &add_dirs,
                ..Default::default()
            },
        )?;
        let position = args
            .iter()
            .position(|arg| arg == "--add-dir")
            .expect("stream args must carry --add-dir");
        assert_eq!(args[position + 1], "/tmp/roots/a");
        assert_eq!(
            args.iter().filter(|arg| *arg == "--add-dir").count(),
            1,
            "blank add-dir entries are skipped: {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "--session-id"),
            "stream init args expected: {args:?}"
        );
        Ok(())
    }

    #[test]
    fn claude_forwards_add_dirs_after_permission() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let add_dirs = vec!["/tmp/roots/a".to_string()];
        let (_, args) = command_parts_for_harness_with_conversation(
            "claude",
            "hi",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                permission: Some(Permission::ReadOnly),
                add_dirs: &add_dirs,
                ..Default::default()
            },
        )?;
        assert_eq!(
            args,
            vec![
                "--permission-mode".to_string(),
                "plan".to_string(),
                "--add-dir".to_string(),
                "/tmp/roots/a".to_string(),
                "--print".to_string(),
                "--".to_string(),
                "hi".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn blank_add_dirs_leave_args_unchanged() -> anyhow::Result<()> {
        let blank = vec!["   ".to_string(), String::new()];
        let with_blank_dirs = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                add_dirs: &blank,
                ..Default::default()
            },
        )?;
        let legacy =
            command_parts_for_harness("codex", "fix tests", HarnessLaunchMode::NonInteractive)?;
        assert_eq!(with_blank_dirs, legacy, "blank add-dirs are a no-op");
        Ok(())
    }

    #[test]
    fn built_in_harnesses_declare_add_dir_flag() {
        // Cave forwards granted project roots via repeatable `--add-dir`;
        // every built-in harness CLI supports the flag natively (verified
        // against codex, claude, and the coven-code engine binaries).
        for spec in built_in_harness_specs() {
            assert_eq!(
                spec.add_dir_flag.as_deref(),
                Some("--add-dir"),
                "built-in `{}` must declare an add-dir mechanism",
                spec.id
            );
        }
    }

    #[test]
    fn codex_forwards_permission_before_prompt() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (_, args) = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                permission: Some(Permission::ReadOnly),
                ..Default::default()
            },
        )?;
        assert!(
            args.windows(2)
                .any(|w| w == ["--sandbox".to_string(), "read-only".to_string()]),
            "expected --sandbox read-only in {args:?}"
        );
        let sandbox_pos = args.iter().position(|a| a == "--sandbox").unwrap();
        let prompt_pos = args.iter().position(|a| a == "fix tests").unwrap();
        assert!(sandbox_pos < prompt_pos, "{args:?}");
        Ok(())
    }

    #[test]
    fn claude_forwards_permission_full_maps_to_bypass() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let (_, args) = command_parts_for_harness_with_conversation(
            "claude",
            "hi",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                permission: Some(Permission::Full),
                ..Default::default()
            },
        )?;
        assert_eq!(
            args,
            vec![
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
                "--print".to_string(),
                "--".to_string(),
                "hi".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn no_permission_leaves_args_unchanged() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let with_opt = command_parts_for_harness_with_conversation(
            "codex",
            "fix tests",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions::default(),
        )?;
        let legacy =
            command_parts_for_harness("codex", "fix tests", HarnessLaunchMode::NonInteractive)?;
        assert_eq!(with_opt, legacy, "no permission is a no-op");
        Ok(())
    }

    #[test]
    fn external_adapter_model_arg_template_expands_placeholder() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manifest = temp_dir.path().join("adapters.json");
        fs::write(
            &manifest,
            r#"{
              "adapters": [
                {
                  "id": "templated",
                  "label": "Templated Adapter",
                  "executable": "templated",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": ["run"],
                  "install_hint": "Install the templated adapter and put it on PATH.",
                  "model_arg_template": "-c model={model}"
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        env::set_var(EXTERNAL_ADAPTER_MANIFEST_ENV, &manifest);
        let parts = command_parts_for_harness_with_conversation(
            "templated",
            "do it",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                model: Some("openai/gpt-5.5"),
                ..Default::default()
            },
        );
        restore_adapter_manifest_env(previous);

        assert_eq!(
            parts?,
            (
                "templated".to_string(),
                vec![
                    "-c".to_string(),
                    "model=gpt-5.5".to_string(),
                    "run".to_string(),
                    "--".to_string(),
                    "do it".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn external_adapter_without_model_mechanism_is_a_noop() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manifest = temp_dir.path().join("adapters.json");
        fs::write(
            &manifest,
            r#"{
              "adapters": [
                {
                  "id": "plainadapter",
                  "label": "Plain Adapter",
                  "executable": "plainadapter",
                  "interactive_prompt_prefix_args": [],
                  "non_interactive_prompt_prefix_args": ["run"],
                  "install_hint": "Install the plain adapter and put it on PATH."
                }
              ]
            }"#,
        )?;

        let _guard = env_lock().lock().unwrap();
        let previous = env::var_os(EXTERNAL_ADAPTER_MANIFEST_ENV);
        env::set_var(EXTERNAL_ADAPTER_MANIFEST_ENV, &manifest);
        let spec = configured_harness_specs()?
            .into_iter()
            .find(|s| s.id == "plainadapter")
            .expect("adapter should load");
        // Passing a model must NOT inject any args (the run layer warns instead).
        let parts = command_parts_for_harness_with_conversation(
            "plainadapter",
            "do it",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                model: Some("openai/gpt-5.5"),
                ..Default::default()
            },
        );
        restore_adapter_manifest_env(previous);

        assert!(!spec.supports_model());
        assert!(spec.model_args("openai/gpt-5.5").is_empty());
        assert_eq!(
            parts?,
            (
                "plainadapter".to_string(),
                vec!["run".to_string(), "--".to_string(), "do it".to_string()]
            )
        );
        Ok(())
    }

    #[test]
    fn none_hint_matches_legacy_command_parts() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        let with_none = command_parts_for_harness_with_conversation(
            "claude",
            "hello",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions::default(),
        )?;
        let legacy =
            command_parts_for_harness("claude", "hello", HarnessLaunchMode::NonInteractive)?;
        assert_eq!(with_none, legacy);
        Ok(())
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) -> anyhow::Result<()> {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    // ── coven-code engine harness tests ──────────────────────────────────────

    #[test]
    fn coven_code_is_a_built_in_harness() {
        let specs = built_in_harness_specs();
        let cc = specs
            .iter()
            .find(|s| s.id == "coven-code")
            .expect("coven-code spec present");
        assert_eq!(cc.executable, "coven-code");
        assert_eq!(
            cc.non_interactive_prompt_prefix_args,
            vec!["--print".to_string()]
        );
        assert_eq!(
            cc.system_prompt_flag.as_deref(),
            Some("--append-system-prompt")
        );
        assert_eq!(cc.model_flag.as_deref(), Some("--model"));
        assert!(cc.capabilities.stream);
        assert!(cc.capabilities.preassigned_session_id);
        assert!(cc.capabilities.think);
        let stream = cc.stream_args.as_ref().expect("stream args");
        assert_eq!(stream.session_id_flag.as_deref(), Some("--session-id"));
        assert_eq!(stream.resume_flag.as_deref(), Some("--resume"));
    }

    #[test]
    fn coven_code_sandbox_uses_kebab_case_permission_mode() {
        let specs = built_in_harness_specs();
        let cc = specs.iter().find(|s| s.id == "coven-code").unwrap();
        let full = cc.sandbox_args(Permission::Full);
        assert!(
            full.iter().any(|a| a == "bypass-permissions"),
            "expected bypass-permissions in {full:?}"
        );
        assert!(
            !full.iter().any(|a| a == "bypassPermissions"),
            "must NOT use camelCase bypassPermissions; got {full:?}"
        );
        let read_only = cc.sandbox_args(Permission::ReadOnly);
        assert!(
            read_only.iter().any(|a| a == "plan"),
            "expected plan in {read_only:?}"
        );
    }

    #[test]
    fn coven_code_think_maps_to_effort_high() -> anyhow::Result<()> {
        let (_, args) = command_parts_for_harness_with_conversation(
            "coven-code",
            "hi",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                think: true,
                ..Default::default()
            },
        )?;
        let effort_pos = args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort flag present");
        assert_eq!(args[effort_pos + 1], "high");
        Ok(())
    }

    // ── Copilot ──────────────────────────────────────────────────────────
    //
    // Argv shapes in these tests are verified against the installed GitHub
    // Copilot CLI (1.0.70): positional prompts are rejected outright, so the
    // prompt always rides a flag's `=` form.

    #[test]
    fn copilot_noninteractive_uses_prompt_flag_equals_form() -> anyhow::Result<()> {
        // Spec resolution reads the adapter env vars; hold the shared env
        // lock so a concurrent test's tempdir manifest never vanishes mid-read.
        let _guard = env_lock().lock().unwrap();
        assert_eq!(
            command_parts_for_harness("copilot", "fix tests", HarnessLaunchMode::NonInteractive)?,
            (
                "copilot".to_string(),
                vec!["--no-color".to_string(), "--prompt=fix tests".to_string()]
            )
        );
        Ok(())
    }

    #[test]
    fn copilot_interactive_uses_interactive_flag_equals_form() -> anyhow::Result<()> {
        let _guard = env_lock().lock().unwrap();
        assert_eq!(
            command_parts_for_harness("copilot", "polish ui", HarnessLaunchMode::Interactive)?,
            (
                "copilot".to_string(),
                vec!["--interactive=polish ui".to_string()]
            )
        );
        Ok(())
    }

    #[test]
    fn copilot_dash_prefixed_prompt_stays_inside_flag_value() -> anyhow::Result<()> {
        let _guard = env_lock().lock().unwrap();
        // A prompt starting with `-` must never parse as harness flags; the
        // `=` form binds it as the prompt flag's value.
        let (_, args) = command_parts_for_harness(
            "copilot",
            "--version; rm -rf /",
            HarnessLaunchMode::NonInteractive,
        )?;
        assert_eq!(
            args.last().map(String::as_str),
            Some("--prompt=--version; rm -rf /"),
            "prompt must be the final argv entry bound via `=`: {args:?}"
        );
        Ok(())
    }

    #[test]
    fn copilot_stream_mode_falls_back_to_noninteractive_one_shot() -> anyhow::Result<()> {
        let _guard = env_lock().lock().unwrap();
        // Copilot has no stream-json stdin mode; a Stream request must build
        // the one-shot non-interactive command instead.
        assert!(!harness_supports_stream_mode("copilot"));
        assert_eq!(
            command_parts_for_harness("copilot", "hi", HarnessLaunchMode::Stream)?,
            (
                "copilot".to_string(),
                vec!["--no-color".to_string(), "--prompt=hi".to_string()]
            )
        );
        Ok(())
    }

    #[test]
    fn copilot_init_hint_preassigns_session_id_with_prompt_flag() -> anyhow::Result<()> {
        let _guard = env_lock().lock().unwrap();
        assert!(harness_supports_preassigned_session_id("copilot"));
        let hint = ConversationHint::Init {
            id: "11111111-2222-4333-8444-555555555555".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "copilot",
            "hello",
            HarnessLaunchMode::NonInteractive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(
            parts,
            (
                "copilot".to_string(),
                vec![
                    "--no-color".to_string(),
                    "--session-id".to_string(),
                    "11111111-2222-4333-8444-555555555555".to_string(),
                    "--prompt=hello".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn copilot_resume_hint_reuses_session_id_flag() -> anyhow::Result<()> {
        let _guard = env_lock().lock().unwrap();
        // Copilot's `--resume` only binds its value as `--resume=<id>`, which
        // the token-pair continuity form can't emit; `--session-id <id>`
        // resumes the same session (and self-heals to a fresh session with
        // that id when the store was wiped).
        let hint = ConversationHint::Resume {
            id: "11111111-2222-4333-8444-555555555555".to_string(),
        };
        let parts = command_parts_for_harness_with_conversation(
            "copilot",
            "follow up",
            HarnessLaunchMode::NonInteractive,
            Some(&hint),
            None,
            HarnessLaunchOptions::default(),
        )?;
        assert_eq!(
            parts,
            (
                "copilot".to_string(),
                vec![
                    "--no-color".to_string(),
                    "--session-id".to_string(),
                    "11111111-2222-4333-8444-555555555555".to_string(),
                    "--prompt=follow up".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn copilot_sandbox_maps_full_and_read_only_to_argv_lists() {
        let specs = built_in_harness_specs();
        let copilot = specs.iter().find(|s| s.id == "copilot").unwrap();
        assert!(copilot.supports_permission());
        assert_eq!(
            copilot.sandbox_args(Permission::Full),
            vec!["--allow-all".to_string()]
        );
        assert_eq!(
            copilot.sandbox_args(Permission::ReadOnly),
            vec![
                "--deny-tool".to_string(),
                "write".to_string(),
                "--deny-tool".to_string(),
                "shell".to_string(),
            ]
        );
    }

    #[test]
    fn copilot_launch_options_prepend_model_permission_dirs_and_effort() -> anyhow::Result<()> {
        let _guard = env_lock().lock().unwrap();
        let add_dirs = vec!["/tmp/extra".to_string()];
        let (_, args) = command_parts_for_harness_with_conversation(
            "copilot",
            "hi",
            HarnessLaunchMode::NonInteractive,
            None,
            None,
            HarnessLaunchOptions {
                model: Some("openai/gpt-5.5"),
                speed: Some(HarnessSpeed::Fast),
                permission: Some(Permission::ReadOnly),
                add_dirs: &add_dirs,
                ..Default::default()
            },
        )?;
        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "gpt-5.5".to_string(),
                "--deny-tool".to_string(),
                "write".to_string(),
                "--deny-tool".to_string(),
                "shell".to_string(),
                "--add-dir".to_string(),
                "/tmp/extra".to_string(),
                "--effort".to_string(),
                "low".to_string(),
                "--no-color".to_string(),
                "--prompt=hi".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn copilot_identity_preamble_rides_inside_prompt_flag_value() -> anyhow::Result<()> {
        let _guard = env_lock().lock().unwrap();
        // Copilot has no system-prompt flag, so familiar identity is injected
        // as a bracketed preamble inside the prompt flag's value.
        let familiar = FamiliarContext {
            id: "charm".to_string(),
            display_name: "Charm".to_string(),
            role: None,
        };
        let (_, args) = command_parts_for_harness_with_conversation(
            "copilot",
            "do the task",
            HarnessLaunchMode::NonInteractive,
            None,
            Some(&familiar),
            HarnessLaunchOptions::default(),
        )?;
        let prompt_arg = args.last().expect("prompt arg present");
        assert!(
            prompt_arg.starts_with("--prompt=[Identity: You are Charm"),
            "identity preamble must lead the prompt value: {prompt_arg:?}"
        );
        assert!(
            prompt_arg.ends_with("do the task"),
            "task prompt must close the prompt value: {prompt_arg:?}"
        );
        Ok(())
    }

    #[test]
    fn harness_available_returns_true_when_managed_engine_resolves() {
        // When the managed engine is installed at the standard location,
        // harness_available("coven-code") must return true even if "coven-code"
        // is not on PATH.  Use a temp-dir fixture to simulate a managed install.
        use std::io::Write;

        let home = tempfile::tempdir().unwrap();
        let engine_dir = home.path().join(".coven").join("engine").join("0.6.1");
        fs::create_dir_all(&engine_dir).unwrap();
        let bin = engine_dir.join(crate::engine::ENGINE_BIN_NAME);
        let mut f = fs::File::create(&bin).unwrap();
        f.write_all(b"#!/bin/sh\n").unwrap();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(home.path().join(".coven/engine/current"), "0.6.1").unwrap();

        // Drive resolution deterministically on every platform via the
        // COVEN_ENGINE_BIN override. `dirs_next::home_dir()` on Windows consults
        // the SHGetKnownFolderPath API, which a USERPROFILE/HOME env override does
        // not reliably change — so the managed-home path is not test-controllable
        // cross-platform. The override exercises the same is_executable() gate and
        // the same harness_available() code path.
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _override_guard = EnvVarGuard::set("COVEN_ENGINE_BIN", &bin);

        assert!(
            harness_available("coven-code"),
            "should be available via managed engine"
        );
    }
}
