use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use crossterm::{
    cursor::MoveTo,
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use uuid::Uuid;

use super::cast::{
    self, advance_quest, build_plan, evaluate_gate, find_cast_summary, follow_until_exit,
    format_summary_note, parse_phase_action, quest_from_goal, render_cast_frame_for_terminal,
    render_outcome, render_plan_intro, render_quest_handoff, set_phase_sub_prompt, skip_phase,
    CastHarness, CastIntent, CastOutcome, CastPlan, CastSessionExit, FollowerObserver,
    FollowerPacer, GateOutcome, PhaseInteraction, Quest, QuestPhase, QuestPhaseStatus,
    QuestPhaseSummary, SafetyDecision,
};
use super::chat::client::{ChatClient, ChatEventQuery, DaemonChatClient, LaunchRequest};
use super::{is_key_press, sessions};
use crate::{
    archive_session_command, attach_session, coven_home_dir, coven_store_path, current_timestamp,
    daemon, default_harness_id, project, prompt_for_optional_line, prompt_for_required_line,
    run_daemon_command, run_doctor, run_patch_openclaw, run_session, sacrifice_session_command,
    store, summon_only_command, theme, DaemonCommand, RUNNING_SESSION_STATUS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MagicalTuiAction {
    StartHere,
    Help,
    OpenTui,
    Doctor,
    DaemonStatus,
    RunHarness,
    PatchOpenClaw,
    Sessions,
    AllSessions,
    AttachSession,
    SummonSession,
    ArchiveSession,
    SacrificeSession,
    Quit,
}

/// Legacy palette-input parser type. Phase 2 routes typed input through
/// `cast::parse_spell` directly, but `parse_magical_tui_input` and this
/// enum remain available to the test suite as an independent record of the
/// slash-command surface area. Not used in production builds.
#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MagicalTuiRequest {
    Action(MagicalTuiAction),
    NaturalPrompt(String),
    HarnessPrompt { harness: String, prompt: String },
    AttachSession(String),
    SummonSession(String),
    ArchiveSession(String),
    SacrificeSession(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MagicalTuiMove {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MagicalTuiItem {
    pub(crate) key: &'static str,
    pub(crate) slash: &'static str,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) command: &'static str,
    pub(crate) action: MagicalTuiAction,
}

const MAGICAL_TUI_DEFAULT_INNER_WIDTH: usize = 76;
pub(crate) const MAGICAL_TUI_MAX_INNER_WIDTH: usize = 96;
const MAGICAL_TUI_MIN_INNER_WIDTH: usize = 40;

/// Restores terminal raw mode on all exit paths for the slash-command launcher.
struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enter Coven's magical terminal mode")?;
        Ok(Self { enabled: true })
    }

    fn restore(&mut self) -> Result<()> {
        if self.enabled {
            disable_raw_mode().context("failed to leave Coven's magical terminal mode")?;
            self.enabled = false;
        }
        Ok(())
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = disable_raw_mode();
        }
    }
}

pub(crate) fn magical_tui_items() -> &'static [MagicalTuiItem] {
    &[
        MagicalTuiItem {
            key: "1",
            slash: "/start",
            label: "Start here",
            description: "Setup check and a safe first command",
            command: "coven doctor",
            action: MagicalTuiAction::StartHere,
        },
        MagicalTuiItem {
            key: "h",
            slash: "/help",
            label: "Help",
            description: "Show natural-language and slash-command examples",
            command: "type a task or /run codex <task>",
            action: MagicalTuiAction::Help,
        },
        MagicalTuiItem {
            key: "0",
            slash: "/tui",
            label: "Open TUI",
            description: "Launch this slash-command palette explicitly",
            command: "coven tui",
            action: MagicalTuiAction::OpenTui,
        },
        MagicalTuiItem {
            key: "2",
            slash: "/doctor",
            label: "Doctor",
            description: "Check store, project, and harness readiness",
            command: "coven doctor",
            action: MagicalTuiAction::Doctor,
        },
        MagicalTuiItem {
            key: "3",
            slash: "/daemon",
            label: "Daemon status",
            description: "See whether the local Coven daemon is awake",
            command: "coven daemon status",
            action: MagicalTuiAction::DaemonStatus,
        },
        MagicalTuiItem {
            key: "4",
            slash: "/run",
            label: "Run an agent",
            description: "Launch Codex or Claude Code inside this project",
            command: "coven run codex \"fix the failing tests\"",
            action: MagicalTuiAction::RunHarness,
        },
        MagicalTuiItem {
            key: "5",
            slash: "/patch",
            label: "Patch OpenClaw",
            description: "Guided repair room for a local OpenClaw checkout",
            command: "coven patch openclaw",
            action: MagicalTuiAction::PatchOpenClaw,
        },
        MagicalTuiItem {
            key: "6",
            slash: "/sessions",
            label: "Active sessions",
            description: "List live, non-archived Coven sessions",
            command: "coven sessions --manage",
            action: MagicalTuiAction::Sessions,
        },
        MagicalTuiItem {
            key: "7",
            slash: "/all",
            label: "All sessions",
            description: "List active and archived sessions together",
            command: "coven sessions --all --manage",
            action: MagicalTuiAction::AllSessions,
        },
        MagicalTuiItem {
            key: "8",
            slash: "/attach",
            label: "Attach session",
            description: "Replay/follow a session by id",
            command: "coven attach <session-id>",
            action: MagicalTuiAction::AttachSession,
        },
        MagicalTuiItem {
            key: "9",
            slash: "/summon",
            label: "Summon session",
            description: "Restore an archived session, then follow it",
            command: "coven summon <session-id>",
            action: MagicalTuiAction::SummonSession,
        },
        MagicalTuiItem {
            key: "a",
            slash: "/archive",
            label: "Archive session",
            description: "Hide completed work without deleting events",
            command: "coven archive <session-id>",
            action: MagicalTuiAction::ArchiveSession,
        },
        MagicalTuiItem {
            key: "x",
            slash: "/sacrifice",
            label: "Sacrifice session",
            description: "Permanently delete a non-running session",
            command: "coven sacrifice <session-id> --yes",
            action: MagicalTuiAction::SacrificeSession,
        },
        MagicalTuiItem {
            key: "q",
            slash: "/quit",
            label: "Quit",
            description: "Exit without changing anything",
            command: "q",
            action: MagicalTuiAction::Quit,
        },
    ]
}

pub(crate) fn run() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        print_cast_non_interactive_frame();
        return Ok(());
    }

    let mut selection = 0;
    let mut input = String::new();
    let mut raw_mode = RawModeGuard::enter()?;
    let choice = loop {
        execute!(io::stdout(), Clear(ClearType::All), MoveTo(0, 0))
            .context("failed to redraw Coven menu")?;
        print!(
            "{}",
            render_magical_tui_frame_for_raw_terminal(selection, &input)
        );
        io::stdout().flush().context("failed to flush Coven menu")?;

        if let Event::Key(key) = event::read().context("failed to read Coven menu input")? {
            if !is_key_press(key.kind) {
                continue;
            }
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break LauncherChoice::Palette(MagicalTuiAction::Quit);
                }
                KeyCode::Up => {
                    selection = move_magical_tui_selection(selection, MagicalTuiMove::Up);
                }
                KeyCode::Down => {
                    selection = move_magical_tui_selection(selection, MagicalTuiMove::Down);
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.clear();
                }
                KeyCode::Enter => {
                    if input.trim().is_empty() {
                        break LauncherChoice::Palette(magical_tui_items()[selection].action);
                    }
                    break LauncherChoice::TypedSpell(input.clone());
                }
                KeyCode::Char(value) => {
                    input.push(value);
                }
                KeyCode::Esc => break LauncherChoice::Palette(MagicalTuiAction::Quit),
                _ => {}
            }
        }
    };
    raw_mode.restore()?;
    println!();

    // Palette buttons keep their existing direct dispatch. Typed input — slash
    // commands and free text alike — flows through Cast so that the same
    // intent parser, safety gate, and outcome card apply to every spell.
    match choice {
        LauncherChoice::Palette(action) => run_magical_tui_action(action),
        LauncherChoice::TypedSpell(raw) => run_cast_spell(&raw),
    }
}

/// What the launcher loop decided to dispatch after the user pressed Enter
/// (or Esc / Ctrl+C). Typed input always flows through Cast; palette buttons
/// stay on the direct `run_magical_tui_action` path.
enum LauncherChoice {
    Palette(MagicalTuiAction),
    TypedSpell(String),
}

