//! Cast intent parsing.
//!
//! Phase 1 is deterministic: slash commands, a small set of plain-language
//! patterns, and a default-prompt fallback. No LLM planner. Each user spell
//! becomes one `CastIntent` value; the planner decides what (if anything) to
//! run.

use anyhow::{anyhow, Result};

/// A first-party Coven harness Cast knows how to route to without asking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CastHarness {
    Codex,
    Claude,
}

impl CastHarness {
    pub(crate) fn id(self) -> &'static str {
        match self {
            CastHarness::Codex => "codex",
            CastHarness::Claude => "claude",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            CastHarness::Codex => "Codex",
            CastHarness::Claude => "Claude Code",
        }
    }

    pub(crate) fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "codex" => Some(CastHarness::Codex),
            "claude" | "claude-code" | "claudecode" => Some(CastHarness::Claude),
            _ => None,
        }
    }
}

/// The typed shape of a user spell after Cast parses it. This is the only
/// thing the planner needs to read; raw user text never reaches the daemon
/// without becoming a `CastIntent` first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CastIntent {
    /// Plain text — Cast will route to the safe default harness with a
    /// project-scoped session.
    NaturalSpell {
        prompt: String,
    },
    /// The user picked a harness explicitly (slash command or "run claude …").
    HarnessSpell {
        harness: CastHarness,
        prompt: String,
    },
    OpenSessions,
    OpenAllSessions,
    AttachSession {
        session_id: String,
    },
    SummonSession {
        session_id: String,
    },
    ArchiveSession {
        session_id: String,
    },
    KillSession {
        session_id: String,
    },
    SacrificeSession {
        session_id: String,
    },
    Doctor,
    DaemonStatus,
    Help,
    StartHere,
    OpenTui,
    PatchOpenClaw,
    /// Multi-phase sequential goal. Cast turns the goal into a `Quest`
    /// (design → implement → verify by default) and dispatches each phase
    /// in order. See `cast::quest` and `docs/design/cast-quest-flow.md`.
    Quest {
        goal: String,
    },
    Quit,
}

/// Parse a raw user spell into a typed `CastIntent`. Empty input is treated
/// as "open the Cast launcher" (which is what the user typed `coven` for).
pub(crate) fn parse_spell(raw: &str) -> Result<CastIntent> {
    let input = raw.trim();
    if input.is_empty() {
        return Ok(CastIntent::OpenTui);
    }

    if let Some(slash_intent) = parse_slash_command(input)? {
        return Ok(slash_intent);
    }

    if let Some(plain_intent) = parse_plain_command(input) {
        return Ok(plain_intent);
    }

    if let Some(quest_intent) = parse_natural_quest_trigger(input) {
        return Ok(quest_intent);
    }

    if let Some(harness_spell) = parse_natural_harness_prefix(input) {
        return Ok(harness_spell);
    }

    Ok(CastIntent::NaturalSpell {
        prompt: input.to_string(),
    })
}

fn parse_slash_command(input: &str) -> Result<Option<CastIntent>> {
    if !input.starts_with('/') {
        return Ok(None);
    }

    let (command, rest) = split_first_token(input);
    let intent = match command {
        "/start" => CastIntent::StartHere,
        "/help" => CastIntent::Help,
        "/tui" => CastIntent::OpenTui,
        "/doctor" => CastIntent::Doctor,
        "/daemon" => CastIntent::DaemonStatus,
        "/patch" => CastIntent::PatchOpenClaw,
        "/sessions" => CastIntent::OpenSessions,
        "/all" => CastIntent::OpenAllSessions,
        "/run" => parse_run_slash(rest)?,
        "/codex" => parse_harness_slash(CastHarness::Codex, rest)?,
        "/claude" => parse_harness_slash(CastHarness::Claude, rest)?,
        "/attach" => session_id_intent(rest, "/attach", |session_id| CastIntent::AttachSession {
            session_id,
        })?,
        "/summon" => session_id_intent(rest, "/summon", |session_id| CastIntent::SummonSession {
            session_id,
        })?,
        "/archive" => session_id_intent(rest, "/archive", |session_id| {
            CastIntent::ArchiveSession { session_id }
        })?,
        "/kill" => session_id_intent(rest, "/kill", |session_id| CastIntent::KillSession {
            session_id,
        })?,
        "/sacrifice" => session_id_intent(rest, "/sacrifice", |session_id| {
            CastIntent::SacrificeSession { session_id }
        })?,
        "/quest" => parse_quest_slash(rest)?,
        "/quit" | "/exit" => CastIntent::Quit,
        unknown => {
            return Err(anyhow!(
                "unknown Cast slash command `{unknown}`. Type `/help` to see what Cast knows."
            ));
        }
    };
    Ok(Some(intent))
}

