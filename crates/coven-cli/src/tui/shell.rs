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
    self, evaluate_gate, follow_until_exit, format_summary_note, render_cast_frame_for_terminal,
    render_outcome, render_plan_intro, CastIntent, CastOutcome, CastPlan, CastSessionExit,
    FollowerObserver, FollowerPacer, GateOutcome, SafetyDecision,
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
    let (exit, replay_summary_note) = if is_live {
        let mut pacer = SleepPacer::new(Duration::from_millis(250));
        (
            Some(follow_until_exit(
                &mut client,
                &session.id,
                &mut observer,
                &mut pacer,
            )?),
            None,
        )
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
    // line we already printed.
    if !is_live {
        if let Some(note) = replay_summary_note {
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
/// contains an `exit` event so the outcome card can show the original status,
/// plus the rendered `cast.summary` note (if present) from the same event set.
fn replay_completed_session(
    client: &mut dyn ChatClient,
    session_id: &str,
    observer: &mut dyn FollowerObserver,
) -> Result<(Option<CastSessionExit>, Option<String>)> {
    let records = client.list_events(ChatEventQuery {
        session_id,
        after_seq: None,
        limit: None,
    })?;
    let summary_note = cast::find_cast_summary(&records).and_then(|s| format_summary_note(&s));

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
    Ok((exit, summary_note))
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
    println!("\nTip: run `coven` in a real terminal to open the Cast launcher and type a spell.");
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

fn render_magical_tui_frame(selection: usize, input: &str) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        input,
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
        theme::TerminalMode::NoColor,
        inner_width,
    )
}

fn render_magical_tui_frame_with_mode_and_width(
    selection: usize,
    input: &str,
    mode: theme::TerminalMode,
    inner_width: usize,
) -> String {
    let inner_width = normalized_magical_tui_inner_width(inner_width);
    let primary = theme::Fg::with_mode(theme::PRIMARY, mode);
    let primary_strong = theme::Fg::with_mode(theme::PRIMARY_STRONG, mode);
    let field_label = theme::Fg::with_mode(theme::FIELD_LABEL, mode);
    let user_label = theme::Fg::with_mode(theme::USER_LABEL, mode);
    let dim = theme::Fg::with_mode(theme::DIM, mode);
    let reset = theme::Reset::with_mode(mode);
    let mut frame = String::new();
    frame.push_str(&magical_tui_line(
        "CovenCLI",
        primary_strong,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        "Welcome back to the Coven.",
        field_label,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        "OpenCoven terminal home for local agent work.",
        user_label,
        reset,
        inner_width,
    ));
    frame.push('\n');
    for line in magical_tui_graph_lines() {
        frame.push_str(&magical_tui_line(line, primary, reset, inner_width));
    }
    frame.push('\n');
    frame.push_str(&magical_tui_line(
        "Status",
        primary_strong,
        reset,
        inner_width,
    ));
    for line in magical_tui_status_lines() {
        frame.push_str(&magical_tui_line(line, field_label, reset, inner_width));
    }
    frame.push('\n');
    frame.push_str(&magical_tui_line(
        "Task inbox",
        primary_strong,
        reset,
        inner_width,
    ));
    for line in magical_tui_task_inbox_lines() {
        frame.push_str(&magical_tui_line(line, primary, reset, inner_width));
    }
    frame.push('\n');
    for line in magical_tui_input_box_lines(input, inner_width) {
        frame.push_str(&magical_tui_line(&line, user_label, reset, inner_width));
    }
    frame.push('\n');

    frame.push_str(&magical_tui_line(
        "Slash commands",
        primary_strong,
        reset,
        inner_width,
    ));
    for (index, item) in magical_tui_items().iter().enumerate() {
        let pointer = if index == selection { ">" } else { " " };
        let content = magical_tui_command_row(pointer, item, inner_width);
        let color = if index == selection {
            primary_strong
        } else {
            primary
        };
        frame.push_str(&magical_tui_line(&content, color, reset, inner_width));
    }

    let selected = magical_tui_items()[selection.min(magical_tui_items().len() - 1)];
    frame.push('\n');
    frame.push_str(&magical_tui_line(
        "Selected command",
        primary_strong,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        selected.description,
        user_label,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        &format!("{} => {}", selected.slash, selected.command),
        primary_strong,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        "Store: ~/.coven",
        dim,
        reset,
        inner_width,
    ));
    frame
}

fn magical_tui_graph_lines() -> &'static [&'static str] {
    &[
        "+-------------------------- Workspace map -----------------------------+",
        "| workspace: current repo            branch: local checkout            |",
        "| harness shelf: Codex | Claude Code | local adapters                  |",
        "|                                                                      |",
        "|       [nova] ------ [coven] ------ [cody]                            |",
        r"|          |            /   \           |                              |",
        r"|          |           /     \          |                              |",
        "| [memory] -- [coven] -- [sessions] -- [review]                        |",
        r"|          |                              \                            |",
        "|     [gateway]                     local daemon                       |",
        "|                                                                      |",
        "| prompt floor: ask | slash | attach | summon | archive | sacrifice    |",
        "+----------------------------------------------------------------------+",
    ]
}

fn magical_tui_status_lines() -> &'static [&'static str] {
    &[
        "System snapshot   local-first session ledger | ~/.coven",
        "Model lane        Codex ready | Claude Code ready | PTY guarded",
        "Context           repo, docs, memory, sessions, and slash palette",
        "Approvals         asks before secrets, deletes, pushes, or public moves",
        "Release notes     CovenCLI now opens as a rich terminal home",
        "Tips              type a task, /run <harness>, or choose below",
    ]
}

