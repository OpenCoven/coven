//! Cast rendering.
//!
//! Phase 1 keeps Cast's voice plain-text and color-aware so the renderer
//! works in interactive terminals, non-interactive pipes, and tests. The
//! goal here is not a beautiful TUI — it is a Cast frame the user can read
//! before and after every spell.

use std::path::Path;

use crate::theme::{self, fit_chars, palette_for, Fg, Palette, TerminalMode};

use super::outcome::CastOutcome;
use super::plan::{CastHarnessSource, CastPlan, CastStepKind};
use super::quest::{Quest, QuestPhase, QuestPhaseStatus};
use super::safety::{CastRisk, SafetyDecision};

const CAST_INTRO_INNER_WIDTH: usize = 76;

/// Width of the field-label column shared by every Cast card. Matches the
/// 14-char rule from `docs/design/cast-tui-contract.md` so the eye locks
/// onto the value column across plan, outcome, and launcher frames.
const LABEL_COLUMN_WIDTH: usize = 14;

/// Fixed-width risk chip. The text-only form is what gets asserted in tests;
/// the colored form is wrapped by `risk_chip_fg`.
const CHIP_SAFE: &str = "[  SAFE  ]";
const CHIP_CONFIRM: &str = "[CONFIRM ]";
const CHIP_REJECT: &str = "[ REJECT ]";

/// One-line subtitle under the `Cast` identity row in the non-interactive
/// frame. Stays brand-aligned without second-person greeting or the
/// "is ready" lampshading the design contract calls out.
pub(crate) fn cast_salute() -> &'static str {
    "Coven familiar · type a spell or pick a slash"
}

/// A short Cast frame for non-interactive mode: who Cast is, what spells look
/// like, and where work goes when it lands. Designed for piped stdout, CI
/// snapshots, and `coven` from a non-tty wrapper. Today only consumed by
/// tests; future phases (announcement banners, plain `coven` snapshots) will
/// wire it into more callsites.
#[allow(dead_code)]
pub(crate) fn render_cast_frame_plain(
    project_root: Option<&Path>,
    default_harness: Option<&str>,
) -> String {
    render_cast_frame_with_mode(project_root, default_harness, TerminalMode::NoColor)
}

pub(crate) fn render_cast_frame_for_terminal(
    project_root: Option<&Path>,
    default_harness: Option<&str>,
) -> String {
    render_cast_frame_with_mode(project_root, default_harness, theme::mode())
}

fn render_cast_frame_with_mode(
    project_root: Option<&Path>,
    default_harness: Option<&str>,
    mode: TerminalMode,
) -> String {
    let p = palette_for(mode);
    let inner_width = CAST_INTRO_INNER_WIDTH;
    let mut frame = String::new();

    // Identity + subtitle (the subtitle is the only place "Coven familiar"
    // appears now — no second-person greeting, no "is ready" lampshading).
    push_section_header(&mut frame, &p, "Cast");
    frame.push_str(&format!(
        "{}{}{}\n",
        p.field_label,
        fit_chars(cast_salute(), inner_width),
        p.reset,
    ));
    frame.push('\n');

    push_section_header(&mut frame, &p, "Context");
    let project = project_root
        .map(|root| root.display().to_string())
        .unwrap_or_else(|| "not inside a project root — run from a repo".to_string());
    push_label_row(&mut frame, &p, "project", &project);
    let harness = default_harness.unwrap_or("none ready — run `coven doctor`");
    push_label_row(&mut frame, &p, "harness", harness);
    frame.push('\n');

    push_section_header(&mut frame, &p, "Example spells");
    for spell in cast_example_spells() {
        frame.push_str(&format!("  {}{}{}\n", p.text, spell, p.reset));
    }
    frame.push('\n');

    push_section_header(&mut frame, &p, "Slash spells");
    for spell in cast_example_slashes() {
        frame.push_str(&format!("  {}{}{}\n", p.user_label, spell, p.reset));
    }
    frame.push('\n');

    push_footer_hint(
        &mut frame,
        &p,
        "run `coven` in a terminal to open the launcher · empty input runs a slash",
    );
    frame
}

fn cast_example_spells() -> &'static [&'static str] {
    &[
        "fix the failing tests",
        "explain this repo in 5 bullets",
        "run claude polish the README",
        "use codex draft a release note",
        "review this branch",
        "open the last Claude session",
        "sessions",
        "doctor",
    ]
}