fn parse_plain_command(input: &str) -> Option<CastIntent> {
    match input.to_ascii_lowercase().as_str() {
        "sessions" | "session" | "list sessions" | "show sessions" => {
            Some(CastIntent::OpenSessions)
        }
        "all sessions" | "show all sessions" => Some(CastIntent::OpenAllSessions),
        "doctor" | "status" | "health" => Some(CastIntent::Doctor),
        "daemon" | "daemon status" => Some(CastIntent::DaemonStatus),
        "help" | "?" => Some(CastIntent::Help),
        "quit" | "exit" | "q" | "bye" => Some(CastIntent::Quit),
        "tui" | "menu" | "home" => Some(CastIntent::OpenTui),
        _ => None,
    }
}

/// Recognise plain-language quest triggers. Returns the *original-case*
/// goal text so the rest of the pipeline can render the user's words back
/// at them. Matches `start a quest to …`, `begin a quest to …`, and the
/// shorter `quest <goal>` (must have at least one whitespace separator so
/// a bare `quest` keyword is unambiguous — currently unclaimed).
fn parse_natural_quest_trigger(input: &str) -> Option<CastIntent> {
    let lower = input.to_ascii_lowercase();
    let triggers = [
        "start a quest to ",
        "start a quest for ",
        "begin a quest to ",
        "begin a quest for ",
        "quest to ",
        "quest for ",
        "quest: ",
    ];
    for trigger in triggers {
        if let Some(rest) = lower.strip_prefix(trigger) {
            let goal = input[trigger.len()..].trim();
            // Defensive: lower-only strip can desync from original casing
            // when whitespace differs. `rest` is just the length anchor.
            let _ = rest;
            if goal.is_empty() {
                return None;
            }
            return Some(CastIntent::Quest {
                goal: goal.to_string(),
            });
        }
    }
    None
}

/// Translate plain-language "run claude X" / "use codex X" / "ask codex X"
/// into an explicit `HarnessSpell`. The verb itself is dropped from the
/// prompt so the harness only sees the actual task.
fn parse_natural_harness_prefix(input: &str) -> Option<CastIntent> {
    let lower = input.to_ascii_lowercase();
    for verb in ["run ", "use ", "ask ", "open ", "launch "] {
        if let Some(rest) = lower.strip_prefix(verb) {
            let raw_rest = &input[verb.len()..];
            let (harness_token, prompt_rest) = split_first_token(rest);
            let Some(harness) = CastHarness::from_token(harness_token) else {
                continue;
            };
            // Recover the original-cased prompt text from the user input.
            let original_remainder = raw_rest
                .get(harness_token.len()..)
                .map(|s| s.trim())
                .unwrap_or("");
            if original_remainder.is_empty() && prompt_rest.is_empty() {
                // "run claude" with no task is just an action; route as harness with no prompt is
                // ambiguous, so fall back to natural spell so the user gets a clear error.
                return None;
            }
            return Some(CastIntent::HarnessSpell {
                harness,
                prompt: original_remainder.to_string(),
            });
        }
    }
    None
}

fn parse_run_slash(rest: &str) -> Result<CastIntent> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(anyhow!(
            "`/run` needs a harness and a task. Example: `/run codex fix the failing tests`."
        ));
    }
    let (first, remainder) = split_first_token(rest);
    if let Some(harness) = CastHarness::from_token(first) {
        let prompt = remainder.trim();
        if prompt.is_empty() {
            return Err(anyhow!(
                "`/run {first}` needs a task. Example: `/run {first} explain this repo`."
            ));
        }
        return Ok(CastIntent::HarnessSpell {
            harness,
            prompt: prompt.to_string(),
        });
    }
    // Treat the whole `/run …` body as a natural spell when no harness is named,
    // so the user can still pass through to the default harness.
    Ok(CastIntent::NaturalSpell {
        prompt: rest.to_string(),
    })
}

fn parse_quest_slash(rest: &str) -> Result<CastIntent> {
    let goal = rest.trim();
    if goal.is_empty() {
        return Err(anyhow!(
            "`/quest` needs a goal. Example: `/quest fix the failing tests`."
        ));
    }
    Ok(CastIntent::Quest {
        goal: goal.to_string(),
    })
}

