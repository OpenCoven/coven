use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use coven_runtime_spec::Capabilities;
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

/// Declared capabilities for a configured harness id. Built-in harnesses
/// declare theirs in [`built_in_harness_specs`]; ids that don't match a
/// built-in (external adapters, unknown ids) get the conservative baseline
/// (all off), which matches today's behavior — external adapters can't
/// declare capabilities until the manifest loader learns to read them
/// (coven-runtimes integration.md, PR 2).
fn declared_capabilities(harness_id: &str) -> Capabilities {
    built_in_harness_specs()
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

/// How a harness translates a `Permission` into its native sandbox flag.
/// `flag` is the CLI flag name (e.g. `--sandbox`); `full`/`read_only` are the
/// values passed for each policy. Mirrors the `model_flag` design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMapping {
    pub flag: String,
    pub full: String,
    pub read_only: String,
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
    /// Behavioral capabilities (stream mode, session pre-assignment, think,
    /// speed). Shared type with the coven-runtimes manifest spec so built-in
    /// harnesses and external adapters declare what they can do through the
    /// same struct instead of hardcoded harness-id checks. External adapters
    /// currently get the conservative baseline (all off); reading these from
    /// the manifest is the follow-up loader change (integration.md, PR 2).
    pub capabilities: Capabilities,
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
            Some(mapping) => vec![
                mapping.flag.clone(),
                match permission {
                    Permission::Full => mapping.full.clone(),
                    Permission::ReadOnly => mapping.read_only.clone(),
                },
            ],
            None => Vec::new(),
        }
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
            available: executable_exists(&spec.executable),
            id: spec.id,
            label: spec.label,
            executable: spec.executable,
            install_hint: spec.install_hint,
            source: spec.source,
            manifest_path: spec.manifest_path,
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
            sandbox: Some(SandboxMapping {
                flag: "--sandbox".to_string(),
                full: "danger-full-access".to_string(),
                read_only: "read-only".to_string(),
            }),
            // One-shot `codex exec` only: no stream-json mode, no session
            // pre-assignment (ids are captured from the first turn's output),
            // no think/speed toggles.
            capabilities: Capabilities::BASELINE,
        },
        HarnessCommandSpec {
            id: "claude".to_string(),
            label: "Claude Code".to_string(),
            executable: "claude".to_string(),
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
            sandbox: Some(SandboxMapping {
                flag: "--permission-mode".to_string(),
                full: "bypassPermissions".to_string(),
                read_only: "plan".to_string(),
            }),
            // Long-lived stream-json mode, `--session-id`/`--resume`
            // pre-assignment, and think/speed via `--effort`. These declared
            // values replace the former `harness_id == "claude"` checks.
            capabilities: Capabilities {
                stream: true,
                preassigned_session_id: true,
                think: true,
                speed: true,
            },
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
    format!(
        "unsupported harness `{harness_id}`. Configured harnesses: {configured}. \
To use Hermes, run `coven adapter install hermes`, then `coven adapter doctor hermes`. \
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
                anyhow::bail!(
                    "external harness adapter `{}` in {} duplicates another adapter id",
                    spec.id,
                    manifest.path().display()
                );
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
    env::var_os("COVEN_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs_next::home_dir().map(|home| home.join(crate::DEFAULT_COVEN_HOME_DIR)))
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
        "hermes" => Some(HERMES_ADAPTER_MANIFEST),
        _ => None,
    }
}

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
    &["hermes"]
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
    /// How this adapter takes a model selection. Declare `model_flag` for a
    /// simple `--flag <value>` pair, or `model_arg_template` for anything else
    /// (e.g. `"-c model={model}"`). Omit both and `coven run --model` is a
    /// warned no-op for this adapter rather than an error.
    #[serde(default, alias = "modelFlag")]
    model_flag: Option<String>,
    #[serde(default, alias = "modelArgTemplate")]
    model_arg_template: Option<String>,
}