fn cast_example_slashes() -> &'static [&'static str] {
    &[
        "/run codex fix the failing tests",
        "/claude review the latest diff",
        "/sessions     /all     /attach <id>     /summon <id>",
        "/archive <id>     /sacrifice <id>",
        "/doctor     /daemon     /patch     /help     /quit",
    ]
}

/// Cast's pre-launch card: shown before any session is created so the user
/// can see what Cast resolved from the spell.
pub(crate) fn render_plan_intro(plan: &CastPlan) -> String {
    render_plan_intro_with_mode(plan, theme::mode())
}

#[allow(dead_code)]
pub(crate) fn render_plan_intro_plain(plan: &CastPlan) -> String {
    render_plan_intro_with_mode(plan, TerminalMode::NoColor)
}

fn render_plan_intro_with_mode(plan: &CastPlan, mode: TerminalMode) -> String {
    let p = palette_for(mode);
    let mut frame = String::new();

    push_section_header(&mut frame, &p, "Cast plan");

    push_label_row(&mut frame, &p, "spell", &plan_spell_value(plan));

    if let Some(plan_harness) = plan.harness {
        let source = match plan_harness.source {
            CastHarnessSource::UserChose => "user-chosen",
            CastHarnessSource::SafeDefault => "Cast default",
        };
        let value = format!("{} · {}", plan_harness.harness.label(), source);
        push_label_row(&mut frame, &p, "harness", &value);
    } else if let Some(session_id) = &plan.session_id {
        push_label_row(&mut frame, &p, "session", session_id);
    } else {
        // System actions (sessions, doctor, daemon, help, start, tui, patch,
        // quit) have no harness or session id — surface what Cast understood
        // so the card still answers "what did you pick?".
        push_label_row(&mut frame, &p, "intent", &plan.headline);
    }

    push_risk_row(&mut frame, &p, mode, plan.risk());
    if let SafetyDecision::Confirm { reason, .. } = &plan.decision {
        push_continuation_row(&mut frame, &p, reason);
    }
    if let SafetyDecision::Reject { reason, .. } = &plan.decision {
        push_continuation_row(&mut frame, &p, reason);
    }

    if !plan.steps.is_empty() {
        frame.push('\n');
        push_section_header(&mut frame, &p, "Steps");
        for (idx, step) in plan.steps.iter().take(4).enumerate() {
            push_step_row(&mut frame, &p, idx + 1, step.kind, &step.note);
        }
    }

    frame.push('\n');
    push_footer_hint(&mut frame, &p, plan_footer_hint(plan));

    frame
}

/// Cast's post-run outcome card: shown after the dispatched action finishes
/// so the user can see what was launched, where it lives, and what to do
/// next.
pub(crate) fn render_outcome(outcome: &CastOutcome) -> String {
    render_outcome_with_mode(outcome, theme::mode())
}

#[allow(dead_code)]
pub(crate) fn render_outcome_plain(outcome: &CastOutcome) -> String {
    render_outcome_with_mode(outcome, TerminalMode::NoColor)
}

fn render_outcome_with_mode(outcome: &CastOutcome, mode: TerminalMode) -> String {
    let p = palette_for(mode);
    let mut frame = String::new();

    push_section_header(&mut frame, &p, "Cast outcome");
    push_label_row(&mut frame, &p, "spell", &outcome.request);
    if let Some(launched) = &outcome.launched {
        push_label_row(&mut frame, &p, "launched", launched);
    }
    if let Some(session_id) = &outcome.session_id {
        push_label_row(&mut frame, &p, "session", session_id);
    }

    if !outcome.notes.is_empty() {
        frame.push('\n');
        push_section_header(&mut frame, &p, "Notes");
        for note in outcome.notes.iter().take(3) {
            push_note_row(&mut frame, &p, note);
        }
    }

    if let Some(next) = &outcome.next_step {
        frame.push('\n');
        push_label_row(&mut frame, &p, "next", next);
    }

    frame
}

/// Cast's quest handoff card: shown between phases of a sequential quest
/// so the user can read what the prior phase produced and exactly what the
/// next phase's sub-prompt will be before approving the handoff. The card
/// is a *visible delegation announcement* — it never executes anything; it
/// just makes Cast's deterministic composer inspectable.
#[allow(dead_code)]
pub(crate) fn render_quest_handoff(quest: &Quest, next_index: usize) -> String {
    render_quest_handoff_with_mode(quest, next_index, theme::mode())
}

