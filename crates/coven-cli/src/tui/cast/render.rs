//! Cast rendering.
//!
//! Phase 1 keeps Cast's voice plain-text and color-aware so the renderer
//! works in interactive terminals, non-interactive pipes, and tests. The
//! goal here is not a beautiful TUI — it is a Cast frame the user can read
//! before and after every spell.

use std::path::Path;

use crate::theme::{self, fit_chars, TerminalMode};

use super::outcome::CastOutcome;
use super::plan::{CastHarnessSource, CastPlan, CastStepKind};
use super::safety::{CastRisk, SafetyDecision};

const CAST_INTRO_INNER_WIDTH: usize = 76;

/// One-line salute at the top of every Cast frame. Used by both the
/// interactive launcher (woven into the shell frame) and the non-interactive
/// fallback so logs and pipes always show the familiar's name.
pub(crate) fn cast_salute() -> &'static str {
    "Cast, your Coven familiar, is ready. Type a spell, or use a slash command."
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
    let primary_strong = theme::Fg::with_mode(theme::PRIMARY_STRONG, mode);
    let primary = theme::Fg::with_mode(theme::PRIMARY, mode);
    let field_label = theme::Fg::with_mode(theme::FIELD_LABEL, mode);
    let user_label = theme::Fg::with_mode(theme::USER_LABEL, mode);
    let dim = theme::Fg::with_mode(theme::DIM, mode);
    let reset = theme::Reset::with_mode(mode);
    let inner_width = CAST_INTRO_INNER_WIDTH;
    let mut frame = String::new();

    frame.push_str(&format!(
        "{primary_strong}Cast — your Coven familiar{reset}\n"
    ));
    frame.push_str(&format!(
        "{field_label}{}{reset}\n",
        fit_chars(cast_salute(), inner_width)
    ));
    frame.push('\n');

    frame.push_str(&format!("{primary_strong}Context{reset}\n"));
    let project = project_root
        .map(|root| root.display().to_string())
        .unwrap_or_else(|| "not inside a project root — run from a repo".to_string());
    frame.push_str(&format!(
        "{field_label}Project{reset}        {}\n",
        fit_chars(&project, inner_width.saturating_sub(15))
    ));
    let harness = default_harness.unwrap_or("none ready — run `coven doctor`");
    frame.push_str(&format!(
        "{field_label}Default harness{reset} {}\n",
        fit_chars(harness, inner_width.saturating_sub(15))
    ));
    frame.push('\n');

    frame.push_str(&format!("{primary_strong}Example spells{reset}\n"));
    for spell in cast_example_spells() {
        frame.push_str(&format!("{primary}  {}{reset}\n", spell));
    }
    frame.push('\n');

    frame.push_str(&format!("{primary_strong}Slash spells{reset}\n"));
    for spell in cast_example_slashes() {
        frame.push_str(&format!("{user_label}  {}{reset}\n", spell));
    }
    frame.push('\n');

    frame.push_str(&format!(
        "{dim}Tip: in a terminal, `coven` opens the Cast launcher. Empty input opens the slash palette.{reset}\n"
    ));
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
    let primary_strong = theme::Fg::with_mode(theme::PRIMARY_STRONG, mode);
    let primary = theme::Fg::with_mode(theme::PRIMARY, mode);
    let field_label = theme::Fg::with_mode(theme::FIELD_LABEL, mode);
    let user_label = theme::Fg::with_mode(theme::USER_LABEL, mode);
    let reset = theme::Reset::with_mode(mode);
    let mut frame = String::new();

    frame.push_str(&format!("{primary_strong}Cast plan{reset}\n"));
    frame.push_str(&format!("{field_label}Spell:{reset} {}\n", plan.headline));

    if let Some(plan_harness) = plan.harness {
        let source = match plan_harness.source {
            CastHarnessSource::UserChose => "user-chosen",
            CastHarnessSource::SafeDefault => "Cast default",
        };
        frame.push_str(&format!(
            "{field_label}Harness:{reset} {} ({})\n",
            plan_harness.harness.label(),
            source
        ));
    }

    if let Some(title) = &plan.title {
        frame.push_str(&format!("{field_label}Session title:{reset} {}\n", title));
    }

    frame.push_str(&format!(
        "{field_label}Risk:{reset} {}\n",
        risk_label(plan.risk())
    ));
    if let SafetyDecision::Confirm { reason, suggestion } = &plan.decision {
        frame.push_str(&format!(
            "{user_label}  ! {} — {}{reset}\n",
            reason, suggestion
        ));
    }
    if let SafetyDecision::Reject {
        reason,
        alternative,
    } = &plan.decision
    {
        frame.push_str(&format!(
            "{user_label}  X {} — {}{reset}\n",
            reason, alternative
        ));
    }

    if !plan.steps.is_empty() {
        frame.push_str(&format!("{primary_strong}Steps{reset}\n"));
        for step in &plan.steps {
            frame.push_str(&format!(
                "{primary}  - [{}] {}{reset}\n",
                step_kind_label(step.kind),
                step.note
            ));
        }
    }
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
    let primary_strong = theme::Fg::with_mode(theme::PRIMARY_STRONG, mode);
    let primary = theme::Fg::with_mode(theme::PRIMARY, mode);
    let field_label = theme::Fg::with_mode(theme::FIELD_LABEL, mode);
    let reset = theme::Reset::with_mode(mode);
    let mut frame = String::new();

    frame.push_str(&format!("{primary_strong}Cast outcome{reset}\n"));
    frame.push_str(&format!("{field_label}Spell:{reset} {}\n", outcome.request));
    if let Some(launched) = &outcome.launched {
        frame.push_str(&format!("{field_label}Launched:{reset} {}\n", launched));
    }
    if let Some(session_id) = &outcome.session_id {
        frame.push_str(&format!("{field_label}Session id:{reset} {}\n", session_id));
    }
    if !outcome.notes.is_empty() {
        frame.push_str(&format!("{primary_strong}Notes{reset}\n"));
        for note in &outcome.notes {
            frame.push_str(&format!("{primary}  - {}{reset}\n", note));
        }
    }
    if let Some(next) = &outcome.next_step {
        frame.push_str(&format!("{field_label}Next:{reset} {}\n", next));
    }
    frame
}

