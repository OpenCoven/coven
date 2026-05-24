use std::env;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessSummary {
    pub id: &'static str,
    pub label: &'static str,
    pub executable: &'static str,
    pub available: bool,
    pub install_hint: &'static str,
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

/// Whether the harness CLI has a long-lived JSON-streaming mode the daemon
/// can keep alive across chat turns. Claude does (`stream-json`); codex
/// doesn't (only one-shot `codex exec`). Gated to Unix today because the
/// daemon's stream-mode kill path relies on Unix process-group semantics
/// (`setsid()` at spawn, then `kill(-pid, SIGKILL)` to tear down the
/// harness plus any subprocesses it spawned in one syscall). A Windows
/// process-tree termination path would let this widen. See
/// `docs/chat-persistence.md`.
#[cfg(unix)]
pub fn harness_supports_stream_mode(harness_id: &str) -> bool {
    harness_id == "claude"
}

#[cfg(not(unix))]
pub fn harness_supports_stream_mode(_harness_id: &str) -> bool {
    false
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
    harness_id == "claude"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub executable: &'static str,
    pub interactive_prompt_prefix_args: &'static [&'static str],
    pub non_interactive_prompt_prefix_args: &'static [&'static str],
    pub install_hint: &'static str,
}

impl HarnessCommandSpec {
    pub fn prompt_args(&self, prompt: &str, mode: HarnessLaunchMode) -> Vec<String> {
        let prefix_args = match mode {
            HarnessLaunchMode::Interactive => self.interactive_prompt_prefix_args,
            HarnessLaunchMode::NonInteractive => self.non_interactive_prompt_prefix_args,
            // Stream mode bypasses `prompt_args` entirely (no trailing
            // prompt; messages arrive on stdin). Fall back to
            // non-interactive args if a caller somehow lands here.
            HarnessLaunchMode::Stream => self.non_interactive_prompt_prefix_args,
        };

        prefix_args
            .iter()
            .map(|arg| (*arg).to_string())
            .chain(std::iter::once(prompt.to_string()))
            .collect()
    }
}

pub fn built_in_harnesses() -> Vec<HarnessSummary> {
    built_in_harness_specs()
        .into_iter()
        .map(|spec| HarnessSummary {
            id: spec.id,
            label: spec.label,
            executable: spec.executable,
            available: executable_exists(spec.executable),
            install_hint: spec.install_hint,
        })
        .collect()
}

pub fn built_in_harness_specs() -> Vec<HarnessCommandSpec> {
    vec![
        HarnessCommandSpec {
            id: "codex",
            label: "Codex",
            executable: "codex",
            interactive_prompt_prefix_args: &[],
            non_interactive_prompt_prefix_args: &[
                "exec",
                "--skip-git-repo-check",
                "--color",
                "never",
            ],
            install_hint: "Install Codex with `npm install -g @openai/codex` or `brew install --cask codex`; if it is already installed, make sure `codex` is on PATH and run `codex login` or `codex` once to authenticate, then retry `coven doctor`.",
        },
        HarnessCommandSpec {
            id: "claude",
            label: "Claude Code",
            executable: "claude",
            interactive_prompt_prefix_args: &[],
            non_interactive_prompt_prefix_args: &["--print"],
            install_hint: "Install Claude Code with `npm install -g @anthropic-ai/claude-code`; if it is already installed, make sure `claude` is on PATH and run `claude doctor` to finish local auth/setup, then retry `coven doctor`.",
        },
    ]
}

#[cfg(test)]
pub fn command_parts_for_harness(
    harness_id: &str,
    prompt: &str,
    mode: HarnessLaunchMode,
) -> Result<(&'static str, Vec<String>)> {
    command_parts_for_harness_with_conversation(harness_id, prompt, mode, None)
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
) -> Result<(&'static str, Vec<String>)> {
    let spec = built_in_harness_specs()
        .into_iter()
        .find(|spec| spec.id == harness_id)
        .ok_or_else(|| anyhow!("unsupported harness `{harness_id}`"))?;

    // Stream mode reads prompts from stdin as JSON messages, so the prompt
    // argument is not appended. The continuity hint (claude resume / init)
    // still maps to a CLI flag; codex falls back to one-shot.
    if mode == HarnessLaunchMode::Stream {
        if let Some(args) = stream_args(&spec, hint) {
            return Ok((spec.executable, args));
        }
        // Harness doesn't support stream: fall through to non-interactive.
        return Ok((
            spec.executable,
            spec.prompt_args(prompt, HarnessLaunchMode::NonInteractive),
        ));
    }

    if let Some(hint) = hint {
        if let Some(args) = continuity_args(&spec, mode, hint) {
            return Ok((
                spec.executable,
                args.into_iter()
                    .chain(std::iter::once(prompt.to_string()))
                    .collect(),
            ));
        }
    }

    Ok((spec.executable, spec.prompt_args(prompt, mode)))
}

