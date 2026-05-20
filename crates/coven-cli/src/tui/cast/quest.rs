//! Cast quest flow — deterministic sub-prompting for sequential goals.
//!
//! Phase 5 takes a high-level user goal and decomposes it into an ordered
//! `Quest` of structured phases. Each [`QuestPhase`] owns a concrete
//! `sub_prompt`: the literal text Cast would hand to a harness if the user
//! approves it right now. After a phase finishes, the caller records a
//! [`QuestPhaseSummary`] and calls [`advance`]; the next pending phase
//! receives a [`QuestHandoff`] describing what changed and *why* its
//! sub-prompt was updated.
//!
//! The composer is intentionally deterministic and local-first. No LLM
//! planner is invoked inside Cast — sub-prompts are assembled from
//! structured templates plus the recorded prior-phase outcome. That makes
//! every handoff inspectable, reproducible in tests, and overridable by the
//! user before the harness sees it.
//!
//! Integration boundary: this module is pure. The Cast shell wires the
//! quest into its existing gate / follow / outcome surfaces; `quest.rs`
//! does no IO and never reaches the daemon directly. Until the shell
//! integration lands, the surface is exercised only by the in-module test
//! suite — `#![allow(dead_code)]` keeps the warning noise off the seam.

#![allow(dead_code)]

use anyhow::{anyhow, Result};

use super::intent::CastHarness;

/// A sequential goal Cast is guiding the user through. Owns the original
/// user request and an ordered list of [`QuestPhase`]s. `cursor` points at
/// the next phase that has not yet completed or been skipped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Quest {
    pub(crate) title: String,
    pub(crate) goal: String,
    pub(crate) phases: Vec<QuestPhase>,
    pub(crate) cursor: usize,
}

/// One scoped phase. `template` is the structured base prompt; `sub_prompt`
/// is the currently-resolved text Cast would hand to the harness. The two
/// diverge after a handoff (which appends carried context) or a manual
/// `set_phase_sub_prompt` edit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuestPhase {
    pub(crate) name: String,
    pub(crate) goal: String,
    pub(crate) harness: Option<CastHarness>,
    pub(crate) template: String,
    pub(crate) sub_prompt: String,
    pub(crate) status: QuestPhaseStatus,
    pub(crate) handoff: Option<QuestHandoff>,
    pub(crate) edited_by_user: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum QuestPhaseStatus {
    Pending,
    Running { session_id: String },
    Complete(QuestPhaseSummary),
    Skipped { reason: String },
}

/// Structured outcome of a single phase. Cast feeds this into the next
/// phase's handoff so the visible sub-prompt update stays reproducible.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuestPhaseSummary {
    pub(crate) session_id: Option<String>,
    pub(crate) exit_status: Option<String>,
    pub(crate) exit_code: Option<i32>,
    /// Bulletable facts extracted from the run that should be carried into
    /// the next sub-prompt (file paths touched, IDs minted, tests run).
    pub(crate) carried_context: Vec<String>,
}

/// What Cast tells the next phase about the prior one. `reason` is
/// rendered verbatim on the handoff card so the user can read why the
/// sub-prompt changed before approving it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuestHandoff {
    pub(crate) from_phase: String,
    pub(crate) prior_status: String,
    pub(crate) reason: String,
    pub(crate) carried_context: Vec<String>,
}

impl Quest {
    pub(crate) fn current_index(&self) -> Option<usize> {
        if self.cursor < self.phases.len() {
            Some(self.cursor)
        } else {
            None
        }
    }

    pub(crate) fn current(&self) -> Option<&QuestPhase> {
        self.current_index().map(|idx| &self.phases[idx])
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.cursor >= self.phases.len()
    }
}

const DESIGN_PHASE_TEMPLATE: &str =
    "Design the smallest viable change for the goal. Produce: a short approach summary, the file or surface boundaries you will touch, and a list of risks or open questions. Do not write code yet. Goal: {goal}.";

const IMPLEMENT_PHASE_TEMPLATE: &str =
    "Implement the change agreed in the prior design phase. Stay within the named boundaries. Run any existing tests touching the change. Goal: {goal}.";

const VERIFY_PHASE_TEMPLATE: &str =
    "Verify the implementation: re-run the touched tests, sanity-check the diff, and surface any regression or follow-up. Do not push or merge. Goal: {goal}.";

const QUEST_TITLE_CHARS: usize = 60;