fn magical_tui_task_inbox_lines() -> &'static [&'static str] {
    &[
        "[ ] inspect repo      [ ] launch harness      [ ] attach session",
        "[ ] review diff       [ ] export trace        [ ] archive work",
        "Claude Code style: welcome, status, context, prompt, command rail",
    ]
}

fn magical_tui_prompt_row(input: &str, inner_width: usize) -> String {
    let value = if input.is_empty() {
        "fix the failing tests  |  /run codex plan the refactor"
    } else {
        input
    };
    fit_chars(&format!("> {value}"), inner_width)
}

fn magical_tui_input_box_lines(input: &str, inner_width: usize) -> Vec<String> {
    let width = normalized_magical_tui_inner_width(inner_width);
    let content_width = width.saturating_sub(4).max(1);
    let prompt = magical_tui_prompt_row(input, content_width);
    let hint = fit_chars(
        "Enter sends. Empty Enter runs selected slash. Ctrl+U clears. Esc quits.",
        content_width,
    );
    vec![
        magical_tui_input_box_top(width),
        magical_tui_input_box_row(&prompt, width),
        magical_tui_input_box_row(&hint, width),
        magical_tui_input_box_bottom(width),
    ]
}

fn magical_tui_input_box_top(width: usize) -> String {
    let label = "+-- Ask anything ";
    if width <= 2 {
        return fit_chars(label, width);
    }
    if width <= label.chars().count() + 1 {
        return fit_chars(label, width);
    }
    let fill = width - label.chars().count() - 1;
    format!("{label}{}+", "-".repeat(fill))
}

fn magical_tui_input_box_bottom(width: usize) -> String {
    if width <= 2 {
        return "-".repeat(width);
    }
    format!("+{}+", "-".repeat(width - 2))
}

fn magical_tui_input_box_row(content: &str, width: usize) -> String {
    if width <= 2 {
        return fit_chars(content, width);
    }
    let content_width = width.saturating_sub(4).max(1);
    let fitted = fit_chars(content, content_width);
    let padding = content_width.saturating_sub(fitted.chars().count());
    format!("| {fitted}{} |", " ".repeat(padding))
}

fn magical_tui_line(
    content: &str,
    text_color: impl std::fmt::Display,
    reset: impl std::fmt::Display,
    inner_width: usize,
) -> String {
    format!("{text_color}{}{reset}\n", fit_chars(content, inner_width))
}

fn magical_tui_command_row(pointer: &str, item: &MagicalTuiItem, inner_width: usize) -> String {
    let row = format!("{pointer} {:<10} {}", item.slash, item.label);
    fit_chars(&row, inner_width)
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

    fn cast_summary_event(seq: i64, request: &str) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "session-1".to_string(),
            kind: "cast.summary".to_string(),
            payload_json: serde_json::json!({
                "request": request,
                "status": "completed",
                "exitCode": 0,
                "harness": "codex"
            })
            .to_string(),
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

        let (exit, summary_note) =
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
        assert!(
            summary_note.is_none(),
            "no summary note without cast.summary"
        );

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

        let (exit, summary_note) =
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
        assert!(
            summary_note.is_none(),
            "no summary note without cast.summary"
        );
    }

    #[test]
    fn replay_completed_session_returns_cast_summary_note_from_same_history_fetch() {
        let mut client = ReplayClient::new(vec![
            output_event(1, "hello\n"),
            cast_summary_event(2, "fix tests"),
            exit_event(3, "completed", Some(0)),
        ]);
        let mut buffer: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut observer = TranscriptObserver::new(&mut buffer);

        let (exit, summary_note) =
            replay_completed_session(&mut client, "session-1", &mut observer).expect("replay");

        assert_eq!(exit.and_then(|v| v.exit_code), Some(0));
        let note = summary_note.expect("summary note should be rendered");
        assert!(note.contains("Prior Cast summary"));
        assert!(note.contains("request `fix tests`"));

        let queries = client.queries.borrow();
        assert_eq!(queries.len(), 1, "summary note should reuse replay history");
    }
}