/// Cast entry point for free-text spells. Parses the raw input into a
/// `CastIntent`, builds a plan, renders the intro card the user sees before
/// any side effect, runs the safety gate, dispatches the matching handler,
/// then renders the outcome card.
fn run_cast_spell(raw: &str) -> Result<()> {
    let plan = cast::plan_spell(raw)?;
    print_plan_intro(&plan);
    dispatch_cast_plan(plan)
}

/// Cast's dispatcher. Runs the safety gate against the plan, then routes
/// safe / confirmed plans to the existing CLI handlers. All destructive or
/// confirmation-required side effects are guarded by the gate; the
/// dispatcher itself does no risk classification.
fn dispatch_cast_plan(plan: CastPlan) -> Result<()> {
    let mut reader = stdin_line_reader;
    match evaluate_gate(&plan, &mut reader)? {
        GateOutcome::Cancelled { reason, next_step } => {
            print_outcome(&plan_cancelled_outcome(&plan, &reason, next_step));
            return Ok(());
        }
        GateOutcome::Proceed => {}
    }

    let request_text = outcome_request_text(&plan);

    let outcome = match plan.intent.clone() {
        CastIntent::NaturalSpell { prompt } => dispatch_default_spell(&plan, &prompt)?,
        CastIntent::HarnessSpell { harness, prompt } => {
            dispatch_harness_spell(&plan, harness.id(), &prompt)?
        }
        CastIntent::OpenSessions => {
            sessions::run_browser(false)?;
            CastOutcome {
                request: request_text,
                launched: Some("Coven session browser (active)".to_string()),
                session_id: None,
                next_step: Some(
                    "Use the browser actions (Rejoin, View Log, Summon, Archive, Sacrifice)."
                        .to_string(),
                ),
                notes: vec![],
            }
        }
        CastIntent::OpenAllSessions => {
            sessions::run_browser(true)?;
            CastOutcome {
                request: request_text,
                launched: Some("Coven session browser (active + archived)".to_string()),
                session_id: None,
                next_step: Some(
                    "Use the browser actions to summon archived sessions or archive completed ones."
                        .to_string(),
                ),
                notes: vec![],
            }
        }
        CastIntent::AttachSession { session_id } => {
            attach_via_cast(&plan, &session_id, &request_text, AttachOrigin::Attach)?
        }
        CastIntent::SummonSession { session_id } => {
            // Un-archive first (cheap, no follower), then re-enter through Cast
            // so the resumed session also gets the Cast transcript / summary.
            summon_only_command(&session_id)?;
            attach_via_cast(&plan, &session_id, &request_text, AttachOrigin::Summon)?
        }
        CastIntent::ArchiveSession { session_id } => {
            archive_session_command(&session_id)?;
            CastOutcome {
                request: request_text,
                launched: Some(format!("Archived session {session_id}")),
                session_id: Some(session_id),
                next_step: Some(
                    "Use `/summon <id>` later to restore it; events are preserved.".to_string(),
                ),
                notes: vec![],
            }
        }
        CastIntent::SacrificeSession { session_id } => {
            // The gate already collected the typed `sacrifice` confirmation;
            // calling the underlying handler with `--yes` is the documented
            // way to bypass its own redundant prompt.
            sacrifice_session_command(&session_id, true)?;
            CastOutcome {
                request: request_text,
                launched: Some(format!("Sacrificed session {session_id}")),
                session_id: Some(session_id),
                next_step: None,
                notes: vec!["Sacrifice permanently deleted the session and its events.".to_string()],
            }
        }
        CastIntent::Doctor => {
            run_doctor()?;
            CastOutcome {
                request: request_text,
                launched: Some("Coven doctor".to_string()),
                session_id: None,
                next_step: Some(
                    "Install or auth any missing harness, then retry your spell.".to_string(),
                ),
                notes: vec![],
            }
        }
        CastIntent::DaemonStatus => {
            run_daemon_command(DaemonCommand::Status)?;
            CastOutcome {
                request: request_text,
                launched: Some("Coven daemon status".to_string()),
                session_id: None,
                next_step: Some("Run `coven daemon start` if status reported stopped.".to_string()),
                notes: vec![],
            }
        }
        CastIntent::Help => {
            run_tui_help()?;
            CastOutcome::for_request(request_text)
        }
        CastIntent::StartHere => {
            run_new_user_start_here()?;
            CastOutcome::for_request(request_text)
        }
        CastIntent::OpenTui => {
            // Already in the launcher — show the palette help instead of
            // re-entering the raw-mode loop.
            run_tui_help()?;
            CastOutcome::for_request(request_text)
        }
        CastIntent::PatchOpenClaw => {
            run_patch_openclaw(vec![], None, None, None, false, false, true)?;
            CastOutcome::for_request(request_text)
        }
        CastIntent::Quest { goal } => dispatch_cast_quest(&plan, &goal)?,
        CastIntent::Quit => {
            let primary = theme::fg(theme::PRIMARY);
            let reset = theme::reset();
            println!("{primary}The circle fades. Nothing changed.{reset}");
            return Ok(());
        }
    };

    print_outcome(&outcome);
    Ok(())
}

/// The line the outcome card shows as the user's request. We prefer the raw
/// spell text (what the user actually typed) so cancellations and successes
/// look the same; we only fall back to the plan headline for plans that were
/// not built from raw input (Phase 1 doesn't create those, but the
/// defensive default keeps the field non-empty).
fn outcome_request_text(plan: &CastPlan) -> String {
    if plan.raw_spell.is_empty() {
        plan.headline.clone()
    } else {
        plan.raw_spell.clone()
    }
}

/// Default stdin reader for the safety gate. Treats empty input as cancel
/// (returns `""`) instead of bubbling an error so the gate can render a
/// clean "cancelled" outcome.
fn stdin_line_reader(prompt: &str) -> Result<String> {
    Ok(prompt_for_optional_line(prompt)?.unwrap_or_default())
}

fn plan_cancelled_outcome(plan: &CastPlan, reason: &str, next_step: Option<String>) -> CastOutcome {
    CastOutcome {
        request: outcome_request_text(plan),
        launched: None,
        session_id: None,
        next_step,
        notes: vec![reason.to_string()],
    }
}

fn dispatch_default_spell(plan: &CastPlan, prompt: &str) -> Result<CastOutcome> {
    let Some(plan_harness) = plan.harness else {
        return Err(anyhow!(
            "no supported harness is available; run `coven doctor` first"
        ));
    };
    dispatch_cast_launch(
        plan,
        plan_harness.harness.id(),
        prompt,
        format!(
            "{} session (Cast default, project-scoped)",
            plan_harness.harness.label()
        ),
    )
}

fn dispatch_harness_spell(plan: &CastPlan, harness_id: &str, prompt: &str) -> Result<CastOutcome> {
    dispatch_cast_launch(
        plan,
        harness_id,
        prompt,
        format!(
            "{} session (user-chosen, project-scoped)",
            harness_label(harness_id)
        ),
    )
}

/// Event kind prefix Cast writes to the anchor session's event log so a
/// future `coven attach <anchor>` can detect that the session belongs to a
/// quest. Per the Phase 7 scope decision (first-session anchor), every
/// `cast.quest.*` event lives on phase-0's session; later phases still
/// emit their own `cast.summary` events on their own sessions.
const CAST_QUEST_STARTED: &str = "cast.quest.started";
const CAST_QUEST_PHASE_STARTED: &str = "cast.quest.phase_started";
const CAST_QUEST_PHASE_COMPLETED: &str = "cast.quest.phase_completed";
const CAST_QUEST_PHASE_SKIPPED: &str = "cast.quest.phase_skipped";
const CAST_QUEST_ADVANCED: &str = "cast.quest.advanced";
const CAST_QUEST_COMPLETED: &str = "cast.quest.completed";