impl ExternalHarnessAdapterSpec {
    fn into_spec(
        self,
        manifest_path: &Path,
        built_ins: &[HarnessCommandSpec],
    ) -> Result<HarnessCommandSpec> {
        let id = self.id.trim().to_lowercase();
        if !valid_adapter_id(&id) {
            anyhow::bail!(
                "invalid harness adapter id `{}` in {}; use lowercase letters, digits, '.', '_' or '-'",
                self.id,
                manifest_path.display()
            );
        }
        if built_ins.iter().any(|spec| spec.id == id) {
            anyhow::bail!(
                "external harness adapter `{id}` in {} conflicts with a built-in harness",
                manifest_path.display()
            );
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
        Ok(HarnessCommandSpec {
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
            model_flag: self
                .model_flag
                .map(|flag| flag.trim().to_string())
                .filter(|flag| !flag.is_empty()),
            model_arg_template: self
                .model_arg_template
                .map(|tmpl| tmpl.trim().to_string())
                .filter(|tmpl| !tmpl.is_empty()),
            // External adapters declare no sandbox mechanism today, so
            // `coven run --permission` warns and continues for them.
            sandbox: None,
            // Conservative baseline (all off) until the loader reads
            // `capabilities` from the manifest (integration.md, PR 2).
            capabilities: Capabilities::BASELINE,
        })
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
                        &launch_option_args,
                        args,
                    )),
                ),
            ));
        }
        // Harness doesn't support stream: fall through to non-interactive.
        return Ok((
            program,
            with_claude_permission_flags(
                harness_id,
                sanitize_argv_for_platform(prepend_launch_args(
                    &model_args,
                    &sandbox_args,
                    &launch_option_args,
                    spec.prompt_args(&effective_prompt, HarnessLaunchMode::NonInteractive),
                )),
            ),
        ));
    }

    if let Some(hint) = hint {
        if let Some(mut args) = continuity_args(&spec, mode, hint) {
            // Inject identity via --system-prompt for harnesses that support it.
            if let (Some(flag), Some(f)) = (spec.system_prompt_flag.as_deref(), familiar) {
                args.insert(0, f.identity_preamble());
                args.insert(0, flag.to_string());
            }
            let args = sanitize_argv_for_platform(prepend_launch_args(
                &model_args,
                &sandbox_args,
                &launch_option_args,
                // `--` before the prompt for the same reason as `prompt_args`:
                // user data must not parse as harness flags.
                args.into_iter()
                    .chain(["--".to_string(), effective_prompt])
                    .collect(),
            ));
            return Ok((program, with_claude_permission_flags(harness_id, args)));
        }
    }

    let mut args = spec.prompt_args(&effective_prompt, mode);
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
                &launch_option_args,
                args,
            )),
        ),
    ))
}