/// Per-harness translation of stream-mode launch into CLI args. Stream-mode
/// processes are long-lived: stdin is a stream of newline-delimited JSON
/// messages and stdout is a stream of newline-delimited JSON events.
/// Returns `None` for harnesses that don't support stream mode so the
/// caller can fall back to a one-shot launch.
fn stream_args(spec: &HarnessCommandSpec, hint: Option<&ConversationHint>) -> Option<Vec<String>> {
    match spec.id {
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
    match spec.id {
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
                let mut args: Vec<String> = spec
                    .non_interactive_prompt_prefix_args
                    .iter()
                    .map(|arg| (*arg).to_string())
                    .collect();
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
    let extensions = env::var_os("PATHEXT")
        .map(|value| {
            env::split_paths(&value)
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()]);

    let base = path.join(executable);
    let has_extension = Path::new(executable).extension().is_some();
    std::iter::once(base.clone()).chain(extensions.into_iter().filter_map(move |extension| {
        if has_extension {
            None
        } else {
            Some(path.join(format!("{executable}{extension}")))
        }
    }))
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

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn executable_exists_in_paths_finds_matching_file() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let executable = temp_dir.path().join("codex");
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
            ("codex", vec!["fix tests".to_string()])
        );
        assert_eq!(
            command_parts_for_harness("claude", "polish ui", HarnessLaunchMode::Interactive)?,
            ("claude", vec!["polish ui".to_string()])
        );
        Ok(())
    }

    #[test]
    fn command_parts_for_known_harnesses_use_noninteractive_entrypoints() -> anyhow::Result<()> {
        assert_eq!(
            command_parts_for_harness("codex", "fix tests", HarnessLaunchMode::NonInteractive)?,
            (
                "codex",
                vec![
                    "exec".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "fix tests".to_string(),
                ]
            )
        );
        assert_eq!(
            command_parts_for_harness("claude", "polish ui", HarnessLaunchMode::NonInteractive)?,
            (
                "claude",
                vec!["--print".to_string(), "polish ui".to_string()]
            )
        );
        Ok(())
    }

    #[test]
    fn command_spec_supports_prefix_args_for_future_harnesses() {
        let spec = HarnessCommandSpec {
            id: "future",
            label: "Future Harness",
            executable: "future",
            interactive_prompt_prefix_args: &["chat"],
            non_interactive_prompt_prefix_args: &["exec", "-q"],
            install_hint: "Install the future harness.",
        };

        assert_eq!(
            spec.prompt_args("hello", HarnessLaunchMode::Interactive),
            vec!["chat".to_string(), "hello".to_string()]
        );
        assert_eq!(
            spec.prompt_args("hello", HarnessLaunchMode::NonInteractive),
            vec!["exec".to_string(), "-q".to_string(), "hello".to_string()]
        );
    }

    #[test]
    fn command_parts_reject_unknown_harnesses() {
        assert!(
            command_parts_for_harness("hermes", "hello", HarnessLaunchMode::Interactive)
                .unwrap_err()
                .to_string()
                .contains("unsupported harness")
        );
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
        )?;
        assert_eq!(
            parts,
            (
                "claude",
                vec![
                    "--print".to_string(),
                    "--session-id".to_string(),
                    "abc-123".to_string(),
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
        )?;
        assert_eq!(
            parts,
            (
                "claude",
                vec![
                    "--print".to_string(),
                    "--resume".to_string(),
                    "abc-123".to_string(),
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
        )?;
        assert_eq!(parts, ("claude", vec!["hello".to_string()]));
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
        )?;
        assert_eq!(
            parts,
            (
                "codex",
                vec![
                    "exec".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
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
        )?;
        assert_eq!(
            parts,
            (
                "codex",
                vec![
                    "exec".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "resume".to_string(),
                    "019e5998-7130-7872-8d96-a6b67c5b6406".to_string(),
                    "follow up".to_string(),
                ]
            )
        );
        Ok(())
    }

    #[test]
    fn preassigned_session_id_support_is_per_harness() {
        assert!(harness_supports_preassigned_session_id("claude"));
        assert!(!harness_supports_preassigned_session_id("codex"));
        assert!(!harness_supports_preassigned_session_id("unknown"));
    }

    #[test]
    fn none_hint_matches_legacy_command_parts() -> anyhow::Result<()> {
        let with_none = command_parts_for_harness_with_conversation(
            "claude",
            "hello",
            HarnessLaunchMode::NonInteractive,
            None,
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