/// Loop a deterministic Cast quest through its phases. Each iteration
/// renders the handoff card, gates the phase's sub-prompt, dispatches via
/// `dispatch_cast_launch`, then advances the quest with a summary built
/// from the session's `cast.summary` event. `cast.quest.*` events are
/// written to the anchor (first-phase) session as a re-attach aid; they
/// are best-effort and skipped silently when the daemon is not running
/// (no session id means no anchor and no ledger writes).
fn dispatch_cast_quest(plan: &CastPlan, goal: &str) -> Result<CastOutcome> {
    let default_harness = plan.harness.map(|h| h.harness);
    let mut quest = quest_from_goal(goal, default_harness);
    let mut anchor_session_id: Option<String> = None;
    let mut completed_notes: Vec<String> = Vec::new();
    let request_text = outcome_request_text(plan);

    while let Some(idx) = quest.current_index() {
        // Phases the user (or a prior skip_phase call) already marked as
        // Skipped are walked past without re-prompting. The cursor was
        // already nudged forward by `skip_phase`, but defensively handle
        // the case here so a future async UX that bumps `cursor` without
        // calling `skip_phase` still behaves.
        if matches!(quest.phases[idx].status, QuestPhaseStatus::Skipped { .. }) {
            quest.cursor = idx + 1;
            continue;
        }

        println!();
        print!("{}", render_quest_handoff(&quest, idx));
        println!();

        let mut reader = stdin_line_reader;
        match run_phase_interaction(&mut quest, idx, &mut reader)? {
            PhaseInteraction::Cancel { reason } => {
                let phase_name = quest.phases[idx].name.clone();
                completed_notes.push(format!("Phase `{phase_name}` cancelled: {reason}"));
                return Ok(quest_outcome(
                    &request_text,
                    &quest,
                    anchor_session_id.clone(),
                    completed_notes,
                    Some("Re-run `/quest <goal>` to start over.".to_string()),
                    Some(format!(
                        "Quest cancelled at phase `{phase_name}` — {reason}."
                    )),
                ));
            }
            PhaseInteraction::Skip { reason } => {
                let phase_name = quest.phases[idx].name.clone();
                skip_phase(&mut quest, idx, reason.clone())?;
                completed_notes.push(format!("Phase `{phase_name}`: skipped — {reason}"));
                if let Some(anchor) = anchor_session_id.as_deref() {
                    write_quest_event(
                        anchor,
                        CAST_QUEST_PHASE_SKIPPED,
                        serde_json::json!({
                            "phase": phase_name,
                            "index": idx,
                            "reason": reason,
                        }),
                    );
                }
                continue;
            }
            PhaseInteraction::Edit { .. } => {
                unreachable!("Edit is consumed inside run_phase_interaction's inner loop")
            }
            PhaseInteraction::Approve => {}
        }

        let phase_harness = match resolve_phase_harness(&quest.phases[idx], default_harness) {
            Some(harness) => harness,
            None => {
                return Ok(quest_outcome(
                    &request_text,
                    &quest,
                    anchor_session_id.clone(),
                    completed_notes,
                    Some(
                        "No harness ready. Run `coven doctor`, then retry `/quest <goal>`."
                            .to_string(),
                    ),
                    Some("Quest paused — no harness available for the next phase.".to_string()),
                ));
            }
        };

        let phase_plan = quest_phase_plan(&quest, idx, phase_harness, goal)?;

        match evaluate_gate(&phase_plan, &mut reader)? {
            GateOutcome::Cancelled { reason, next_step } => {
                completed_notes.push(format!(
                    "Phase `{}` cancelled: {reason}",
                    quest.phases[idx].name
                ));
                return Ok(quest_outcome(
                    &request_text,
                    &quest,
                    anchor_session_id.clone(),
                    completed_notes,
                    next_step,
                    Some(format!(
                        "Quest cancelled at phase `{}`.",
                        quest.phases[idx].name
                    )),
                ));
            }
            GateOutcome::Proceed => {}
        }

        let phase_outcome = dispatch_cast_launch(
            &phase_plan,
            phase_harness.id(),
            &quest.phases[idx].sub_prompt,
            format!(
                "Quest phase `{}` ({})",
                quest.phases[idx].name,
                phase_harness.label()
            ),
        )?;

        let phase_session_id = phase_outcome.session_id.clone();
        if anchor_session_id.is_none() {
            if let Some(sid) = &phase_session_id {
                anchor_session_id = Some(sid.clone());
                write_quest_event(
                    sid,
                    CAST_QUEST_STARTED,
                    serde_json::json!({
                        "title": quest.title,
                        "goal": quest.goal,
                        "harness": phase_harness.id(),
                        "phases": quest
                            .phases
                            .iter()
                            .map(|p| p.name.clone())
                            .collect::<Vec<_>>(),
                    }),
                );
            }
        }

        if let Some(anchor) = anchor_session_id.as_deref() {
            write_quest_event(
                anchor,
                CAST_QUEST_PHASE_STARTED,
                serde_json::json!({
                    "phase": quest.phases[idx].name,
                    "index": idx,
                    "session_id": phase_session_id,
                    "harness": phase_harness.id(),
                }),
            );
        }

        let summary = phase_summary_from_session(phase_session_id.as_deref());
        completed_notes.push(format_phase_note(&quest.phases[idx].name, &summary));

        if let Some(anchor) = anchor_session_id.as_deref() {
            write_quest_event(
                anchor,
                CAST_QUEST_PHASE_COMPLETED,
                serde_json::json!({
                    "phase": quest.phases[idx].name,
                    "index": idx,
                    "session_id": summary.session_id,
                    "exit_status": summary.exit_status,
                    "exit_code": summary.exit_code,
                }),
            );
        }

        let next = advance_quest(&mut quest, summary);
        if let Some(anchor) = anchor_session_id.as_deref() {
            write_quest_event(
                anchor,
                CAST_QUEST_ADVANCED,
                serde_json::json!({
                    "from_index": idx,
                    "next_index": next,
                }),
            );
        }
    }

    if let Some(anchor) = anchor_session_id.as_deref() {
        write_quest_event(
            anchor,
            CAST_QUEST_COMPLETED,
            serde_json::json!({
                "title": quest.title,
                "phase_count": quest.phases.len(),
            }),
        );
    }

    Ok(quest_outcome(
        &request_text,
        &quest,
        anchor_session_id,
        completed_notes,
        Some(
            "Use `/sessions` to inspect each phase, or `/attach <anchor-id>` to read the quest event log."
                .to_string(),
        ),
        Some(format!("Quest `{}` complete.", quest.title)),
    ))
}

/// Each phase prefers the harness recorded on the phase (so a future UX
/// could let the user route phase N to a different harness) and falls back
/// to the quest-wide default. Returns `None` only when neither is set.
fn resolve_phase_harness(
    phase: &QuestPhase,
    default_harness: Option<CastHarness>,
) -> Option<CastHarness> {
    phase.harness.or(default_harness)
}

/// One line of phase-prompt help, matching the handoff card's footer hint.
/// The wording is duplicated here (the card is rendered separately) but
/// kept short and contract-aligned per §2.6 of `cast-tui-contract.md`.
const PHASE_PROMPT_HINT: &str =
    "enter approve · type to replace · /skip [reason] · /cancel [reason]";

/// Drive the per-phase interaction described in
/// `docs/design/cast-quest-flow.md` §5: render → prompt → loop on edits →
/// resolve to Approve / Skip / Cancel.
///
/// `Edit` is consumed inside this function (the handoff card re-renders
/// with the new sub-prompt and the user is prompted again) so callers
/// only see one of the three terminal actions.
fn run_phase_interaction<R>(
    quest: &mut Quest,
    idx: usize,
    reader: &mut R,
) -> Result<PhaseInteraction>
where
    R: FnMut(&str) -> Result<String>,
{
    loop {
        let phase_name = quest.phases[idx].name.clone();
        let prompt = format!("Phase `{phase_name}` — {PHASE_PROMPT_HINT}: ");
        let line = reader(&prompt)?;
        match parse_phase_action(&line) {
            PhaseInteraction::Edit { sub_prompt } => {
                set_phase_sub_prompt(quest, idx, sub_prompt)?;
                println!();
                print!("{}", render_quest_handoff(quest, idx));
                println!();
            }
            terminal => return Ok(terminal),
        }
    }
}

/// Synthesize a per-phase `CastPlan` so the existing safety gate can vet
/// the resolved sub-prompt before each launch. The plan is built from a
/// `HarnessSpell` intent so the classifier reads the *sub-prompt* content,
/// not the original quest goal verbatim.
fn quest_phase_plan(
    quest: &Quest,
    idx: usize,
    harness: CastHarness,
    goal: &str,
) -> Result<CastPlan> {
    let phase = &quest.phases[idx];
    let intent = CastIntent::HarnessSpell {
        harness,
        prompt: phase.sub_prompt.clone(),
    };
    Ok(
        build_plan(intent, || Some(harness))?.with_raw_spell(format!(
            "/quest {goal} (phase {phase_num}/{total}: {name})",
            phase_num = idx + 1,
            total = quest.phases.len(),
            name = phase.name,
        )),
    )
}

