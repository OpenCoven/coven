//! Cast planner.
//!
//! Phase 1: take a deterministic `CastIntent` and a default-harness resolver
//! and produce a `CastPlan` the executor (and the renderer) can read. The
//! planner never executes anything — it just describes what Cast would do.

use anyhow::Result;

use super::intent::{CastHarness, CastIntent};
use super::safety::{classify_prompt_risk, CastRisk, SafetyDecision};

/// What Cast intends to do in response to a single spell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CastPlan {
    pub(crate) intent: CastIntent,
    pub(crate) headline: String,
    pub(crate) steps: Vec<CastStep>,
    pub(crate) decision: SafetyDecision,
    pub(crate) harness: Option<CastPlanHarness>,
    pub(crate) session_id: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) title: Option<String>,
}

impl CastPlan {
    pub(crate) fn risk(&self) -> CastRisk {
        self.decision.risk()
    }
}

/// The harness Cast resolved for this plan. The planner records *how* it picked
/// the harness so the renderer can explain it to the user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CastPlanHarness {
    pub(crate) harness: CastHarness,
    pub(crate) source: CastHarnessSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CastHarnessSource {
    /// The user named the harness explicitly (slash command or "run claude …").
    UserChose,
    /// Cast picked the safe default because the user only said what they wanted.
    SafeDefault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CastStepKind {
    LaunchSession,
    Browse,
    Attach,
    Summon,
    Archive,
    Sacrifice,
    Diagnose,
    Inform,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CastStep {
    pub(crate) kind: CastStepKind,
    pub(crate) note: String,
}

const DEFAULT_TITLE_CHARS: usize = 48;

/// Build a plan for a parsed spell. The caller passes in a `default_harness`
/// resolver so tests can avoid touching `which`/PATH.
pub(crate) fn build_plan<F>(intent: CastIntent, default_harness: F) -> Result<CastPlan>
where
    F: Fn() -> Option<CastHarness>,
{
    Ok(match intent {
        CastIntent::NaturalSpell { ref prompt } => {
            natural_spell_plan(prompt, intent.clone(), &default_harness)
        }
        CastIntent::HarnessSpell {
            harness,
            ref prompt,
        } => harness_spell_plan(harness, prompt, intent.clone()),
        CastIntent::OpenSessions => simple_plan(
            intent,
            "Open the active session browser",
            CastStep::new(CastStepKind::Browse, "Show project-scoped Coven sessions"),
        ),
        CastIntent::OpenAllSessions => simple_plan(
            intent,
            "Open the full session browser (active + archived)",
            CastStep::new(
                CastStepKind::Browse,
                "Show every Coven session including archived",
            ),
        ),
        CastIntent::AttachSession { ref session_id } => simple_plan(
            intent.clone(),
            &format!("Attach to session {}", short_id(session_id)),
            CastStep::new(
                CastStepKind::Attach,
                "Replay session output and forward live input",
            ),
        ),
        CastIntent::SummonSession { ref session_id } => simple_plan(
            intent.clone(),
            &format!("Summon archived session {}", short_id(session_id)),
            CastStep::new(
                CastStepKind::Summon,
                "Restore the archived session, then replay/follow it",
            ),
        ),
        CastIntent::ArchiveSession { ref session_id } => simple_plan(
            intent.clone(),
            &format!("Archive session {}", short_id(session_id)),
            CastStep::new(
                CastStepKind::Archive,
                "Hide the session from the active list; keep events",
            ),
        ),
        CastIntent::SacrificeSession { ref session_id } => {
            sacrifice_plan(session_id, intent.clone())
        }
        CastIntent::Doctor => simple_plan(
            intent,
            "Run Coven doctor",
            CastStep::new(
                CastStepKind::Diagnose,
                "Check store, project, and harness readiness",
            ),
        ),
        CastIntent::DaemonStatus => simple_plan(
            intent,
            "Check the local Coven daemon",
            CastStep::new(
                CastStepKind::Diagnose,
                "Report the daemon pid, socket, and reachability",
            ),
        ),
        CastIntent::Help => simple_plan(
            intent,
            "Show Cast help",
            CastStep::new(
                CastStepKind::Inform,
                "Examples of natural-language and slash spells",
            ),
        ),
        CastIntent::StartHere => simple_plan(
            intent,
            "Open the Coven quick-start",
            CastStep::new(CastStepKind::Inform, "Setup check and a safe first command"),
        ),
        CastIntent::OpenTui => simple_plan(
            intent,
            "Open the Coven slash palette",
            CastStep::new(CastStepKind::Inform, "Show the Cast launcher"),
        ),
        CastIntent::PatchOpenClaw => simple_plan(
            intent,
            "Open the guided OpenClaw patch room",
            CastStep::new(CastStepKind::Inform, "Walk through `coven patch openclaw`"),
        ),
        CastIntent::Quit => simple_plan(
            intent,
            "Close Cast without changing anything",
            CastStep::new(CastStepKind::Inform, "Exit the launcher"),
        ),
    })
}

fn natural_spell_plan(
    prompt: &str,
    intent: CastIntent,
    default_harness: &dyn Fn() -> Option<CastHarness>,
) -> CastPlan {
    let decision = classify_prompt_risk(prompt);
    let title = derive_title(prompt);
    let headline = format!("Cast a project-scoped spell: {title}");
    let harness = default_harness().map(|harness| CastPlanHarness {
        harness,
        source: CastHarnessSource::SafeDefault,
    });
    let mut steps = vec![CastStep::new(
        CastStepKind::LaunchSession,
        match harness {
            Some(plan_harness) => format!(
                "Launch {} inside this project with the spell as the task",
                plan_harness.harness.label()
            ),
            None => {
                "No harness ready — run `coven doctor` to install Codex or Claude Code".to_string()
            }
        },
    )];
    if let SafetyDecision::Confirm {
        ref reason,
        ref suggestion,
    } = decision
    {
        steps.push(CastStep::new(
            CastStepKind::Inform,
            format!("Risk: {reason}. {suggestion}"),
        ));
    }
    if let SafetyDecision::Reject {
        ref reason,
        ref alternative,
    } = decision
    {
        steps.push(CastStep::new(
            CastStepKind::Inform,
            format!("Rejected: {reason}. {alternative}"),
        ));
    }
    CastPlan {
        intent,
        headline,
        steps,
        decision,
        harness,
        session_id: None,
        prompt: Some(prompt.to_string()),
        title: Some(title),
    }
}

fn harness_spell_plan(harness: CastHarness, prompt: &str, intent: CastIntent) -> CastPlan {
    let decision = classify_prompt_risk(prompt);
    let title = derive_title(prompt);
    let headline = format!("Cast {} on this project: {title}", harness.label());
    let mut steps = vec![CastStep::new(
        CastStepKind::LaunchSession,
        format!(
            "Launch {} inside this project with the spell as the task",
            harness.label()
        ),
    )];
    if let SafetyDecision::Confirm {
        ref reason,
        ref suggestion,
    } = decision
    {
        steps.push(CastStep::new(
            CastStepKind::Inform,
            format!("Risk: {reason}. {suggestion}"),
        ));
    }
    if let SafetyDecision::Reject {
        ref reason,
        ref alternative,
    } = decision
    {
        steps.push(CastStep::new(
            CastStepKind::Inform,
            format!("Rejected: {reason}. {alternative}"),
        ));
    }
    CastPlan {
        intent,
        headline,
        steps,
        decision,
        harness: Some(CastPlanHarness {
            harness,
            source: CastHarnessSource::UserChose,
        }),
        session_id: None,
        prompt: Some(prompt.to_string()),
        title: Some(title),
    }
}

fn sacrifice_plan(session_id: &str, intent: CastIntent) -> CastPlan {
    let decision = SafetyDecision::Confirm {
        reason: "sacrifice permanently deletes a session and its events".to_string(),
        suggestion: "Cast will require typed confirmation in the session browser before \
                     proceeding with the delete."
            .to_string(),
    };
    CastPlan {
        intent,
        headline: format!("Sacrifice session {}", short_id(session_id)),
        steps: vec![
            CastStep::new(
                CastStepKind::Sacrifice,
                "Permanently delete the session and its events",
            ),
            CastStep::new(
                CastStepKind::Inform,
                "Requires explicit typed confirmation before any delete",
            ),
        ],
        decision,
        harness: None,
        session_id: Some(session_id.to_string()),
        prompt: None,
        title: None,
    }
}

fn simple_plan(intent: CastIntent, headline: &str, step: CastStep) -> CastPlan {
    let session_id = match &intent {
        CastIntent::AttachSession { session_id }
        | CastIntent::SummonSession { session_id }
        | CastIntent::ArchiveSession { session_id } => Some(session_id.clone()),
        _ => None,
    };
    CastPlan {
        intent,
        headline: headline.to_string(),
        steps: vec![step],
        decision: SafetyDecision::Proceed,
        harness: None,
        session_id,
        prompt: None,
        title: None,
    }
}

impl CastStep {
    fn new(kind: CastStepKind, note: impl Into<String>) -> Self {
        Self {
            kind,
            note: note.into(),
        }
    }
}

fn derive_title(prompt: &str) -> String {
    let collapsed: String = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "Untitled spell".to_string();
    }
    let count = collapsed.chars().count();
    if count <= DEFAULT_TITLE_CHARS {
        collapsed
    } else {
        let mut out: String = collapsed.chars().take(DEFAULT_TITLE_CHARS - 1).collect();
        out.push('…');
        out
    }
}

fn short_id(session_id: &str) -> String {
    session_id.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex() -> Option<CastHarness> {
        Some(CastHarness::Codex)
    }

    fn claude() -> Option<CastHarness> {
        Some(CastHarness::Claude)
    }

    fn none() -> Option<CastHarness> {
        None
    }

    #[test]
    fn natural_spell_picks_safe_default_and_records_source() {
        let plan = build_plan(
            CastIntent::NaturalSpell {
                prompt: "fix the failing tests".to_string(),
            },
            codex,
        )
        .unwrap();

        let harness = plan.harness.expect("default harness should be resolved");
        assert_eq!(harness.harness, CastHarness::Codex);
        assert_eq!(harness.source, CastHarnessSource::SafeDefault);
        assert_eq!(plan.risk(), CastRisk::Safe);
        assert_eq!(plan.title.as_deref(), Some("fix the failing tests"));
        assert!(plan.headline.contains("fix the failing tests"));
        assert!(plan
            .steps
            .iter()
            .any(|step| step.kind == CastStepKind::LaunchSession));
    }

    #[test]
    fn natural_spell_with_no_default_harness_still_plans_a_launch_step() {
        let plan = build_plan(
            CastIntent::NaturalSpell {
                prompt: "explain this repo".to_string(),
            },
            none,
        )
        .unwrap();

        assert!(plan.harness.is_none());
        let launch_step = plan
            .steps
            .iter()
            .find(|step| step.kind == CastStepKind::LaunchSession)
            .expect("plan should still include a launch step");
        assert!(
            launch_step.note.contains("coven doctor"),
            "missing harness should point at doctor, got {:?}",
            launch_step.note
        );
    }

    #[test]
    fn harness_spell_uses_explicit_harness_choice() {
        let plan = build_plan(
            CastIntent::HarnessSpell {
                harness: CastHarness::Claude,
                prompt: "polish the README".to_string(),
            },
            codex,
        )
        .unwrap();

        let harness = plan.harness.expect("explicit harness should win");
        assert_eq!(harness.harness, CastHarness::Claude);
        assert_eq!(harness.source, CastHarnessSource::UserChose);
        assert!(plan.headline.contains("Claude Code"));
    }

    #[test]
    fn risky_prompt_marks_confirm_in_plan() {
        let plan = build_plan(
            CastIntent::NaturalSpell {
                prompt: "git push the changes to main".to_string(),
            },
            codex,
        )
        .unwrap();

        assert_eq!(plan.risk(), CastRisk::Confirm);
        assert!(plan.steps.iter().any(|step| step.note.contains("Risk:")));
    }

    #[test]
    fn rejected_prompt_marks_reject_in_plan() {
        let plan = build_plan(
            CastIntent::NaturalSpell {
                prompt: "rm -rf / please".to_string(),
            },
            codex,
        )
        .unwrap();

        assert_eq!(plan.risk(), CastRisk::Reject);
        assert!(plan
            .steps
            .iter()
            .any(|step| step.note.contains("Rejected:")));
    }

    #[test]
    fn sessions_intent_plans_a_browse_step() {
        let plan = build_plan(CastIntent::OpenSessions, claude).unwrap();
        assert_eq!(plan.risk(), CastRisk::Safe);
        assert!(plan
            .steps
            .iter()
            .any(|step| step.kind == CastStepKind::Browse));
    }

    #[test]
    fn sacrifice_intent_is_marked_confirm_with_explanation() {
        let plan = build_plan(
            CastIntent::SacrificeSession {
                session_id: "abcdef123456".to_string(),
            },
            codex,
        )
        .unwrap();

        assert_eq!(plan.risk(), CastRisk::Confirm);
        match plan.decision {
            SafetyDecision::Confirm { reason, .. } => {
                assert!(reason.contains("sacrifice"));
            }
            other => panic!("expected confirm, got {other:?}"),
        }
        assert_eq!(plan.session_id.as_deref(), Some("abcdef123456"));
    }

    #[test]
    fn attach_intent_records_session_id_in_plan() {
        let plan = build_plan(
            CastIntent::AttachSession {
                session_id: "abcdef123456".to_string(),
            },
            codex,
        )
        .unwrap();

        assert_eq!(plan.session_id.as_deref(), Some("abcdef123456"));
        assert!(plan
            .steps
            .iter()
            .any(|step| step.kind == CastStepKind::Attach));
        assert!(plan.headline.contains("abcdef123456"));
    }

    #[test]
    fn natural_spell_title_truncates_long_prompts() {
        let prompt = "do everything imaginable that could possibly matter to this repository";
        let plan = build_plan(
            CastIntent::NaturalSpell {
                prompt: prompt.to_string(),
            },
            codex,
        )
        .unwrap();

        let title = plan.title.unwrap();
        assert!(title.chars().count() <= DEFAULT_TITLE_CHARS);
        assert!(title.ends_with('…'));
    }
}