fn launch_option_args(harness_id: &str, options: HarnessLaunchOptions<'_>) -> Vec<String> {
    if harness_id != "claude" {
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
    option_args: &[String],
    args: Vec<String>,
) -> Vec<String> {
    if model_args.is_empty() && sandbox_args.is_empty() && option_args.is_empty() {
        return args;
    }
    let mut out =
        Vec::with_capacity(model_args.len() + sandbox_args.len() + option_args.len() + args.len());
    out.extend_from_slice(model_args);
    out.extend_from_slice(sandbox_args);
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

fn stream_args(spec: &HarnessCommandSpec, hint: Option<&ConversationHint>) -> Option<Vec<String>> {
    match spec.id.as_str() {
        "claude" => {
            let mut args: Vec<String> = vec![
                "--print".to_string(),
                "--input-format".to_string(),
                "stream-json".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
            ];
            if let Some(hint) = hint {
                let flag = match hint {
                    ConversationHint::Init { .. } => "--session-id",
                    ConversationHint::Resume { .. } => "--resume",
                };
                args.push(flag.to_string());
                args.push(hint.id().to_string());
            }
            Some(args)
        }
        _ => None,
    }
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
    match spec.id.as_str() {
        "claude" => {
            let flag = match hint {
                ConversationHint::Init { .. } => "--session-id",
                ConversationHint::Resume { .. } => "--resume",
            };
            Some(vec![
                "--print".to_string(),
                flag.to_string(),
                hint.id().to_string(),
            ])
        }
        "codex" => match hint {
            // Codex auto-assigns the session id on the first turn; we capture
            // it from output and feed it back on subsequent turns.
            ConversationHint::Init { .. } => None,
            ConversationHint::Resume { id } => {
                let mut args: Vec<String> = spec.non_interactive_prompt_prefix_args.to_vec();
                args.push("resume".to_string());
                args.push(id.clone());
                Some(args)
            }
        },
        _ => None,
    }
}

fn executable_exists(executable: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| executable_exists_in_paths(executable, env::split_paths(&paths)))
        .unwrap_or(false)
}

#[cfg(windows)]
pub(crate) fn spawn_executable_for_platform(executable: &str) -> String {
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

    paths.into_iter().find_map(|path| {
        windows_executable_candidates(&path, executable, extensions.clone())
            .into_iter()
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

#[cfg(any(windows, test))]
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
    fn cmd_shim_metacharacters_are_caret_escaped_and_wrapped() {
        assert_eq!(escape_cmd_shim_metacharacters("safe prompt"), "safe prompt");

        let escaped = escape_cmd_shim_metacharacters(r#"a&b|c<d>e^f%g!h"i"#);

        assert_eq!(escaped, r#""a^&b^|c^<d^>e^^f^%g^!h^"i""#);
        assert!(escaped.starts_with('"'));
        assert!(escaped.ends_with('"'));
    }

    #[test]
    fn built_in_harnesses_returns_codex_and_claude() {
        let harnesses = built_in_harnesses();

        assert_eq!(harnesses.len(), 2);
        assert_eq!(harnesses[0].id, "codex");
        assert_eq!(harnesses[0].label, "Codex");
        assert_eq!(harnesses[0].executable, "codex");
        assert_eq!(harnesses[1].id, "claude");
        assert_eq!(harnesses[1].label, "Claude Code");
        assert_eq!(harnesses[1].executable, "claude");
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
    }

    #[test]
    fn command_parts_for_known_harnesses_append_interactive_prompt() -> anyhow::Result<()> {
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
            interactive_prompt_prefix_args: vec!["chat".to_string()],
            non_interactive_prompt_prefix_args: vec!["exec".to_string(), "-q".to_string()],
            install_hint: "Install the future harness.".to_string(),
            source: "manifest".to_string(),
            manifest_path: None,
            system_prompt_flag: None,
            model_flag: None,
            model_arg_template: None,
            sandbox: None,
            capabilities: Capabilities::BASELINE,
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

        let custom = harnesses
            .iter()
            .find(|harness| harness.id == "solo-codex")
            .unwrap();
        assert_eq!(custom.source, "manifest");
        assert_eq!(
            custom.manifest_path.as_deref(),
            Some(manifest.to_string_lossy().as_ref())
        );
        Ok(())
    }

    #[test]
    fn claude_init_hint_attaches_session_id_flag_in_print_mode() -> anyhow::Result<()> {
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
    fn interactive_mode_ignores_conversation_hint() -> anyhow::Result<()> {
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
    fn codex_forwards_model_before_prompt_with_prefix_stripped() -> anyhow::Result<()> {
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
    fn claude_forwards_model_with_prefix_stripped() -> anyhow::Result<()> {
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
            interactive_prompt_prefix_args: Vec::new(),
            non_interactive_prompt_prefix_args: vec!["run".to_string()],
            install_hint: "Install the future harness.".to_string(),
            source: "manifest".to_string(),
            manifest_path: None,
            system_prompt_flag: None,
            model_flag: None,
            model_arg_template: None,
            sandbox: None,
            capabilities: Capabilities::BASELINE,
        };
        assert!(!spec.supports_permission());
        assert!(spec.sandbox_args(Permission::Full).is_empty());
        assert!(spec.sandbox_args(Permission::ReadOnly).is_empty());
    }

    #[test]
    fn codex_forwards_permission_before_prompt() -> anyhow::Result<()> {
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
}