/// Build a `QuestPhaseSummary` from the session's `cast.summary` event.
/// Returns a default summary (no exit info) when the session id is absent
/// (local-PTY fallback) or the store cannot be opened.
fn phase_summary_from_session(session_id: Option<&str>) -> QuestPhaseSummary {
    let Some(sid) = session_id else {
        return QuestPhaseSummary::default();
    };
    let Ok(store_path) = coven_store_path() else {
        return QuestPhaseSummary {
            session_id: Some(sid.to_string()),
            ..Default::default()
        };
    };
    let Ok(conn) = store::open_store(&store_path) else {
        return QuestPhaseSummary {
            session_id: Some(sid.to_string()),
            ..Default::default()
        };
    };
    let Ok(events) = store::list_events(&conn, sid) else {
        return QuestPhaseSummary {
            session_id: Some(sid.to_string()),
            ..Default::default()
        };
    };
    let summary = find_cast_summary(&events);
    QuestPhaseSummary {
        session_id: Some(sid.to_string()),
        exit_status: summary.as_ref().and_then(|s| s.status.clone()),
        exit_code: summary.as_ref().and_then(|s| s.exit_code),
        carried_context: Vec::new(),
    }
}

fn format_phase_note(name: &str, summary: &QuestPhaseSummary) -> String {
    match (&summary.exit_status, summary.exit_code) {
        (Some(status), Some(code)) => format!("Phase `{name}`: {status} (exit {code})"),
        (Some(status), None) => format!("Phase `{name}`: {status}"),
        (None, Some(code)) => format!("Phase `{name}`: exit {code}"),
        (None, None) => format!("Phase `{name}`: complete"),
    }
}

fn quest_outcome(
    request: &str,
    quest: &Quest,
    anchor_session_id: Option<String>,
    notes: Vec<String>,
    next_step: Option<String>,
    launched: Option<String>,
) -> CastOutcome {
    CastOutcome {
        request: request.to_string(),
        launched: launched.or_else(|| Some(format!("Quest `{}`", quest.title))),
        session_id: anchor_session_id,
        next_step,
        notes,
    }
}

/// Best-effort writer for `cast.quest.*` events. Failures are intentionally
/// swallowed: the harness output is the source of truth; these events are
/// reconstruction aids. The daemon may have a different connection open
/// simultaneously; SQLite's WAL/locking handles that.
fn write_quest_event(session_id: &str, kind: &str, payload: serde_json::Value) {
    let Ok(store_path) = coven_store_path() else {
        return;
    };
    let Ok(conn) = store::open_store(&store_path) else {
        return;
    };
    let _ = store::insert_json_event(&conn, session_id, kind, &payload, &current_timestamp());
}

/// Launch a project-scoped session and follow its events into the Cast TUI.
///
/// Phase 2 prefers the daemon-backed path so the follower can stream events
/// with `afterSeq`, surface the exit status + exit code in the outcome card,
/// and write a `cast.summary` event to the ledger when the run finishes. If
/// the daemon is not running Cast falls back to the synchronous local PTY
/// path Phase 1 used; the user can still read the transcript inline, but the
/// outcome card will note the missing daemon and skip the follower-only
/// fields.
fn dispatch_cast_launch(
    plan: &CastPlan,
    harness_id: &str,
    prompt: &str,
    launched_label: String,
) -> Result<CastOutcome> {
    match daemon_runtime_state()? {
        DaemonRuntimeState::Running => {
            dispatch_via_daemon(plan, harness_id, prompt, &launched_label)
        }
        DaemonRuntimeState::NotReady(reason) => {
            dispatch_via_local_pty(plan, harness_id, prompt, &launched_label, &reason)
        }
    }
}

enum DaemonRuntimeState {
    Running,
    NotReady(String),
}

fn daemon_runtime_state() -> Result<DaemonRuntimeState> {
    let home = coven_home_dir()?;
    match daemon::background_server_status(&home)? {
        Some(daemon::DaemonStatusState::Running(_)) => Ok(DaemonRuntimeState::Running),
        Some(daemon::DaemonStatusState::Stale(_)) => Ok(DaemonRuntimeState::NotReady(
            "the local Coven daemon is recorded but unreachable; run `coven daemon restart`"
                .to_string(),
        )),
        None => Ok(DaemonRuntimeState::NotReady(
            "the local Coven daemon is not running; run `coven daemon start` to enable Cast's \
             event follower"
                .to_string(),
        )),
    }
}

fn dispatch_via_daemon(
    plan: &CastPlan,
    harness_id: &str,
    prompt: &str,
    launched_label: &str,
) -> Result<CastOutcome> {
    let project_root = resolve_project_root_for_cast()?;
    let title = plan
        .title
        .clone()
        .unwrap_or_else(|| prompt.chars().take(48).collect());

    let launch = LaunchRequest {
        id: Uuid::new_v4().to_string(),
        project_root: project_root.to_string_lossy().into_owned(),
        cwd: project_root.to_string_lossy().into_owned(),
        harness: harness_id.to_string(),
        prompt: prompt.to_string(),
        title,
    };

    let mut client = DaemonChatClient::default();
    let session = client.launch_session(launch).with_context(|| {
        format!(
            "Cast failed to launch {} session via the daemon",
            harness_label(harness_id)
        )
    })?;

    println!();
    println!(
        "Cast transcript — session {} ({}). Press Enter at any time to send input.",
        session.id, session.harness
    );

    // Forward user keystrokes from this process into the live daemon
    // session so the user can follow up on a running spell without
    // detaching. The thread is detached and exits when stdin closes or
    // the host process exits.
    maybe_spawn_cast_input_forwarder(coven_home_dir()?, session.id.clone());

    let mut observer = TranscriptObserver::new(io::stdout());
    let mut pacer = SleepPacer::new(Duration::from_millis(250));
    let exit = follow_until_exit(&mut client, &session.id, &mut observer, &mut pacer)?;

    write_cast_summary_event(&session.id, plan, harness_id, &exit)?;

    let exit_summary = format_exit_summary(&exit);
    let next_step = format!(
        "Run `coven attach {}` to revisit, or `coven sessions` to list everything.",
        session.id
    );
    let mut notes = plan_outcome_notes(plan);
    notes.push(exit_summary.clone());

    Ok(CastOutcome {
        request: outcome_request_text(plan),
        launched: Some(format!("{launched_label} via daemon")),
        session_id: Some(session.id),
        next_step: Some(next_step),
        notes,
    })
}

fn dispatch_via_local_pty(
    plan: &CastPlan,
    harness_id: &str,
    prompt: &str,
    launched_label: &str,
    daemon_reason: &str,
) -> Result<CastOutcome> {
    let title = plan.title.as_deref();
    run_session(harness_id, &[prompt.to_string()], None, title, false)?;
    let mut notes = plan_outcome_notes(plan);
    notes.push(format!("Cast event follower skipped: {daemon_reason}."));
    Ok(CastOutcome {
        request: outcome_request_text(plan),
        launched: Some(format!("{launched_label} (local PTY fallback)")),
        session_id: None,
        next_step: Some(
            "Start the daemon for streamed transcripts: `coven daemon start`.".to_string(),
        ),
        notes,
    })
}

fn resolve_project_root_for_cast() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    project::canonical_project_root(&cwd).with_context(|| {
        format!(
            "Cast could not resolve a project root from {}. Run `coven` inside a repo.",
            cwd.display()
        )
    })
}

/// Stdout-backed observer that prints each output chunk verbatim and adds a
/// Cast-styled completion line when the session exits. Holds a writer so
/// tests can swap stdout for a buffer if they ever need to.
struct TranscriptObserver<W: Write> {
    out: W,
}

impl<W: Write> TranscriptObserver<W> {
    fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write> FollowerObserver for TranscriptObserver<W> {
    fn on_output(&mut self, chunk: &str) {
        // Swallow individual write errors — best-effort transcript should
        // not abort the follower because stdout is briefly unavailable.
        let _ = self.out.write_all(chunk.as_bytes());
        let _ = self.out.flush();
    }

    fn on_exit(&mut self, status: &str, exit_code: Option<i32>) {
        let summary = match exit_code {
            Some(code) => format!("\n[Cast: session {status} (exit code {code})]\n"),
            None => format!("\n[Cast: session {status}]\n"),
        };
        let _ = self.out.write_all(summary.as_bytes());
        let _ = self.out.flush();
    }
}

/// Sleep-based pacer for the production follower. Tests use the test pacer
/// in `cast::follow` so they never sleep.
struct SleepPacer {
    interval: Duration,
}

impl SleepPacer {
    fn new(interval: Duration) -> Self {
        Self { interval }
    }
}

impl FollowerPacer for SleepPacer {
    fn between_polls(&mut self) -> Result<()> {
        thread::sleep(self.interval);
        Ok(())
    }
}

fn maybe_spawn_cast_input_forwarder(coven_home: PathBuf, session_id: String) {
    if !io::stdin().is_terminal() {
        return;
    }
    thread::spawn(move || {
        let mut client = DaemonChatClient::with_coven_home(coven_home);
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let Ok(mut line) = line else {
                break;
            };
            line.push('\n');
            if client.send_input(&session_id, &line).is_err() {
                // Session likely exited; stop forwarding silently.
                break;
            }
        }
    });
}