fn risk_label(risk: CastRisk) -> &'static str {
    match risk {
        CastRisk::Safe => "safe",
        CastRisk::Confirm => "confirmation-required",
        CastRisk::Reject => "rejected",
    }
}

fn step_kind_label(kind: CastStepKind) -> &'static str {
    match kind {
        CastStepKind::LaunchSession => "launch",
        CastStepKind::Browse => "browse",
        CastStepKind::Attach => "attach",
        CastStepKind::Summon => "summon",
        CastStepKind::Archive => "archive",
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

    #[test]
    fn intro_card_shows_safe_default_source_and_session_title() {
        let plan = build_plan(
            CastIntent::NaturalSpell {
                prompt: "fix the failing tests".to_string(),
            },
            codex,
        )
        .unwrap();

        let frame = render_plan_intro_plain(&plan);
        assert!(frame.contains("Cast plan"));
        assert!(frame.contains("Codex"));
        assert!(frame.contains("Cast default"));
        assert!(frame.contains("Session title: fix the failing tests"));
        assert!(frame.contains("Risk: safe"));
        assert!(frame.contains("[launch]"));
    }

    #[test]
    fn intro_card_surfaces_confirm_reason_for_risky_spell() {
        let plan = build_plan(
            CastIntent::NaturalSpell {
                prompt: "git push the changes to main".to_string(),
            },
            codex,
        )
        .unwrap();

        let frame = render_plan_intro_plain(&plan);
        assert!(frame.contains("Risk: confirmation-required"));
        assert!(frame.contains("push"));
    }

    #[test]
    fn intro_card_surfaces_reject_reason_for_rejected_spell() {
        let plan = build_plan(
            CastIntent::NaturalSpell {
                prompt: "rm -rf / now".to_string(),
            },
            codex,
        )
        .unwrap();

        let frame = render_plan_intro_plain(&plan);
        assert!(frame.contains("Risk: rejected"));
    }

    #[test]
    fn outcome_card_includes_session_id_and_next_step() {
        let outcome = CastOutcome {
            request: "fix the failing tests".to_string(),
            launched: Some("Codex session (project-scoped)".to_string()),
            session_id: Some("abcdef-1234".to_string()),
            next_step: Some("`coven attach abcdef-1234` to follow live output".to_string()),
            notes: vec!["risk: safe".to_string()],
        };

        let frame = render_outcome_plain(&outcome);
        assert!(frame.contains("Cast outcome"));
        assert!(frame.contains("Launched: Codex session"));
        assert!(frame.contains("Session id: abcdef-1234"));
        assert!(frame.contains("coven attach abcdef-1234"));
        assert!(frame.contains("risk: safe"));
    }
}