#[allow(dead_code)]
pub(crate) fn render_quest_handoff_plain(quest: &Quest, next_index: usize) -> String {
    render_quest_handoff_with_mode(quest, next_index, TerminalMode::NoColor)
}

fn render_quest_handoff_with_mode(quest: &Quest, next_index: usize, mode: TerminalMode) -> String {
    let p = palette_for(mode);
    let mut frame = String::new();

    push_section_header(&mut frame, &p, "Cast handoff");
    push_label_row(&mut frame, &p, "quest", &quest.title);
    push_label_row(
        &mut frame,
        &p,
        "phase",
        &quest_phase_position_label(quest, next_index),
    );

    if let Some(next) = quest.phases.get(next_index) {
        if let Some(handoff) = &next.handoff {
            push_label_row(&mut frame, &p, "from", &handoff.from_phase);
            push_label_row(&mut frame, &p, "prior", &handoff.prior_status);
            push_continuation_row(&mut frame, &p, &handoff.reason);
            if !handoff.carried_context.is_empty() {
                frame.push('\n');
                push_section_header(&mut frame, &p, "Carried context");
                for fact in handoff.carried_context.iter().take(4) {
                    push_note_row(&mut frame, &p, fact);
                }
            }
        } else {
            push_label_row(&mut frame, &p, "from", "(quest start)");
        }

        let harness_label = next
            .harness
            .map(|h| h.label())
            .unwrap_or("(default harness)");
        let edited_marker = if next.edited_by_user {
            " · user-edited"
        } else {
            ""
        };
        push_label_row(
            &mut frame,
            &p,
            "delegate to",
            &format!("{harness_label}{edited_marker}"),
        );

        frame.push('\n');
        push_section_header(&mut frame, &p, "Sub-prompt");
        for line in clip_sub_prompt_lines(&next.sub_prompt) {
            push_sub_prompt_line(&mut frame, &p, &line);
        }

        frame.push('\n');
        push_footer_hint(&mut frame, &p, quest_handoff_footer_hint(next));
    } else {
        push_label_row(&mut frame, &p, "status", "quest complete");
        frame.push('\n');
        push_footer_hint(&mut frame, &p, "no further phases · type a new spell");
    }

    frame
}

fn quest_phase_position_label(quest: &Quest, next_index: usize) -> String {
    let total = quest.phases.len();
    let phase_name = quest
        .phases
        .get(next_index)
        .map(|p| p.name.as_str())
        .unwrap_or("(end)");
    let position = (next_index + 1).min(total.max(1));
    format!("{position}/{total} · {phase_name}")
}

fn quest_handoff_footer_hint(next: &QuestPhase) -> &'static str {
    match &next.status {
        QuestPhaseStatus::Pending => {
            "enter approve · type to replace · /skip [reason] · /cancel [reason]"
        }
        QuestPhaseStatus::Running { .. } => "phase running · attach to follow",
        QuestPhaseStatus::Complete(_) => "phase already complete · advance again",
        QuestPhaseStatus::Skipped { .. } => "phase skipped · advance to continue",
    }
}

/// Keep the visible sub-prompt block bounded; long composer output (with
/// many carried-context bullets) would push the footer out of the user's
/// view. We cap at 8 lines, with an ellipsis line at the end so the user
/// knows there is more text the harness will receive.
const SUB_PROMPT_VISIBLE_LINES: usize = 8;

fn clip_sub_prompt_lines(sub_prompt: &str) -> Vec<String> {
    let lines: Vec<&str> = sub_prompt.lines().collect();
    if lines.len() <= SUB_PROMPT_VISIBLE_LINES {
        return lines.iter().map(|l| (*l).to_string()).collect();
    }
    let mut out: Vec<String> = lines
        .iter()
        .take(SUB_PROMPT_VISIBLE_LINES - 1)
        .map(|l| (*l).to_string())
        .collect();
    out.push(format!(
        "… {} more lines",
        lines.len() - (SUB_PROMPT_VISIBLE_LINES - 1)
    ));
    out
}

fn push_sub_prompt_line(frame: &mut String, p: &Palette, line: &str) {
    frame.push_str(&format!("  {}{}{}\n", p.text, line, p.reset));
}