fn write_cast_summary_event(
    session_id: &str,
    plan: &CastPlan,
    harness_id: &str,
    exit: &CastSessionExit,
) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    // Cast attach replays existing events through the same follower, which
    // would re-trigger the summary writer at the end. Skip the write when a
    // summary already exists so the ledger keeps exactly one `cast.summary`
    // per session.
    if store::event_kind_exists(&conn, session_id, "cast.summary")? {
        return Ok(());
    }
    let payload = serde_json::json!({
        "request": plan.raw_spell,
        "headline": plan.headline,
        "harness": harness_id,
        "status": exit.status,
        "exitCode": exit.exit_code,
    });
    store::insert_json_event(
        &conn,
        session_id,
        "cast.summary",
        &payload,
        &current_timestamp(),
    )
}

/// What brought the user into this attach. Affects the outcome card's
/// `launched` label so the user can tell the two flows apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttachOrigin {
    Attach,
    Summon,
}

impl AttachOrigin {
    fn verb_past(self) -> &'static str {
        match self {
            AttachOrigin::Attach => "Attached to",
            AttachOrigin::Summon => "Summoned and attached to",
        }
    }
}

/// Cast-native attach. Live sessions stream through the follower with the
/// same `afterSeq` cursor the launch path uses (so no duplicate output) and
/// accept follow-up input from stdin. Completed sessions replay their full
/// event log through the same observer so the user sees the same transcript
/// shape, then surface any existing `cast.summary` in the outcome card.
///
/// Falls back to the legacy `attach_session` loop when the daemon is not
/// reachable; the outcome card explains why the follower was skipped.
fn attach_via_cast(
    plan: &CastPlan,
    session_id: &str,
    request_text: &str,
    origin: AttachOrigin,
) -> Result<CastOutcome> {
    match daemon_runtime_state()? {
        DaemonRuntimeState::Running => attach_via_daemon(plan, session_id, request_text, origin),
        DaemonRuntimeState::NotReady(reason) => {
            attach_session(session_id)?;
            Ok(CastOutcome {
                request: request_text.to_string(),
                launched: Some(format!(
                    "{} session {session_id} (legacy attach fallback)",
                    origin.verb_past()
                )),
                session_id: Some(session_id.to_string()),
                next_step: Some(
                    "Start the daemon for streamed transcripts: `coven daemon start`.".to_string(),
                ),
                notes: vec![format!("Cast event follower skipped: {reason}.")],
            })
        }
    }
}

fn attach_via_daemon(
    plan: &CastPlan,
    session_id: &str,
    request_text: &str,
    origin: AttachOrigin,
) -> Result<CastOutcome> {
    let mut client = DaemonChatClient::default();
    let session = client
        .get_session(session_id)
        .with_context(|| format!("Cast could not look up session `{session_id}` via the daemon"))?;

    let is_live = session.status == RUNNING_SESSION_STATUS;

    println!();
    if is_live {
        println!(
            "Cast transcript — session {} ({}). Press Enter at any time to send input.",
            session.id, session.harness
        );
    } else {
        println!(
            "Cast transcript — session {} ({}) replay.",
            session.id, session.harness
        );
    }

    if is_live {
        // Mirror the launch path: forward stdin lines into the daemon for
        // follow-up input while the follower streams output. The thread is
        // detached and exits when stdin closes.
        maybe_spawn_cast_input_forwarder(coven_home_dir()?, session.id.clone());
    }

    let mut observer = TranscriptObserver::new(io::stdout());
    let exit = if is_live {
        let mut pacer = SleepPacer::new(Duration::from_millis(250));
        Some(follow_until_exit(
            &mut client,
            &session.id,
            &mut observer,
            &mut pacer,
        )?)
    } else {
        // For completed sessions, drain the historical event log once. The
        // follower observer renders the transcript exactly as it did on the
        // original run and reports the exit when it lands in the replay.
        replay_completed_session(&mut client, &session.id, &mut observer)?
    };

    if is_live {
        if let Some(exit) = exit.as_ref() {
            // Idempotent — skips when a `cast.summary` already exists.
            write_cast_summary_event(&session.id, plan, &session.harness, exit)?;
        }
    }

    let mut notes = plan_outcome_notes(plan);
    if let Some(exit) = exit.as_ref() {
        notes.push(format_exit_summary(exit));
    }
    // For completed/replayed sessions, surface the original Cast summary so
    // the user can see what the prior run was about. Live attaches just wrote
    // the summary themselves, so showing it again would only echo the exit
    // line we already printed. We also use the same history query to detect
    // a quest anchor (Phase 7 minimal re-attach aid — detect + inform).
    if !is_live {
        let history = client.list_events(ChatEventQuery {
            session_id: &session.id,
            after_seq: None,
            limit: None,
        })?;
        if let Some(note) = cast::find_cast_summary(&history).and_then(|s| format_summary_note(&s))
        {
            notes.push(note);
        }
        if let Some(note) = cast::find_cast_quest_info(&history)
            .and_then(|info| cast::format_quest_attach_note(&info))
        {
            notes.push(note);
        }
    }

    let launched = if is_live {
        format!(
            "{} live session {} via daemon",
            origin.verb_past(),
            session.id
        )
    } else {
        format!(
            "{} completed session {} (replayed via daemon)",
            origin.verb_past(),
            session.id
        )
    };

    let next_step = if is_live {
        Some(format!(
            "Run `coven attach {}` later to revisit; events are durable.",
            session.id
        ))
    } else {
        Some(format!(
            "Run `coven sessions` to manage this session, or `/sacrifice {}` to delete it.",
            session.id
        ))
    };

    Ok(CastOutcome {
        request: request_text.to_string(),
        launched: Some(launched),
        session_id: Some(session.id),
        next_step,
        notes,
    })
}

/// Drain the historical event log for a non-running session through the same
/// observer the live follower uses. Returns the decoded exit if the replay
/// contains an `exit` event so the outcome card can show the original status.
fn replay_completed_session(
    client: &mut dyn ChatClient,
    session_id: &str,
    observer: &mut dyn FollowerObserver,
) -> Result<Option<CastSessionExit>> {
    let records = client.list_events(ChatEventQuery {
        session_id,
        after_seq: None,
        limit: None,
    })?;

    let mut exit = None;
    for record in records {
        let decoded = cast::follow::decode_event(&record);
        match &decoded {
            cast::follow::CastFollowEvent::Output(chunk) => observer.on_output(chunk),
            cast::follow::CastFollowEvent::Exit { status, exit_code } => {
                observer.on_exit(status, *exit_code);
                exit = Some(CastSessionExit {
                    status: status.clone(),
                    exit_code: *exit_code,
                });
            }
            cast::follow::CastFollowEvent::Other { kind } => observer.on_other(kind),
        }
    }
    Ok(exit)
}

fn format_exit_summary(exit: &CastSessionExit) -> String {
    match exit.exit_code {
        Some(code) => format!(
            "Session finished: status `{}`, exit code {code}",
            exit.status
        ),
        None => format!("Session finished: status `{}`", exit.status),
    }
}

fn plan_outcome_notes(plan: &CastPlan) -> Vec<String> {
    let mut notes = Vec::new();
    if let SafetyDecision::Confirm { reason, suggestion } = &plan.decision {
        notes.push(format!("Risk: {reason}. {suggestion}"));
    }
    notes
}

fn harness_label(harness_id: &str) -> &'static str {
    match harness_id {
        "codex" => "Codex",
        "claude" => "Claude Code",
        _ => "Harness",
    }
}

fn print_plan_intro(plan: &CastPlan) {
    let frame = render_plan_intro(plan);
    if !frame.is_empty() {
        println!("{frame}");
    }
}