/// Build a fresh quest from a free-text goal. The default template is the
/// Design → Implement → Verify rhythm that fits most repository work; each
/// phase starts pending with a concrete `sub_prompt` already composed so
/// the user can read what Cast would delegate.
pub(crate) fn quest_from_goal(goal: &str, default_harness: Option<CastHarness>) -> Quest {
    let trimmed = goal.trim();
    let title = derive_quest_title(trimmed);
    let mut quest = Quest {
        title,
        goal: trimmed.to_string(),
        phases: default_phase_set(trimmed, default_harness),
        cursor: 0,
    };
    let goal = quest.goal.clone();
    for phase in &mut quest.phases {
        phase.sub_prompt = compose_sub_prompt(phase, &goal);
    }
    quest
}

fn default_phase_set(goal: &str, harness: Option<CastHarness>) -> Vec<QuestPhase> {
    vec![
        new_phase(
            "design",
            "Scope the work",
            DESIGN_PHASE_TEMPLATE,
            goal,
            harness,
        ),
        new_phase(
            "implement",
            "Make the change",
            IMPLEMENT_PHASE_TEMPLATE,
            goal,
            harness,
        ),
        new_phase(
            "verify",
            "Confirm the change",
            VERIFY_PHASE_TEMPLATE,
            goal,
            harness,
        ),
    ]
}

fn new_phase(
    name: &str,
    role: &str,
    template: &str,
    goal: &str,
    harness: Option<CastHarness>,
) -> QuestPhase {
    QuestPhase {
        name: name.to_string(),
        goal: format!("{role} for: {goal}"),
        harness,
        template: template.to_string(),
        sub_prompt: String::new(),
        status: QuestPhaseStatus::Pending,
        handoff: None,
        edited_by_user: false,
    }
}

/// Compose `phase.sub_prompt` from its template plus any attached handoff.
/// Pure: same inputs always yield the same output, which is what makes the
/// handoff card honest.
pub(crate) fn compose_sub_prompt(phase: &QuestPhase, quest_goal: &str) -> String {
    let mut out = phase.template.replace("{goal}", quest_goal);
    if let Some(handoff) = &phase.handoff {
        out.push_str("\n\nHandoff from phase `");
        out.push_str(&handoff.from_phase);
        out.push_str("` (status `");
        out.push_str(&handoff.prior_status);
        out.push_str("`):\n- ");
        out.push_str(&handoff.reason);
        for fact in &handoff.carried_context {
            out.push_str("\n- ");
            out.push_str(fact);
        }
    }
    out
}

/// Mark the current phase complete and advance the quest. The next pending
/// phase receives a structured [`QuestHandoff`] and its `sub_prompt` is
/// recomposed deterministically. Phases the user has explicitly edited
/// (see [`set_phase_sub_prompt`]) are preserved — Cast must not silently
/// clobber a user's choice.
///
/// Returns the index of the next pending phase, or `None` when the quest
/// has no further work.
pub(crate) fn advance(quest: &mut Quest, summary: QuestPhaseSummary) -> Option<usize> {
    let current = quest.current_index()?;
    let from_name = quest.phases[current].name.clone();
    let prior_status_label = phase_status_label(&summary);
    let reason = handoff_reason(&from_name, &prior_status_label);
    let carried = summary.carried_context.clone();

    quest.phases[current].status = QuestPhaseStatus::Complete(summary);
    quest.cursor = current + 1;

    let next_index = quest.current_index()?;
    let goal = quest.goal.clone();
    let next = &mut quest.phases[next_index];
    next.handoff = Some(QuestHandoff {
        from_phase: from_name,
        prior_status: prior_status_label,
        reason,
        carried_context: carried,
    });
    if !next.edited_by_user {
        next.sub_prompt = compose_sub_prompt(next, &goal);
    }
    Some(next_index)
}

/// Override the sub-prompt for a pending phase. Marks the phase as
/// user-edited so a later [`advance`] does not silently regenerate the
/// text. Errors out if the phase is not pending — Cast does not rewrite
/// already-running or completed phases.
pub(crate) fn set_phase_sub_prompt(
    quest: &mut Quest,
    index: usize,
    sub_prompt: String,
) -> Result<()> {
    let phase = quest
        .phases
        .get_mut(index)
        .ok_or_else(|| anyhow!("quest phase index {index} out of range"))?;
    if !matches!(phase.status, QuestPhaseStatus::Pending) {
        return Err(anyhow!(
            "phase `{}` is not pending; sub-prompts can only be edited before the phase runs",
            phase.name
        ));
    }
    phase.sub_prompt = sub_prompt;
    phase.edited_by_user = true;
    Ok(())
}

/// Skip a pending phase with a recorded reason. Useful when the prior
/// phase already satisfied this phase's goal (e.g. tests passed during
/// implement, so verify becomes a no-op).
pub(crate) fn skip_phase(quest: &mut Quest, index: usize, reason: String) -> Result<()> {
    let phase = quest
        .phases
        .get_mut(index)
        .ok_or_else(|| anyhow!("quest phase index {index} out of range"))?;
    if !matches!(phase.status, QuestPhaseStatus::Pending) {
        return Err(anyhow!(
            "phase `{}` is not pending; only pending phases can be skipped",
            phase.name
        ));
    }
    phase.status = QuestPhaseStatus::Skipped { reason };
    if quest.cursor == index {
        quest.cursor = index + 1;
    }
    Ok(())
}