fn parse_harness_slash(harness: CastHarness, rest: &str) -> Result<CastIntent> {
    let prompt = rest.trim();
    if prompt.is_empty() {
        return Err(anyhow!(
            "`/{}` needs a task. Example: `/{} polish the README`.",
            harness.id(),
            harness.id()
        ));
    }
    Ok(CastIntent::HarnessSpell {
        harness,
        prompt: prompt.to_string(),
    })
}

fn session_id_intent<F>(rest: &str, command: &str, build: F) -> Result<CastIntent>
where
    F: FnOnce(String) -> CastIntent,
{
    let session_id = rest.trim();
    if session_id.is_empty() {
        return Err(anyhow!(
            "`{command}` needs a session id. Use `/sessions` to find one."
        ));
    }
    Ok(build(session_id.to_string()))
}

fn split_first_token(input: &str) -> (&str, &str) {
    match input.find(char::is_whitespace) {
        Some(index) => (&input[..index], input[index..].trim_start()),
        None => (input, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(raw: &str) -> CastIntent {
        parse_spell(raw).expect("parse should succeed")
    }

    #[test]
    fn empty_input_opens_launcher() {
        assert_eq!(intent(""), CastIntent::OpenTui);
        assert_eq!(intent("   "), CastIntent::OpenTui);
    }

    #[test]
    fn plain_text_is_a_natural_spell() {
        assert_eq!(
            intent("fix the failing tests"),
            CastIntent::NaturalSpell {
                prompt: "fix the failing tests".to_string()
            }
        );
    }

    #[test]
    fn run_claude_plain_language_selects_claude() {
        assert_eq!(
            intent("run claude polish the README"),
            CastIntent::HarnessSpell {
                harness: CastHarness::Claude,
                prompt: "polish the README".to_string(),
            }
        );
    }

    #[test]
    fn run_codex_plain_language_selects_codex() {
        assert_eq!(
            intent("run codex explain this repo"),
            CastIntent::HarnessSpell {
                harness: CastHarness::Codex,
                prompt: "explain this repo".to_string(),
            }
        );
    }

    #[test]
    fn use_or_ask_prefixes_also_select_a_harness() {
        assert_eq!(
            intent("use claude review the latest diff"),
            CastIntent::HarnessSpell {
                harness: CastHarness::Claude,
                prompt: "review the latest diff".to_string(),
            }
        );
        assert_eq!(
            intent("ask codex draft a release note"),
            CastIntent::HarnessSpell {
                harness: CastHarness::Codex,
                prompt: "draft a release note".to_string(),
            }
        );
    }

    #[test]
    fn run_without_harness_keyword_falls_through_to_natural_spell() {
        assert_eq!(
            intent("run the failing tests once more"),
            CastIntent::NaturalSpell {
                prompt: "run the failing tests once more".to_string(),
            }
        );
    }

    #[test]
    fn run_with_harness_but_no_task_is_a_natural_spell() {
        // Bare "run claude" is too ambiguous to launch — leave it as a natural
        // spell so the default-harness path can decide.
        assert_eq!(
            intent("run claude"),
            CastIntent::NaturalSpell {
                prompt: "run claude".to_string()
            }
        );
    }

    #[test]
    fn plain_keyword_sessions_opens_browser() {
        assert_eq!(intent("sessions"), CastIntent::OpenSessions);
        assert_eq!(intent("Sessions"), CastIntent::OpenSessions);
        assert_eq!(intent("show sessions"), CastIntent::OpenSessions);
    }

    #[test]
    fn plain_keyword_doctor_runs_doctor() {
        assert_eq!(intent("doctor"), CastIntent::Doctor);
        assert_eq!(intent("DOCTOR"), CastIntent::Doctor);
    }

    #[test]
    fn plain_keyword_help_opens_help() {
        assert_eq!(intent("help"), CastIntent::Help);
        assert_eq!(intent("?"), CastIntent::Help);
    }

    #[test]
    fn plain_keyword_quit_quits() {
        assert_eq!(intent("quit"), CastIntent::Quit);
        assert_eq!(intent("exit"), CastIntent::Quit);
        assert_eq!(intent("q"), CastIntent::Quit);
    }

    #[test]
    fn slash_commands_map_to_their_intents() {
        assert_eq!(intent("/sessions"), CastIntent::OpenSessions);
        assert_eq!(intent("/all"), CastIntent::OpenAllSessions);
        assert_eq!(intent("/doctor"), CastIntent::Doctor);
        assert_eq!(intent("/daemon"), CastIntent::DaemonStatus);
        assert_eq!(intent("/help"), CastIntent::Help);
        assert_eq!(intent("/start"), CastIntent::StartHere);
        assert_eq!(intent("/tui"), CastIntent::OpenTui);
        assert_eq!(intent("/quit"), CastIntent::Quit);
        assert_eq!(intent("/exit"), CastIntent::Quit);
        assert_eq!(intent("/patch"), CastIntent::PatchOpenClaw);
    }

    #[test]
    fn slash_run_requires_a_harness_or_task() {
        let error = parse_spell("/run").unwrap_err();
        assert!(error.to_string().contains("/run` needs"));
    }

    #[test]
    fn slash_run_codex_with_task_selects_codex() {
        assert_eq!(
            intent("/run codex explain this repo"),
            CastIntent::HarnessSpell {
                harness: CastHarness::Codex,
                prompt: "explain this repo".to_string(),
            }
        );
    }

    #[test]
    fn slash_claude_requires_a_task() {
        let error = parse_spell("/claude").unwrap_err();
        assert!(error.to_string().contains("needs a task"));
    }

    #[test]
    fn slash_attach_requires_session_id() {
        let error = parse_spell("/attach").unwrap_err();
        assert!(error.to_string().contains("session id"));

        assert_eq!(
            intent("/attach abc123"),
            CastIntent::AttachSession {
                session_id: "abc123".to_string()
            }
        );
    }

    #[test]
    fn slash_summon_archive_sacrifice_take_session_ids() {
        assert_eq!(
            intent("/summon abc"),
            CastIntent::SummonSession {
                session_id: "abc".to_string()
            }
        );
        assert_eq!(
            intent("/archive abc"),
            CastIntent::ArchiveSession {
                session_id: "abc".to_string()
            }
        );
        assert_eq!(
            intent("/kill abc"),
            CastIntent::KillSession {
                session_id: "abc".to_string()
            }
        );
        assert_eq!(
            intent("/sacrifice abc"),
            CastIntent::SacrificeSession {
                session_id: "abc".to_string()
            }
        );
    }

    #[test]
    fn unknown_slash_is_an_error() {
        let error = parse_spell("/banana split").unwrap_err();
        assert!(error.to_string().contains("unknown Cast slash command"));
    }

    #[test]
    fn slash_quest_requires_a_goal() {
        let error = parse_spell("/quest").unwrap_err();
        assert!(error.to_string().contains("`/quest` needs a goal"));
        let error = parse_spell("/quest   ").unwrap_err();
        assert!(error.to_string().contains("`/quest` needs a goal"));
    }

    #[test]
    fn slash_quest_with_goal_produces_quest_intent() {
        assert_eq!(
            intent("/quest fix the failing tests"),
            CastIntent::Quest {
                goal: "fix the failing tests".to_string(),
            }
        );
    }

    #[test]
    fn natural_language_quest_triggers_produce_quest_intent() {
        let cases = [
            "start a quest to ship the redesign",
            "Begin a quest to ship the redesign",
            "quest to ship the redesign",
            "quest: ship the redesign",
        ];
        for raw in cases {
            assert_eq!(
                intent(raw),
                CastIntent::Quest {
                    goal: "ship the redesign".to_string(),
                },
                "raw input `{raw}` should parse as a quest",
            );
        }
    }

    #[test]
    fn bare_quest_keyword_without_goal_falls_through_to_natural_spell() {
        // "quest" alone is too ambiguous to launch — leave it as a natural
        // spell so the user sees what Cast would route a non-keyword
        // through. `/quest` (slash form) still errors clearly above.
        assert_eq!(
            intent("quest"),
            CastIntent::NaturalSpell {
                prompt: "quest".to_string(),
            }
        );
    }

    #[test]
    fn cast_harness_token_accepts_common_aliases() {
        assert_eq!(CastHarness::from_token("codex"), Some(CastHarness::Codex));
        assert_eq!(CastHarness::from_token("Codex"), Some(CastHarness::Codex));
        assert_eq!(CastHarness::from_token("claude"), Some(CastHarness::Claude));
        assert_eq!(
            CastHarness::from_token("claude-code"),
            Some(CastHarness::Claude)
        );
        assert_eq!(CastHarness::from_token("hermes"), None);
    }
}