/// What the user typed (or, if Cast built the plan without raw input, the
/// most descriptive fallback). The visual contract calls this `spell`.
fn plan_spell_value(plan: &CastPlan) -> String {
    if !plan.raw_spell.is_empty() {
        plan.raw_spell.clone()
    } else if let Some(title) = &plan.title {
        title.clone()
    } else {
        plan.headline.clone()
    }
}

/// One-line, DIM footer that tells the user how to leave or continue. The
/// risk level changes the verb so the message tracks what the gate will
/// actually ask for.
fn plan_footer_hint(plan: &CastPlan) -> &'static str {
    use crate::tui::cast::intent::CastIntent;

    if matches!(plan.intent, CastIntent::SacrificeSession { .. }) {
        return "type sacrifice to confirm · esc cancels";
    }
    match plan.risk() {
        CastRisk::Safe => "press enter to cast · esc cancels",
        CastRisk::Confirm => "review the risk note · y/N to confirm · esc cancels",
        CastRisk::Reject => "Cast will not run this · type to reframe",
    }
}

/// Section heading rendered in `PRIMARY_STRONG`, Title Case, no decoration.
fn push_section_header(frame: &mut String, p: &Palette, title: &str) {
    frame.push_str(&format!("{}{}{}\n", p.primary_strong, title, p.reset));
}

/// `label    value` row with a fixed 14-char label column. Two-space gap
/// before the value so the eye locks onto a single value column across the
/// whole frame.
fn push_label_row(frame: &mut String, p: &Palette, label: &str, value: &str) {
    let label_block = format!("{:<width$}", label, width = LABEL_COLUMN_WIDTH);
    frame.push_str(&format!(
        "{}{}{}  {}{}{}\n",
        p.field_label, label_block, p.reset, p.text, value, p.reset
    ));
}

/// Risk row: label + 10-char ALL-CAPS chip colored by severity.
fn push_risk_row(frame: &mut String, p: &Palette, mode: TerminalMode, risk: CastRisk) {
    let label_block = format!("{:<width$}", "risk", width = LABEL_COLUMN_WIDTH);
    let chip_fg = risk_chip_fg(mode, risk);
    frame.push_str(&format!(
        "{}{}{}  {}{}{}\n",
        p.field_label,
        label_block,
        p.reset,
        chip_fg,
        risk_chip_text(risk),
        p.reset,
    ));
}

/// Continuation prose under a label row. Indented to the value column so
/// it visually belongs to the row above. Used for risk reasons today.
fn push_continuation_row(frame: &mut String, p: &Palette, value: &str) {
    let pad = " ".repeat(LABEL_COLUMN_WIDTH + 2);
    frame.push_str(&format!("{}{}{}{}\n", pad, p.text_dim, value, p.reset));
}

/// `  N.  kind        note` line for a single plan step.
fn push_step_row(frame: &mut String, p: &Palette, index: usize, kind: CastStepKind, note: &str) {
    let kind_block = format!("{:<9}", step_kind_label(kind));
    frame.push_str(&format!(
        "  {}{}.{}  {}{}{}  {}{}{}\n",
        p.field_label, index, p.reset, p.primary, kind_block, p.reset, p.text, note, p.reset,
    ));
}

/// `  · note` row inside the Notes section of the outcome card.
fn push_note_row(frame: &mut String, p: &Palette, note: &str) {
    frame.push_str(&format!(
        "  {}·{}  {}{}{}\n",
        p.primary, p.reset, p.text, note, p.reset
    ));
}

/// Single dim footer hint. Never two lines, never punctuated.
fn push_footer_hint(frame: &mut String, p: &Palette, hint: &str) {
    frame.push_str(&format!("{}{}{}\n", p.dim, hint, p.reset));
}

fn risk_chip_text(risk: CastRisk) -> &'static str {
    match risk {
        CastRisk::Safe => CHIP_SAFE,
        CastRisk::Confirm => CHIP_CONFIRM,
        CastRisk::Reject => CHIP_REJECT,
    }
}

fn risk_chip_fg(mode: TerminalMode, risk: CastRisk) -> Fg {
    match risk {
        CastRisk::Safe => Fg::with_mode(theme::PRIMARY_STRONG, mode),
        CastRisk::Confirm => Fg::with_mode(theme::PRIMARY, mode),
        CastRisk::Reject => Fg::with_mode(theme::DANGER, mode),
    }
}