fn print_outcome(outcome: &CastOutcome) {
    let frame = render_outcome(outcome);
    if !frame.is_empty() {
        print!("\n{frame}");
    }
}

fn print_cast_non_interactive_frame() {
    let project_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| project::canonical_project_root(&cwd).ok());
    let default_harness_id = default_harness_id();
    let frame = render_cast_frame_for_terminal(project_root.as_deref(), default_harness_id);
    print!("{frame}");
}

/// Plain-text Cast frame for tests and pipe targets. Mirrors
/// `print_cast_non_interactive_frame` minus the theme escapes and stdout.
#[cfg(test)]
pub(crate) fn cast_non_interactive_frame_for_test(
    project_root: Option<&std::path::Path>,
    default_harness: Option<&str>,
) -> String {
    super::cast::render_cast_frame_plain(project_root, default_harness)
}

fn run_magical_tui_action(action: MagicalTuiAction) -> Result<()> {
    match action {
        MagicalTuiAction::StartHere => run_new_user_start_here(),
        MagicalTuiAction::Help => run_tui_help(),
        MagicalTuiAction::OpenTui => run(),
        MagicalTuiAction::Doctor => run_doctor(),
        MagicalTuiAction::DaemonStatus => run_daemon_command(DaemonCommand::Status),
        MagicalTuiAction::RunHarness => run_guided_harness_session(),
        MagicalTuiAction::PatchOpenClaw => {
            run_patch_openclaw(vec![], None, None, None, false, false, true)
        }
        MagicalTuiAction::Sessions => sessions::run_browser(false),
        MagicalTuiAction::AllSessions => sessions::run_browser(true),
        MagicalTuiAction::AttachSession
        | MagicalTuiAction::SummonSession
        | MagicalTuiAction::ArchiveSession
        | MagicalTuiAction::SacrificeSession => sessions::run_browser(true),
        MagicalTuiAction::Quit => {
            let primary = theme::fg(theme::PRIMARY);
            let reset = theme::reset();
            println!("{primary}The circle fades. Nothing changed.{reset}");
            Ok(())
        }
    }
}

#[cfg(test)]
pub(crate) fn parse_magical_tui_input(input: &str) -> Result<MagicalTuiRequest> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(MagicalTuiRequest::Action(MagicalTuiAction::OpenTui));
    }
    if !input.starts_with('/') {
        return Ok(MagicalTuiRequest::NaturalPrompt(input.to_string()));
    }

    let (command, rest) = split_command(input);
    match command {
        "/start" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::StartHere)),
        "/help" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::Help)),
        "/tui" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::OpenTui)),
        "/doctor" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::Doctor)),
        "/daemon" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::DaemonStatus)),
        "/patch" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::PatchOpenClaw)),
        "/sessions" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::Sessions)),
        "/all" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::AllSessions)),
        "/run" => parse_run_slash_command(rest),
        "/codex" => parse_harness_slash_command("codex", rest),
        "/claude" => parse_harness_slash_command("claude", rest),
        "/attach" => parse_session_slash_command(rest, MagicalTuiRequest::AttachSession),
        "/summon" => parse_session_slash_command(rest, MagicalTuiRequest::SummonSession),
        "/archive" => parse_session_slash_command(rest, MagicalTuiRequest::ArchiveSession),
        "/sacrifice" => parse_session_slash_command(rest, MagicalTuiRequest::SacrificeSession),
        "/quit" | "/exit" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::Quit)),
        _ => anyhow::bail!(
            "unknown Coven slash command `{command}`. Type `/help` to see available commands"
        ),
    }
}

#[cfg(test)]
fn split_command(input: &str) -> (&str, &str) {
    if let Some(index) = input.find(char::is_whitespace) {
        (&input[..index], input[index..].trim())
    } else {
        (input, "")
    }
}

#[cfg(test)]
fn parse_run_slash_command(rest: &str) -> Result<MagicalTuiRequest> {
    if rest.trim().is_empty() {
        return Ok(MagicalTuiRequest::Action(MagicalTuiAction::RunHarness));
    }
    let (first, remaining) = split_command(rest);
    if matches!(first, "codex" | "claude") {
        if remaining.is_empty() {
            anyhow::bail!("`/run {first}` needs a task, for example `/run {first} fix tests`");
        }
        return Ok(MagicalTuiRequest::HarnessPrompt {
            harness: first.to_string(),
            prompt: remaining.to_string(),
        });
    }
    Ok(MagicalTuiRequest::NaturalPrompt(rest.trim().to_string()))
}

#[cfg(test)]
fn parse_harness_slash_command(harness: &str, rest: &str) -> Result<MagicalTuiRequest> {
    let prompt = rest.trim();
    if prompt.is_empty() {
        anyhow::bail!("`/{harness}` needs a task, for example `/{harness} fix tests`");
    }
    Ok(MagicalTuiRequest::HarnessPrompt {
        harness: harness.to_string(),
        prompt: prompt.to_string(),
    })
}

#[cfg(test)]
mod quest_interaction_tests {
    use super::*;
    use std::cell::RefCell;

    fn scripted_reader(script: Vec<&'static str>) -> impl FnMut(&str) -> Result<String> {
        let lines = RefCell::new(script.into_iter());
        move |_prompt: &str| {
            Ok(lines
                .borrow_mut()
                .next()
                .map(str::to_string)
                .unwrap_or_default())
        }
    }

    #[test]
    fn run_phase_interaction_returns_approve_on_empty_input() {
        let mut quest = quest_from_goal("polish the README", Some(CastHarness::Codex));
        let mut reader = scripted_reader(vec![""]);
        let action = run_phase_interaction(&mut quest, 0, &mut reader).unwrap();
        assert_eq!(action, PhaseInteraction::Approve);
    }

    #[test]
    fn run_phase_interaction_loops_on_edit_until_terminal_action() {
        let mut quest = quest_from_goal("polish the README", Some(CastHarness::Codex));
        let original = quest.phases[0].sub_prompt.clone();
        // First line is an edit → second line skips. The function must
        // apply the edit, mark the phase as user-edited, then return Skip.
        let mut reader = scripted_reader(vec!["draft just the headlines", "/skip already covered"]);
        let action = run_phase_interaction(&mut quest, 0, &mut reader).unwrap();
        assert_eq!(
            action,
            PhaseInteraction::Skip {
                reason: "already covered".to_string(),
            }
        );
        assert!(quest.phases[0].edited_by_user);
        assert_eq!(quest.phases[0].sub_prompt, "draft just the headlines");
        assert_ne!(quest.phases[0].sub_prompt, original);
    }

    #[test]
    fn run_phase_interaction_returns_cancel_with_default_reason() {
        let mut quest = quest_from_goal("polish the README", Some(CastHarness::Codex));
        let mut reader = scripted_reader(vec!["/cancel"]);
        let action = run_phase_interaction(&mut quest, 0, &mut reader).unwrap();
        match action {
            PhaseInteraction::Cancel { reason } => {
                assert!(!reason.is_empty(), "default reason should be non-empty");
            }
            other => panic!("expected Cancel, got {other:?}"),
        }
    }
}

#[cfg(test)]
fn parse_session_slash_command(
    rest: &str,
    build: fn(String) -> MagicalTuiRequest,
) -> Result<MagicalTuiRequest> {
    let session_id = rest.trim();
    if session_id.is_empty() {
        anyhow::bail!("this slash command needs a session id");
    }
    Ok(build(session_id.to_string()))
}

fn run_tui_help() -> Result<()> {
    let primary_strong = theme::fg(theme::PRIMARY_STRONG);
    let reset = theme::reset();
    println!("{primary_strong}Coven TUI{reset}");
    println!("Type a plain-language task and press Enter to launch your default harness.");
    println!("Use slash commands when you want a specific route. Examples:");
    println!("  fix the failing tests");
    println!("  /run codex explain this repo");
    println!("  /claude review the latest diff");
    println!("  /sessions");
    println!("  /attach <session-id>");
    println!("  /doctor");
    Ok(())
}

fn run_new_user_start_here() -> Result<()> {
    let primary_strong = theme::fg(theme::PRIMARY_STRONG);
    let reset = theme::reset();
    println!("{primary_strong}Coven quick start{reset}");
    println!("Coven is a safe room for coding agents. It keeps each run tied to this project,");
    println!("records the session, and lets other tools list or attach to the work later.\n");
    println!("Recommended first run:");
    println!("  1. coven doctor");
    println!("  2. coven run codex \"explain this repo in 5 bullets\"");
    println!("  3. coven sessions");
    println!("\nSetup check:\n");
    run_doctor()
}