fn phase_status_label(summary: &QuestPhaseSummary) -> String {
    if let Some(status) = &summary.exit_status {
        match summary.exit_code {
            Some(code) => format!("{status} (exit {code})"),
            None => status.clone(),
        }
    } else if let Some(code) = summary.exit_code {
        format!("exit {code}")
    } else {
        "complete".to_string()
    }
}

fn handoff_reason(from_phase: &str, prior_status_label: &str) -> String {
    let lower = prior_status_label.to_ascii_lowercase();
    let failed = lower.starts_with("failed")
        || lower.contains("error")
        || lower.contains("exit 1")
        || lower.contains("interrupted");
    if failed {
        format!(
            "Phase `{from_phase}` finished with `{prior_status_label}` — incorporate the failure context before continuing."
        )
    } else {
        format!(
            "Phase `{from_phase}` finished with `{prior_status_label}` — carry its result into the next sub-prompt."
        )
    }
}

fn derive_quest_title(goal: &str) -> String {
    let collapsed: String = goal.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "Untitled quest".to_string();
    }
    let count = collapsed.chars().count();
    if count <= QUEST_TITLE_CHARS {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(QUEST_TITLE_CHARS - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quest(goal: &str) -> Quest {
        quest_from_goal(goal, Some(CastHarness::Codex))
    }

    #[test]
    fn quest_from_goal_uses_default_three_phase_template() {
        let q = quest("ship phase 5 sub-prompting");
        assert_eq!(q.goal, "ship phase 5 sub-prompting");
        assert_eq!(
            q.phases.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            vec!["design", "implement", "verify"]
        );
        assert_eq!(q.cursor, 0);
        assert!(!q.is_complete());
    }

    #[test]
    fn every_phase_starts_with_a_concrete_sub_prompt_containing_the_goal() {
        let q = quest("rename the legacy `cody` module to `cast`");
        for phase in &q.phases {
            assert!(
                !phase.sub_prompt.is_empty(),
                "phase `{}` must have a sub_prompt",
                phase.name
            );
            assert!(
                phase
                    .sub_prompt
                    .contains("rename the legacy `cody` module to `cast`"),
                "phase `{}` sub_prompt should include the user goal verbatim, got:\n{}",
                phase.name,
                phase.sub_prompt
            );
        }
    }

    #[test]
    fn compose_sub_prompt_substitutes_goal_and_appends_handoff() {
        let mut q = quest("fix the flaky integration test");
        q.phases[1].handoff = Some(QuestHandoff {
            from_phase: "design".to_string(),
            prior_status: "completed (exit 0)".to_string(),
            reason: "Design pinned the seam to `cast::quest`.".to_string(),
            carried_context: vec!["touched `cast/quest.rs`".to_string()],
        });
        let composed = compose_sub_prompt(&q.phases[1], &q.goal);
        assert!(composed.contains("fix the flaky integration test"));
        assert!(composed.contains("Handoff from phase `design`"));
        assert!(composed.contains("status `completed (exit 0)`"));
        assert!(composed.contains("Design pinned the seam to `cast::quest`."));
        assert!(composed.contains("touched `cast/quest.rs`"));
    }

    #[test]
    fn advance_marks_prior_complete_and_recomposes_next_sub_prompt() {
        let mut q = quest("polish the README");
        let original_implement = q.phases[1].sub_prompt.clone();

        let next = advance(
            &mut q,
            QuestPhaseSummary {
                session_id: Some("session-abc".to_string()),
                exit_status: Some("completed".to_string()),
                exit_code: Some(0),
                carried_context: vec!["proposed bullet list of edits".to_string()],
            },
        );

        assert_eq!(
            next,
            Some(1),
            "cursor should advance to the implement phase"
        );
        assert_eq!(q.cursor, 1);
        assert!(matches!(q.phases[0].status, QuestPhaseStatus::Complete(_)));
        assert!(matches!(q.phases[1].status, QuestPhaseStatus::Pending));
        // The recomposed sub-prompt carries the handoff text, so it must
        // differ from the bare template form built at quest construction.
        assert_ne!(
            q.phases[1].sub_prompt, original_implement,
            "implement sub_prompt should be refreshed with handoff context after advance"
        );
        assert!(q.phases[1]
            .sub_prompt
            .contains("proposed bullet list of edits"));
        let handoff = q.phases[1].handoff.as_ref().expect("handoff attached");
        assert_eq!(handoff.from_phase, "design");
        assert!(handoff.reason.contains("carry its result"));
    }

    #[test]
    fn advance_after_failed_phase_uses_failure_oriented_handoff_reason() {
        let mut q = quest("upgrade the rust toolchain");
        advance(
            &mut q,
            QuestPhaseSummary {
                session_id: None,
                exit_status: Some("failed".to_string()),
                exit_code: Some(1),
                carried_context: vec!["`cargo build` exited 1 on `coven-cli`".to_string()],
            },
        );
        let handoff = q.phases[1].handoff.as_ref().expect("handoff attached");
        assert!(
            handoff.reason.contains("incorporate the failure context"),
            "failed phase should produce a failure-flavoured reason, got: {}",
            handoff.reason
        );
        assert!(q.phases[1].sub_prompt.contains("`cargo build` exited 1"));
    }

    #[test]
    fn set_phase_sub_prompt_overrides_and_survives_subsequent_advance() {
        let mut q = quest("rotate the daemon socket path");
        set_phase_sub_prompt(
            &mut q,
            1,
            "Move the socket to `$XDG_RUNTIME_DIR/coven.sock` and update the lockfile.".to_string(),
        )
        .unwrap();
        assert!(q.phases[1].edited_by_user);

        advance(
            &mut q,
            QuestPhaseSummary {
                session_id: None,
                exit_status: Some("completed".to_string()),
                exit_code: Some(0),
                carried_context: vec!["socket location decided".to_string()],
            },
        );

        // The handoff is still attached so the user can read it, but the
        // sub_prompt text the user authored is preserved verbatim.
        assert!(q.phases[1].handoff.is_some(), "handoff should still attach");
        assert_eq!(
            q.phases[1].sub_prompt,
            "Move the socket to `$XDG_RUNTIME_DIR/coven.sock` and update the lockfile.",
            "user-authored sub_prompt must not be clobbered by advance"
        );
    }

    #[test]
    fn set_phase_sub_prompt_rejects_non_pending_phases() {
        let mut q = quest("anything");
        q.phases[0].status = QuestPhaseStatus::Running {
            session_id: "session-1".to_string(),
        };
        let err = set_phase_sub_prompt(&mut q, 0, "ignored".to_string()).unwrap_err();
        assert!(err.to_string().contains("not pending"));
    }

    #[test]
    fn skip_phase_advances_cursor_and_marks_status() {
        let mut q = quest("publish a release");
        skip_phase(&mut q, 2, "verify happens in CI".to_string()).unwrap();
        assert!(matches!(
            q.phases[2].status,
            QuestPhaseStatus::Skipped { .. }
        ));

        advance(
            &mut q,
            QuestPhaseSummary {
                exit_status: Some("completed".to_string()),
                exit_code: Some(0),
                ..QuestPhaseSummary::default()
            },
        );
        // After design completes, cursor moves to implement (1).
        assert_eq!(q.cursor, 1);
        advance(
            &mut q,
            QuestPhaseSummary {
                exit_status: Some("completed".to_string()),
                exit_code: Some(0),
                ..QuestPhaseSummary::default()
            },
        );
        // Implement completes; cursor lands on the skipped verify (2). The
        // next `current()` call shows verify as skipped so the shell can
        // jump past it without re-prompting.
        assert_eq!(q.cursor, 2);
        let current = q.current().expect("verify exists");
        assert!(matches!(current.status, QuestPhaseStatus::Skipped { .. }));
    }

    #[test]
    fn advance_returns_none_after_the_last_phase() {
        let mut q = quest("teach Cast to whistle");
        let r1 = advance(&mut q, QuestPhaseSummary::default());
        let r2 = advance(&mut q, QuestPhaseSummary::default());
        let r3 = advance(&mut q, QuestPhaseSummary::default());
        let r4 = advance(&mut q, QuestPhaseSummary::default());
        assert_eq!((r1, r2, r3, r4), (Some(1), Some(2), None, None));
        assert!(q.is_complete());
        assert!(q.current().is_none());
    }

    #[test]
    fn handoff_status_label_falls_back_when_exit_code_only() {
        let label = phase_status_label(&QuestPhaseSummary {
            exit_status: None,
            exit_code: Some(2),
            ..QuestPhaseSummary::default()
        });
        assert_eq!(label, "exit 2");

        let label = phase_status_label(&QuestPhaseSummary::default());
        assert_eq!(label, "complete");
    }

    #[test]
    fn quest_title_truncates_very_long_goals() {
        let goal = "do every single conceivable thing across the whole repository in one go please";
        let q = quest_from_goal(goal, None);
        assert!(q.title.chars().count() <= QUEST_TITLE_CHARS);
        assert!(q.title.ends_with('…'));
    }
}