fn step_kind_label(kind: CastStepKind) -> &'static str {
    match kind {
        CastStepKind::LaunchSession => "launch",
        CastStepKind::Browse => "browse",
        CastStepKind::Attach => "attach",
        CastStepKind::Summon => "summon",
        CastStepKind::Archive => "archive",
        CastStepKind::Kill => "kill",
        CastStepKind::Sacrifice => "sacrifice",
        CastStepKind::Diagnose => "diagnose",
        CastStepKind::Inform => "inform",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::tui::cast::intent::{CastHarness, CastIntent};
    use crate::tui::cast::plan::build_plan;

    fn codex() -> Option<CastHarness> {
        Some(CastHarness::Codex)
    }

    #[test]
    fn non_interactive_frame_introduces_cast_and_lists_spells() {
        let project = PathBuf::from("/tmp/some-repo");
        let frame = render_cast_frame_plain(Some(&project), Some("codex"));

        assert!(frame.contains("Cast"));
        assert!(frame.contains("Coven familiar"));
        assert!(frame.contains("/tmp/some-repo"));
        assert!(frame.contains("codex"));
        assert!(frame.contains("fix the failing tests"));
        assert!(frame.contains("run claude polish the README"));
        assert!(frame.contains("/sessions"));
    }

    #[test]
    fn non_interactive_frame_handles_missing_project_and_harness() {
        let frame = render_cast_frame_plain(None, None);
        assert!(frame.contains("not inside a project root"));
        assert!(frame.contains("coven doctor"));
    }

    fn natural_plan(prompt: &str) -> CastPlan {
        build_plan(
            CastIntent::NaturalSpell {
                prompt: prompt.to_string(),
            },
            codex,
        )
        .unwrap()
        .with_raw_spell(prompt)
    }

    fn slash_plan(raw: &str) -> CastPlan {
        let intent = crate::tui::cast::intent::parse_spell(raw).unwrap();
        build_plan(intent, codex).unwrap().with_raw_spell(raw)
    }

    fn assert_no_ansi_leakage(frame: &str) {
        assert!(
            !frame.contains('\x1b'),
            "plain-mode frame leaked an ANSI escape sequence:\n{frame}"
        );
    }

    fn assert_label_column(frame: &str, label: &str) {
        let expected = format!("{:<width$}  ", label, width = LABEL_COLUMN_WIDTH);
        assert!(
            frame.contains(&expected),
            "expected label `{label}` padded to {LABEL_COLUMN_WIDTH} chars and followed by 2-space gap, frame:\n{frame}"
        );
    }

    #[test]
    fn plan_card_uses_section_headers_and_field_columns() {
        let plan = natural_plan("fix the failing tests");
        let frame = render_plan_intro_plain(&plan);

        assert!(frame.contains("Cast plan"), "missing identity header");
        assert!(frame.contains("Steps"), "missing Steps section");
        assert_label_column(&frame, "spell");
        assert_label_column(&frame, "harness");
        assert_label_column(&frame, "risk");
        // The redesigned card drops the trailing-colon labels and the
        // bracketed step-kind chips.
        assert!(
            !frame.contains("Spell:"),
            "old `Spell:` label still present"
        );
        assert!(!frame.contains("Risk:"), "old `Risk:` label still present");
        assert!(
            !frame.contains("[launch]"),
            "old `[launch]` chip still present"
        );
        assert_no_ansi_leakage(&frame);
    }

    #[test]
    fn plan_card_shows_user_typed_spell_text_not_just_the_headline() {
        let plan = natural_plan("polish the README");
        let frame = render_plan_intro_plain(&plan);
        // The raw spell text the user typed is what the `spell` row carries.
        assert!(
            frame.contains("polish the README"),
            "spell row should echo user-typed text, frame:\n{frame}"
        );
    }

    #[test]
    fn plan_card_safe_chip_is_fixed_width_and_no_glyph_prefix() {
        let plan = natural_plan("fix the failing tests");
        let frame = render_plan_intro_plain(&plan);

        assert!(frame.contains(CHIP_SAFE), "missing SAFE chip");
        assert_eq!(CHIP_SAFE.chars().count(), 10, "chip must be 10 chars wide");
        // No leading `!` or `X` glyphs on risk lines.
        assert!(!frame.contains("  ! "), "found legacy `!` glyph prefix");
        assert!(!frame.contains("  X "), "found legacy `X` glyph prefix");
    }

    #[test]
    fn plan_card_confirm_chip_includes_noun_first_reason_line() {
        let plan = natural_plan("git push the changes to main");
        let frame = render_plan_intro_plain(&plan);

        assert!(frame.contains(CHIP_CONFIRM), "missing CONFIRM chip");
        // The classifier's noun-first reason appears as a continuation row
        // aligned to the value column — no `!` glyph, no `Risk:` label.
        assert!(
            frame.contains("spell mentions publishing, pushing, merging, or releasing"),
            "missing reason continuation row, frame:\n{frame}"
        );
        assert!(!frame.contains("[  SAFE  ]"));
        assert!(!frame.contains("[ REJECT ]"));
    }

    #[test]
    fn plan_card_reject_chip_for_dangerous_prompt() {
        let plan = natural_plan("rm -rf / now");
        let frame = render_plan_intro_plain(&plan);

        assert!(frame.contains(CHIP_REJECT), "missing REJECT chip");
        assert!(
            frame.contains("destructive shell command"),
            "reject reason missing, frame:\n{frame}"
        );
        // Reject footer hints the user to reframe, not just press enter.
        assert!(
            frame.contains("type to reframe"),
            "reject footer should guide a reframe, frame:\n{frame}"
        );
    }

    #[test]
    fn plan_card_records_harness_source_for_user_chosen_harness() {
        let plan = build_plan(
            CastIntent::HarnessSpell {
                harness: CastHarness::Claude,
                prompt: "polish the README".to_string(),
            },
            codex,
        )
        .unwrap()
        .with_raw_spell("/claude polish the README");
        let frame = render_plan_intro_plain(&plan);

        assert!(
            frame.contains("Claude Code · user-chosen"),
            "frame:\n{frame}"
        );
        assert!(!frame.contains("Cast default"));
    }

    #[test]
    fn plan_card_records_safe_default_harness_source() {
        let plan = natural_plan("fix the failing tests");
        let frame = render_plan_intro_plain(&plan);

        assert!(frame.contains("Codex · Cast default"), "frame:\n{frame}");
        assert!(!frame.contains("user-chosen"));
    }

    #[test]
    fn plan_card_for_session_action_shows_session_id_row() {
        let plan = slash_plan("/attach abcdef123456");
        let frame = render_plan_intro_plain(&plan);

        assert_label_column(&frame, "session");
        assert!(frame.contains("abcdef123456"), "frame:\n{frame}");
        assert!(
            !frame.contains("harness"),
            "session actions have no harness row"
        );
    }

    #[test]
    fn plan_card_for_sacrifice_uses_typed_word_confirm_footer() {
        let plan = slash_plan("/sacrifice abcdef123456");
        let frame = render_plan_intro_plain(&plan);

        assert!(frame.contains(CHIP_CONFIRM));
        assert!(
            frame.contains("type sacrifice to confirm"),
            "sacrifice footer should ask for the typed confirm word, frame:\n{frame}"
        );
    }

    #[test]
    fn plan_card_for_system_action_shows_intent_when_no_harness() {
        let plan = slash_plan("/sessions");
        let frame = render_plan_intro_plain(&plan);

        assert_label_column(&frame, "intent");
        assert!(
            frame.contains("Open the active session browser"),
            "intent row should echo the planner's headline, frame:\n{frame}"
        );
    }

    #[test]
    fn plan_card_truncates_to_four_visible_steps() {
        // Phase 1 plans never exceed two real steps, so this is a guard-rail
        // for future planners that might emit more — the renderer caps at 4.
        let mut plan = natural_plan("fix the failing tests");
        for i in 0..10 {
            plan.steps.push(crate::tui::cast::plan::CastStep {
                kind: CastStepKind::Inform,
                note: format!("extra step {i}"),
            });
        }
        let frame = render_plan_intro_plain(&plan);
        assert!(frame.contains("extra step 0"));
        assert!(frame.contains("extra step 2"));
        assert!(
            !frame.contains("extra step 5"),
            "renderer should clip to 4 visible steps, frame:\n{frame}"
        );
    }

    #[test]
    fn plan_card_plain_output_has_no_ansi_escapes_in_any_risk_state() {
        for raw in [
            "fix the failing tests",
            "git push the changes to main",
            "rm -rf / now",
            "/sessions",
            "/attach abcdef123456",
            "/sacrifice abcdef123456",
        ] {
            let plan = match crate::tui::cast::intent::parse_spell(raw) {
                Ok(intent) => build_plan(intent, codex).unwrap().with_raw_spell(raw),
                Err(_) => continue,
            };
            assert_no_ansi_leakage(&render_plan_intro_plain(&plan));
        }
    }

    #[test]
    fn outcome_card_uses_section_headers_and_field_columns() {
        let outcome = CastOutcome {
            request: "fix the failing tests".to_string(),
            launched: Some("Codex session (project-scoped)".to_string()),
            session_id: Some("abcdef-1234".to_string()),
            next_step: Some("Run `coven attach abcdef-1234` to revisit".to_string()),
            notes: vec!["Session finished: status `clean`, exit code 0".to_string()],
        };

        let frame = render_outcome_plain(&outcome);
        assert!(frame.contains("Cast outcome"));
        assert!(frame.contains("Notes"));
        assert_label_column(&frame, "spell");
        assert_label_column(&frame, "launched");
        assert_label_column(&frame, "session");
        assert_label_column(&frame, "next");
        // Old colon-suffixed labels are gone.
        assert!(!frame.contains("Launched:"));
        assert!(!frame.contains("Session id:"));
        // The next-step value remains copy-pastable.
        assert!(frame.contains("coven attach abcdef-1234"));
        // Note prefix is a thin middle dot per the contract — no hyphen bullets.
        assert!(
            frame.contains("·  Session finished"),
            "notes should use a `·` bullet, frame:\n{frame}"
        );
        assert!(
            !frame.contains("- Session finished"),
            "notes must not use hyphen bullets, frame:\n{frame}"
        );
        assert_no_ansi_leakage(&frame);
    }

    #[test]
    fn outcome_card_omits_optional_rows_when_unset() {
        let outcome = CastOutcome::for_request("/sessions");
        let frame = render_outcome_plain(&outcome);
        assert!(frame.contains("Cast outcome"));
        assert_label_column(&frame, "spell");
        assert!(!frame.contains("launched"));
        assert!(!frame.contains("session "));
        assert!(!frame.contains("Notes"));
        assert!(!frame.contains("next"));
    }

    #[test]
    fn outcome_card_caps_notes_to_three_visible() {
        let outcome = CastOutcome {
            request: "fix tests".to_string(),
            launched: None,
            session_id: None,
            next_step: None,
            notes: (0..6).map(|i| format!("note {i}")).collect(),
        };
        let frame = render_outcome_plain(&outcome);
        assert!(frame.contains("note 0"));
        assert!(frame.contains("note 2"));
        assert!(
            !frame.contains("note 4"),
            "outcome should clip to 3 notes, frame:\n{frame}"
        );
    }

    #[test]
    fn quest_handoff_card_shows_source_phase_status_and_next_sub_prompt() {
        use crate::tui::cast::quest::{advance, quest_from_goal, QuestPhaseSummary};

        let mut quest = quest_from_goal("ship phase 5 sub-prompting", Some(CastHarness::Codex));
        let next = advance(
            &mut quest,
            QuestPhaseSummary {
                session_id: Some("session-abc123".to_string()),
                exit_status: Some("completed".to_string()),
                exit_code: Some(0),
                carried_context: vec![
                    "added `cast::quest` module".to_string(),
                    "drafted handoff card".to_string(),
                ],
            },
        )
        .expect("advance should yield the implement phase");

        let frame = render_quest_handoff_plain(&quest, next);

        assert!(
            frame.contains("Cast handoff"),
            "missing header, frame:\n{frame}"
        );
        assert_label_column(&frame, "quest");
        assert_label_column(&frame, "phase");
        assert_label_column(&frame, "from");
        assert_label_column(&frame, "prior");
        assert_label_column(&frame, "delegate to");

        assert!(frame.contains("ship phase 5 sub-prompting"));
        assert!(frame.contains("2/3 · implement"));
        assert!(frame.contains("Codex"));
        assert!(
            frame.contains("Phase `design` finished with `completed (exit 0)`"),
            "handoff reason should describe prior status, frame:\n{frame}"
        );
        assert!(
            frame.contains("Sub-prompt"),
            "render must surface the next sub-prompt block"
        );
        assert!(
            frame.contains("ship phase 5 sub-prompting"),
            "sub-prompt should echo the user's goal verbatim"
        );
        assert!(frame.contains("added `cast::quest` module"));
        assert_no_ansi_leakage(&frame);
    }

    #[test]
    fn quest_handoff_card_marks_user_edited_sub_prompt_so_users_can_tell() {
        use crate::tui::cast::quest::{
            advance, quest_from_goal, set_phase_sub_prompt, QuestPhaseSummary,
        };

        let mut quest = quest_from_goal("rotate the daemon socket", Some(CastHarness::Codex));
        set_phase_sub_prompt(
            &mut quest,
            1,
            "Move the socket to `$XDG_RUNTIME_DIR/coven.sock`.".to_string(),
        )
        .unwrap();
        let next = advance(
            &mut quest,
            QuestPhaseSummary {
                exit_status: Some("completed".to_string()),
                exit_code: Some(0),
                ..QuestPhaseSummary::default()
            },
        )
        .unwrap();

        let frame = render_quest_handoff_plain(&quest, next);
        assert!(
            frame.contains("user-edited"),
            "user-authored sub_prompts should be flagged, frame:\n{frame}"
        );
        assert!(frame.contains("Move the socket to `$XDG_RUNTIME_DIR/coven.sock`."));
    }

    #[test]
    fn quest_handoff_card_handles_quest_complete_state_with_no_panic() {
        use crate::tui::cast::quest::{advance, quest_from_goal, QuestPhaseSummary};

        let mut quest = quest_from_goal("trivial", Some(CastHarness::Codex));
        // Drain all phases.
        advance(&mut quest, QuestPhaseSummary::default());
        advance(&mut quest, QuestPhaseSummary::default());
        advance(&mut quest, QuestPhaseSummary::default());
        assert!(quest.is_complete());

        // Asking for the handoff at the past-the-end cursor must not panic.
        let frame = render_quest_handoff_plain(&quest, quest.phases.len());
        assert!(frame.contains("quest complete"), "frame:\n{frame}");
    }

    #[test]
    fn quest_handoff_card_clips_very_long_sub_prompts() {
        use crate::tui::cast::quest::{
            advance, quest_from_goal, set_phase_sub_prompt, QuestPhaseSummary,
        };

        let mut quest = quest_from_goal("anything", Some(CastHarness::Codex));
        // Compose a sub-prompt with many lines so the renderer clips.
        let long: String = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        set_phase_sub_prompt(&mut quest, 1, long).unwrap();
        let next = advance(&mut quest, QuestPhaseSummary::default()).unwrap();

        let frame = render_quest_handoff_plain(&quest, next);
        assert!(frame.contains("line 0"));
        assert!(
            frame.contains("more lines"),
            "long sub_prompt should be clipped with a `… N more lines` indicator, frame:\n{frame}"
        );
        // The last few lines should NOT appear once we clip.
        assert!(!frame.contains("line 29"));
    }

    #[test]
    fn risk_chip_colors_change_with_severity_in_true_color_mode() {
        // The chip text stays the same; the foreground escape changes by
        // severity. Spot-check the escapes by rendering against a TrueColor
        // palette directly so we don't depend on the cached `mode()` value.
        let safe_plan = natural_plan("fix the failing tests");
        let confirm_plan = natural_plan("git push to main");
        let reject_plan = natural_plan("rm -rf / now");

        let safe = render_plan_intro_with_mode(&safe_plan, TerminalMode::TrueColor);
        let confirm = render_plan_intro_with_mode(&confirm_plan, TerminalMode::TrueColor);
        let reject = render_plan_intro_with_mode(&reject_plan, TerminalMode::TrueColor);

        // PRIMARY_STRONG (0x9A, 0x8E, 0xCD) for SAFE.
        assert!(safe.contains("\x1b[38;2;154;142;205m"));
        // PRIMARY (0xC5, 0xBD, 0xED) for CONFIRM.
        assert!(confirm.contains("\x1b[38;2;197;189;237m"));
        // DANGER (0xFF, 0x3B, 0x30) for REJECT.
        assert!(reject.contains("\x1b[38;2;255;59;48m"));
    }
}