fn run_guided_harness_session() -> Result<()> {
    let primary_strong = theme::fg(theme::PRIMARY_STRONG);
    let reset = theme::reset();
    println!("{primary_strong}Run an agent in this project{reset}");
    println!("Coven will create a session record, validate the project root, then attach to the harness.\n");
    let default_harness = default_harness_id().unwrap_or("codex");
    let harness_prompt = format!("Harness [default: {default_harness}; options: codex, claude]: ");
    let harness =
        prompt_for_optional_line(&harness_prompt)?.unwrap_or_else(|| default_harness.to_string());
    let prompt = prompt_for_required_line("Task for the agent: ")?;
    let title = prompt_for_optional_line("Optional session title [enter to skip]: ")?;
    run_session(&harness, &[prompt], None, title.as_deref(), false)
}

const LAUNCHER_VISIBLE_COMMANDS: usize = 6;
const LAUNCHER_FIELD_LABEL_WIDTH: usize = 14;
const LAUNCHER_PROMPT_PLACEHOLDER: &str = "type a task or /run codex";
const LAUNCHER_FOOTER_HINT: &str = "enter run · ↑↓ select · esc quit · ctrl+u clear";

/// Snapshot of the local context shown in the right-hand lane of the
/// launcher (`project`, `harness`, `daemon`). Each field is best-effort:
/// when the environment cannot be read we fall back to quiet placeholders
/// rather than failing to render the frame.
pub(crate) struct LauncherSnapshot {
    pub project: String,
    pub harness: String,
    pub daemon: String,
}

impl LauncherSnapshot {
    fn placeholder() -> Self {
        Self {
            project: "(unset)".to_string(),
            harness: "(unset)".to_string(),
            daemon: "unknown".to_string(),
        }
    }
}

fn resolve_launcher_snapshot() -> LauncherSnapshot {
    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| project::canonical_project_root(&cwd).ok())
        .and_then(|root| root.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "(no project)".to_string());

    let harness = default_harness_id()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "(none)".to_string());

    let daemon = coven_home_dir()
        .ok()
        .and_then(|home| daemon::background_server_status(&home).ok().flatten())
        .map(|state| match state {
            daemon::DaemonStatusState::Running(_) => "running",
            daemon::DaemonStatusState::Stale(_) => "stale",
        })
        .unwrap_or("stopped")
        .to_string();

    LauncherSnapshot {
        project,
        harness,
        daemon,
    }
}

fn render_magical_tui_frame(selection: usize, input: &str) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        input,
        &resolve_launcher_snapshot(),
        theme::mode(),
        magical_tui_inner_width(),
    )
}

pub(crate) fn render_magical_tui_frame_for_raw_terminal(selection: usize, input: &str) -> String {
    render_magical_tui_frame(selection, input).replace('\n', "\r\n")
}

#[allow(dead_code)]
pub(crate) fn render_magical_tui_frame_plain(selection: usize) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        "",
        &LauncherSnapshot::placeholder(),
        theme::TerminalMode::NoColor,
        MAGICAL_TUI_DEFAULT_INNER_WIDTH,
    )
}

#[cfg(test)]
pub(crate) fn render_magical_tui_frame_plain_with_width(
    selection: usize,
    inner_width: usize,
) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        "",
        &LauncherSnapshot::placeholder(),
        theme::TerminalMode::NoColor,
        inner_width,
    )
}

#[cfg(test)]
pub(crate) fn render_magical_tui_frame_plain_with_input(
    selection: usize,
    input: &str,
    inner_width: usize,
) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        input,
        &LauncherSnapshot::placeholder(),
        theme::TerminalMode::NoColor,
        inner_width,
    )
}

fn render_magical_tui_frame_with_mode_and_width(
    selection: usize,
    input: &str,
    snapshot: &LauncherSnapshot,
    mode: theme::TerminalMode,
    inner_width: usize,
) -> String {
    let inner_width = normalized_magical_tui_inner_width(inner_width);
    let palette = theme::palette_for(mode);
    let total_items = magical_tui_items().len();
    let visible = LAUNCHER_VISIBLE_COMMANDS.min(total_items);
    let window = launcher_command_window(selection, total_items, visible);
    let (left_width, right_width) = body_lane_widths(inner_width);

    let mut frame = String::new();

    // 1. Identity
    push_line(
        &mut frame,
        "Cast",
        palette.primary_strong,
        palette.reset,
        inner_width,
    );
    push_line(&mut frame, "", palette.text, palette.reset, inner_width);

    // 2. Prompt area — quiet rule above, emphasized rule below. The bottom
    //    rule is the focus underline: the launcher prompt is always the
    //    interactive element, so it gets `BORDER_STRONG`; the top rule is a
    //    plain panel separator (`BORDER_SUBTLE`).
    let rule: String = "─".repeat(inner_width);
    let border_subtle = theme::Fg::with_mode(theme::BORDER_SUBTLE, mode);
    let border_strong = theme::Fg::with_mode(theme::BORDER_STRONG, mode);
    push_line(&mut frame, &rule, border_subtle, palette.reset, inner_width);
    let (prompt_text, prompt_color) = if input.is_empty() {
        (format!("> {LAUNCHER_PROMPT_PLACEHOLDER}"), palette.dim)
    } else {
        (format!("> {input}"), palette.text)
    };
    push_line(
        &mut frame,
        &prompt_text,
        prompt_color,
        palette.reset,
        inner_width,
    );
    push_line(&mut frame, &rule, border_strong, palette.reset, inner_width);
    push_line(&mut frame, "", palette.text, palette.reset, inner_width);

    // 3. Two-lane body: Commands rail (windowed) + Snapshot rows.
    push_two_lane(
        &mut frame,
        ("Commands", palette.primary_strong),
        ("Snapshot", palette.primary_strong),
        palette.reset,
        (left_width, right_width),
    );

    let snapshot_rows = [
        ("project", snapshot.project.as_str()),
        ("harness", snapshot.harness.as_str()),
        ("daemon", snapshot.daemon.as_str()),
    ];
    let body_row_count = visible.max(snapshot_rows.len());
    for row in 0..body_row_count {
        let (left_text, left_color) = match window.get(row) {
            Some(&idx) => {
                let item = &magical_tui_items()[idx];
                let row_text = magical_tui_command_row(idx == selection, item);
                let color = if idx == selection {
                    palette.primary_strong
                } else {
                    palette.text
                };
                (row_text, color)
            }
            None => (String::new(), palette.text),
        };
        let right_text = snapshot_rows
            .get(row)
            .map(|(label, value)| snapshot_row(label, value, right_width))
            .unwrap_or_default();
        push_two_lane(
            &mut frame,
            (&left_text, left_color),
            (&right_text, palette.text),
            palette.reset,
            (left_width, right_width),
        );
    }

    // Scroll hint when the rail can't display every item.
    if visible < total_items {
        let hint = format!("{} of {}", selection.min(total_items - 1) + 1, total_items);
        push_line(&mut frame, &hint, palette.dim, palette.reset, inner_width);
    }
    push_line(&mut frame, "", palette.text, palette.reset, inner_width);

    // 4. Action preview rows for the current selection.
    let selected = &magical_tui_items()[selection.min(total_items - 1)];
    push_field_row(&mut frame, "spell", selected.slash, &palette, inner_width);
    push_field_row(
        &mut frame,
        "detail",
        selected.description,
        &palette,
        inner_width,
    );
    push_line(&mut frame, "", palette.text, palette.reset, inner_width);

    // 5. Footer hint — one dim line, never two.
    push_line(
        &mut frame,
        LAUNCHER_FOOTER_HINT,
        palette.dim,
        palette.reset,
        inner_width,
    );

    frame
}

fn launcher_command_window(selection: usize, total: usize, visible: usize) -> Vec<usize> {
    if total == 0 || visible == 0 {
        return Vec::new();
    }
    let last = total - 1;
    let sel = selection.min(last);
    let start = if sel < visible { 0 } else { sel + 1 - visible };
    let end = (start + visible).min(total);
    (start..end).collect()
}

fn body_lane_widths(inner_width: usize) -> (usize, usize) {
    // Two columns separated by a 2-space gap; the rail favours the left
    // lane by one character on odd widths because slash + label text is
    // typically longer than the snapshot values.
    let usable = inner_width.saturating_sub(2);
    let left = usable.div_ceil(2);
    let right = usable - left;
    (left, right)
}

