//! Cast: Coven's integrated first-party familiar.
//!
//! Phase 1 keeps Cast deterministic. The user types a spell, Cast parses it
//! into a `CastIntent`, builds a `CastPlan` describing what would happen,
//! checks the plan against a small safety classifier, and (for routable
//! plans) reports a `CastOutcome` after the existing CLI handler does the
//! real work.
//!
//! The Rust daemon, session ledger, harness adapters, and project-root guard
//! all remain the authority. Cast is orchestration and presentation: it
//! never bypasses the daemon and never invents a second runtime.

pub(crate) mod attach;
pub(crate) mod follow;
pub(crate) mod gate;
pub(crate) mod intent;
pub(crate) mod outcome;
pub(crate) mod plan;
pub(crate) mod quest;
pub(crate) mod render;
pub(crate) mod safety;

use anyhow::Result;

use crate::harness;

pub(crate) use attach::{
    find_cast_quest_info, find_cast_summary, format_quest_attach_note, format_summary_note,
    reconstruct_quest, ReconstructedQuest,
};
pub(crate) use follow::{follow_until_exit, CastSessionExit, FollowerObserver, FollowerPacer};
pub(crate) use gate::{evaluate_gate, GateOutcome};
pub(crate) use intent::{parse_spell, CastHarness, CastIntent, ObserveView};
pub(crate) use outcome::CastOutcome;
pub(crate) use plan::{build_plan, CastPlan};
pub(crate) use quest::{
    advance as advance_quest, mark_phase_running, parse_phase_action, quest_from_goal,
    set_phase_sub_prompt, skip_phase, PhaseInteraction, Quest, QuestPhase, QuestPhaseStatus,
    QuestPhaseSummary, CAST_QUEST_ADVANCED_KIND, CAST_QUEST_COMPLETED_KIND,
    CAST_QUEST_PHASE_COMPLETED_KIND, CAST_QUEST_PHASE_EDITED_KIND, CAST_QUEST_PHASE_SKIPPED_KIND,
    CAST_QUEST_PHASE_STARTED_KIND, CAST_QUEST_STARTED_KIND, PHASE_PROMPT_HINT,
};
// `compose_sub_prompt` and `QuestHandoff` are exercised by the in-module
// test suite; they remain in the public crate surface so a future async /
// detached-quest UX can read them without further plumbing.
#[allow(unused_imports)]
pub(crate) use quest::{compose_sub_prompt, QuestHandoff};
pub(crate) use render::{
    render_cast_frame_for_terminal, render_outcome, render_plan_intro, render_quest_handoff,
};
pub(crate) use safety::SafetyDecision;

// Re-exports used only by tests in `crate::tests` (main.rs). Bundled here
// rather than scattered behind `cfg(test)` so the names live next to the
// rest of the Cast surface.
#[cfg(test)]
pub(crate) use plan::CastStepKind;
#[cfg(test)]
pub(crate) use render::render_cast_frame_plain;
#[cfg(test)]
pub(crate) use safety::CastRisk;

/// Build a plan from raw user text, using the installed harnesses on PATH
/// to resolve the safe default. Tests should prefer `build_plan` directly
/// with an injected resolver so PATH lookups stay out of the suite.
///
/// The raw user text is preserved on the returned plan so the outcome card
/// can show the spell exactly as the user typed it.
pub(crate) fn plan_spell(raw: &str) -> Result<CastPlan> {
    let intent = parse_spell(raw)?;
    Ok(build_plan(intent, default_harness)?.with_raw_spell(raw))
}

/// Resolve Cast's safe default harness from the host's installed adapters.
/// Codex wins; Claude is the fallback; otherwise `None`.
pub(crate) fn default_harness() -> Option<CastHarness> {
    let harnesses = harness::built_in_harnesses();
    if harnesses.iter().any(|h| h.id == "codex" && h.available) {
        return Some(CastHarness::Codex);
    }
    if harnesses.iter().any(|h| h.id == "claude" && h.available) {
        return Some(CastHarness::Claude);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_spell_with_empty_input_opens_launcher() {
        let plan = plan_spell("").unwrap();
        assert!(matches!(plan.intent, CastIntent::OpenTui));
    }

    #[test]
    fn plan_spell_with_natural_text_produces_a_launch_plan() {
        let plan = plan_spell("fix the failing tests").unwrap();
        assert!(matches!(plan.intent, CastIntent::NaturalSpell { .. }));
        assert!(plan
            .steps
            .iter()
            .any(|step| step.kind == CastStepKind::LaunchSession));
    }

    #[test]
    fn plan_spell_propagates_parser_errors() {
        let error = plan_spell("/banana").unwrap_err();
        assert!(error.to_string().contains("unknown Cast slash command"));
    }

    #[test]
    fn plan_spell_preserves_raw_user_text_for_outcome_card() {
        let plan = plan_spell("run claude polish the README").unwrap();
        assert_eq!(plan.raw_spell, "run claude polish the README");
    }

    #[test]
    fn plan_spell_preserves_raw_text_even_when_intent_is_session_action() {
        let plan = plan_spell("/sacrifice abcdef123456").unwrap();
        assert_eq!(plan.raw_spell, "/sacrifice abcdef123456");
        assert!(matches!(plan.intent, CastIntent::SacrificeSession { .. }));
    }
}