fn push_line(
    frame: &mut String,
    content: &str,
    color: impl std::fmt::Display,
    reset: impl std::fmt::Display,
    inner_width: usize,
) {
    let fitted = fit_chars(content, inner_width);
    frame.push_str(&format!("{color}{fitted}{reset}\n"));
}

fn push_two_lane(
    frame: &mut String,
    left: (&str, impl std::fmt::Display),
    right: (&str, impl std::fmt::Display),
    reset: impl std::fmt::Display,
    widths: (usize, usize),
) {
    let (left_text, left_color) = left;
    let (right_text, right_color) = right;
    let (left_width, right_width) = widths;
    let left_fitted = fit_chars(left_text, left_width);
    let right_fitted = fit_chars(right_text, right_width);
    let pad = left_width.saturating_sub(left_fitted.chars().count());
    let padding = " ".repeat(pad);
    frame.push_str(&format!(
        "{left_color}{left_fitted}{reset}{padding}  {right_color}{right_fitted}{reset}\n",
    ));
}

fn snapshot_row(label: &str, value: &str, right_width: usize) -> String {
    if right_width == 0 {
        return String::new();
    }
    let column = LAUNCHER_FIELD_LABEL_WIDTH.min(right_width);
    if column + 2 >= right_width {
        return fit_chars(label, right_width);
    }
    let fitted_label = fit_chars(label, column);
    let pad = column.saturating_sub(fitted_label.chars().count());
    let value_width = right_width - column - 2;
    let fitted_value = fit_chars(value, value_width);
    format!(
        "{fitted_label}{padding}  {fitted_value}",
        padding = " ".repeat(pad)
    )
}

fn push_field_row(
    frame: &mut String,
    label: &str,
    value: &str,
    palette: &theme::Palette,
    inner_width: usize,
) {
    let column = LAUNCHER_FIELD_LABEL_WIDTH.min(inner_width.saturating_sub(2).max(1));
    let fitted_label = fit_chars(label, column);
    let pad = column.saturating_sub(fitted_label.chars().count());
    let value_width = inner_width.saturating_sub(column + 2);
    let fitted_value = fit_chars(value, value_width);
    let padding = " ".repeat(pad);
    frame.push_str(&format!(
        "{field_label}{fitted_label}{reset}{padding}  {text}{fitted_value}{reset}\n",
        field_label = palette.field_label,
        text = palette.text,
        reset = palette.reset,
    ));
}

fn magical_tui_command_row(selected: bool, item: &MagicalTuiItem) -> String {
    let pointer = if selected { "›" } else { " " };
    format!("{pointer} {:<10} {}", item.slash, item.label)
}

fn magical_tui_inner_width() -> usize {
    crossterm::terminal::size()
        .map(|(columns, _)| magical_tui_inner_width_for_columns(columns as usize))
        .unwrap_or(MAGICAL_TUI_DEFAULT_INNER_WIDTH)
}

pub(crate) fn magical_tui_inner_width_for_columns(columns: usize) -> usize {
    let available = columns.saturating_sub(2);
    if available < MAGICAL_TUI_MIN_INNER_WIDTH {
        available.max(18)
    } else {
        available.min(MAGICAL_TUI_MAX_INNER_WIDTH)
    }
}

fn normalized_magical_tui_inner_width(inner_width: usize) -> usize {
    inner_width.clamp(18, MAGICAL_TUI_MAX_INNER_WIDTH)
}

fn fit_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_string();
    }
    if limit == 0 {
        return String::new();
    }
    if limit == 1 {
        return "…".to_string();
    }

    let mut fitted = value.chars().take(limit - 1).collect::<String>();
    fitted.push('…');
    fitted
}

pub(crate) fn move_magical_tui_selection(current: usize, direction: MagicalTuiMove) -> usize {
    let item_count = magical_tui_items().len();
    match direction {
        MagicalTuiMove::Up => current.checked_sub(1).unwrap_or(item_count - 1),
        MagicalTuiMove::Down => (current + 1) % item_count,
    }
}

#[cfg(test)]
pub(crate) fn render_frame_plain_for_test(selection: usize) -> String {
    render_magical_tui_frame_plain(selection)
}

#[cfg(test)]
mod attach_tests {
    use std::cell::RefCell;
    use std::io::Cursor;

    use super::*;
    use crate::tui::chat::client::LaunchRequest;

    /// Stub client that returns a canned event log on every `list_events` call
    /// and records which session id was queried. Mirrors the stub in
    /// `cast::follow` tests but kept local to shell.rs so the wiring (not the
    /// already-tested decode pipeline) is what's under test here.
    struct ReplayClient {
        events: Vec<store::EventRecord>,
        queries: RefCell<Vec<String>>,
    }

    impl ReplayClient {
        fn new(events: Vec<store::EventRecord>) -> Self {
            Self {
                events,
                queries: RefCell::new(Vec::new()),
            }
        }
    }

    impl ChatClient for ReplayClient {
        fn launch_session(&mut self, _request: LaunchRequest) -> Result<store::SessionRecord> {
            unimplemented!("not exercised by replay tests")
        }

        fn get_session(&mut self, _session_id: &str) -> Result<store::SessionRecord> {
            unimplemented!("not exercised by replay tests")
        }

        fn list_sessions(&mut self) -> Result<Vec<store::SessionRecord>> {
            unimplemented!("not exercised by replay tests")
        }

        fn list_events(&mut self, query: ChatEventQuery<'_>) -> Result<Vec<store::EventRecord>> {
            self.queries.borrow_mut().push(query.session_id.to_string());
            Ok(self.events.clone())
        }

        fn send_input(&mut self, _session_id: &str, _data: &str) -> Result<()> {
            unimplemented!("not exercised by replay tests")
        }

        fn kill_session(&mut self, _session_id: &str) -> Result<()> {
            unimplemented!("not exercised by replay tests")
        }
    }

    fn output_event(seq: i64, data: &str) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "session-1".to_string(),
            kind: "output".to_string(),
            payload_json: serde_json::json!({ "data": data }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        }
    }

    fn exit_event(seq: i64, status: &str, exit_code: Option<i32>) -> store::EventRecord {
        let payload = match exit_code {
            Some(code) => serde_json::json!({ "status": status, "exitCode": code }),
            None => serde_json::json!({ "status": status }),
        };
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "session-1".to_string(),
            kind: "exit".to_string(),
            payload_json: payload.to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn attach_origin_verb_labels_distinguish_attach_from_summon() {
        assert_eq!(AttachOrigin::Attach.verb_past(), "Attached to");
        assert_eq!(AttachOrigin::Summon.verb_past(), "Summoned and attached to");
    }

    #[test]
    fn replay_completed_session_streams_full_transcript_into_observer() {
        let mut client = ReplayClient::new(vec![
            output_event(1, "hello\n"),
            output_event(2, "world\n"),
            exit_event(3, "completed", Some(0)),
        ]);
        let mut buffer: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut observer = TranscriptObserver::new(&mut buffer);

        let exit =
            replay_completed_session(&mut client, "session-1", &mut observer).expect("replay");

        let rendered = String::from_utf8(buffer.into_inner()).unwrap();
        assert!(
            rendered.contains("hello"),
            "transcript missing first chunk: {rendered:?}"
        );
        assert!(
            rendered.contains("world"),
            "transcript missing second chunk"
        );
        assert!(
            rendered.contains("[Cast: session completed (exit code 0)]"),
            "transcript should include Cast exit banner: {rendered:?}"
        );

        let exit = exit.expect("replay should surface the recorded exit");
        assert_eq!(exit.status, "completed");
        assert_eq!(exit.exit_code, Some(0));

        let queries = client.queries.borrow();
        assert_eq!(queries.len(), 1, "one full-history fetch is enough");
        assert_eq!(queries[0], "session-1");
    }

    #[test]
    fn replay_completed_session_returns_none_when_no_exit_event_in_log() {
        let mut client = ReplayClient::new(vec![
            output_event(1, "still going\n"),
            output_event(2, "no exit yet\n"),
        ]);
        let mut buffer: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut observer = TranscriptObserver::new(&mut buffer);

        let exit =
            replay_completed_session(&mut client, "session-1", &mut observer).expect("replay");

        assert!(
            exit.is_none(),
            "replay must not invent an exit when the log has none"
        );
        let rendered = String::from_utf8(buffer.into_inner()).unwrap();
        assert!(rendered.contains("still going"));
        assert!(rendered.contains("no exit yet"));
        assert!(
            !rendered.contains("[Cast:"),
            "no Cast exit banner without an exit event: {rendered:?}"
        );
    }
}
