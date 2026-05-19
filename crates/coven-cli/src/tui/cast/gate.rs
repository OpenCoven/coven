//! Cast confirmation gate.
//!
//! The gate sits between the plan and the dispatcher. It takes a typed
//! `CastPlan` and an injectable reader closure, decides whether the plan
//! should proceed, and returns a typed `GateOutcome`. The reader closure is
//! the only part of the gate that touches stdin; tests pass a canned reader
//! so the y/N and typed-word flows can be exercised without a TTY.
//!
//! Phase 1.5 gates:
//! - `CastRisk::Safe` → `Proceed`
//! - `CastRisk::Confirm` → y/N prompt (or `sacrifice` typed-confirm when the
//!   intent is `SacrificeSession`)
//! - `CastRisk::Reject` → `Cancelled` with the alternative the classifier
//!   suggested
//!
//! The dispatcher never calls a destructive handler until the gate returns
//! `Proceed`, so confirmation is enforced for every Confirm-risk plan, not
//! just plans the dispatcher happens to remember to check.

use anyhow::Result;

use super::intent::CastIntent;
use super::plan::CastPlan;
use super::safety::{CastRisk, SafetyDecision};

const SACRIFICE_CONFIRM_WORD: &str = "sacrifice";

/// What the gate decided about the plan. The dispatcher branches on this
/// before any side effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GateOutcome {
    Proceed,
    Cancelled {
        reason: String,
        next_step: Option<String>,
    },
}

/// Evaluate a plan's safety gate. The `reader` closure is invoked whenever
/// the gate needs interactive confirmation; it returns the user's typed
/// response (already trimmed). Tests pass canned readers.
pub(crate) fn evaluate_gate<F>(plan: &CastPlan, reader: &mut F) -> Result<GateOutcome>
where
    F: FnMut(&str) -> Result<String>,
{
    match plan.risk() {
        CastRisk::Safe => Ok(GateOutcome::Proceed),
        CastRisk::Reject => Ok(rejected_outcome(plan)),
        CastRisk::Confirm => {
            if matches!(plan.intent, CastIntent::SacrificeSession { .. }) {
                run_typed_confirm(plan, reader, SACRIFICE_CONFIRM_WORD)
            } else {
                run_yes_no_confirm(plan, reader)
            }
        }
    }
}

fn rejected_outcome(plan: &CastPlan) -> GateOutcome {
    match &plan.decision {
        SafetyDecision::Reject {
            reason,
            alternative,
        } => GateOutcome::Cancelled {
            reason: format!("Rejected: {reason}"),
            next_step: Some(alternative.clone()),
        },
        _ => GateOutcome::Cancelled {
            reason: "Rejected by Cast.".to_string(),
            next_step: None,
        },
    }
}

fn run_yes_no_confirm<F>(plan: &CastPlan, reader: &mut F) -> Result<GateOutcome>
where
    F: FnMut(&str) -> Result<String>,
{
    let (reason, suggestion) = match &plan.decision {
        SafetyDecision::Confirm { reason, suggestion } => (reason.as_str(), suggestion.as_str()),
        _ => ("Cast wants your confirmation.", ""),
    };
    let prompt = build_yes_no_prompt(reason, suggestion);
    let answer = reader(&prompt)?;
    if is_yes(&answer) {
        Ok(GateOutcome::Proceed)
    } else {
        Ok(GateOutcome::Cancelled {
            reason: "spell cancelled at the Cast confirm step".to_string(),
            next_step: None,
        })
    }
}

fn run_typed_confirm<F>(plan: &CastPlan, reader: &mut F, word: &str) -> Result<GateOutcome>
where
    F: FnMut(&str) -> Result<String>,
{
    let session_id_short = plan
        .session_id
        .as_deref()
        .map(short_id)
        .unwrap_or_else(|| "<unknown>".to_string());
    let prompt = format!(
        "Cast: this will permanently delete session `{session_id_short}` and its events.\n\
         Type `{word}` to confirm (anything else cancels): "
    );
    let answer = reader(&prompt)?;
    if answer.trim() == word {
        Ok(GateOutcome::Proceed)
    } else {
        Ok(GateOutcome::Cancelled {
            reason: format!("sacrifice cancelled: confirmation word `{word}` was not typed"),
            next_step: None,
        })
    }
}

fn build_yes_no_prompt(reason: &str, suggestion: &str) -> String {
    if suggestion.is_empty() {
        format!("Cast: {reason}.\nProceed? [y/N] ")
    } else {
        format!("Cast: {reason}.\n{suggestion}\nProceed? [y/N] ")
    }
}

fn is_yes(input: &str) -> bool {
    matches!(input.trim(), "y" | "Y" | "yes" | "YES" | "Yes")
}

fn short_id(session_id: &str) -> String {
    session_id.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::cast::intent::CastHarness;
    use crate::tui::cast::plan::build_plan;

    fn always_codex() -> Option<CastHarness> {
        Some(CastHarness::Codex)
    }

    /// A reader that returns its canned answers in order. Panics if asked
    /// more times than answers were provided so tests fail loudly on extra
    /// prompts.
    struct ScriptedReader {
        answers: Vec<String>,
        prompts_seen: Vec<String>,
    }

    impl ScriptedReader {
        fn new(answers: &[&str]) -> Self {
            Self {
                answers: answers.iter().map(|s| (*s).to_string()).collect(),
                prompts_seen: Vec::new(),
            }
        }

        fn reader(&mut self) -> impl FnMut(&str) -> Result<String> + '_ {
            |prompt: &str| {
                self.prompts_seen.push(prompt.to_string());
                if self.answers.is_empty() {
                    panic!("ScriptedReader exhausted; unexpected prompt: {prompt:?}");
                }
                Ok(self.answers.remove(0))
            }
        }
    }

    fn natural_plan(prompt: &str) -> CastPlan {
        build_plan(
            CastIntent::NaturalSpell {
                prompt: prompt.to_string(),
            },
            always_codex,
        )
        .unwrap()
    }

    fn sacrifice_plan(session_id: &str) -> CastPlan {
        build_plan(
            CastIntent::SacrificeSession {
                session_id: session_id.to_string(),
            },
            always_codex,
        )
        .unwrap()
    }

    #[test]
    fn safe_plan_proceeds_without_reader_use() {
        let plan = natural_plan("fix the failing tests");
        let mut scripted = ScriptedReader::new(&[]);

        let outcome = evaluate_gate(&plan, &mut scripted.reader()).unwrap();

        assert_eq!(outcome, GateOutcome::Proceed);
        assert!(
            scripted.prompts_seen.is_empty(),
            "safe plan must not prompt"
        );
    }

    #[test]
    fn confirm_risk_proceeds_when_user_says_yes() {
        let plan = natural_plan("git push the changes to main");
        let mut scripted = ScriptedReader::new(&["y"]);

        let outcome = evaluate_gate(&plan, &mut scripted.reader()).unwrap();

        assert_eq!(outcome, GateOutcome::Proceed);
        assert_eq!(scripted.prompts_seen.len(), 1);
        assert!(scripted.prompts_seen[0].contains("Proceed? [y/N]"));
        assert!(
            scripted.prompts_seen[0].contains("publishing, pushing"),
            "yes/no prompt should include the risk reason: {:?}",
            scripted.prompts_seen[0]
        );
    }

    #[test]
    fn confirm_risk_cancels_on_empty_or_no_answer() {
        for answer in &["", "n", "no", "NO", "anything else"] {
            let plan = natural_plan("git push the changes to main");
            let mut scripted = ScriptedReader::new(&[answer]);

            let outcome = evaluate_gate(&plan, &mut scripted.reader()).unwrap();

            match outcome {
                GateOutcome::Cancelled { reason, .. } => {
                    assert!(
                        reason.contains("cancelled at the Cast confirm step"),
                        "unexpected reason for answer `{answer}`: {reason}"
                    );
                }
                GateOutcome::Proceed => {
                    panic!("answer `{answer}` should not proceed");
                }
            }
        }
    }

    #[test]
    fn reject_risk_never_prompts_and_returns_alternative() {
        let plan = natural_plan("rm -rf / now");
        let mut scripted = ScriptedReader::new(&[]);

        let outcome = evaluate_gate(&plan, &mut scripted.reader()).unwrap();

        match outcome {
            GateOutcome::Cancelled { reason, next_step } => {
                assert!(reason.starts_with("Rejected:"));
                assert!(next_step.is_some());
            }
            GateOutcome::Proceed => panic!("rejected plan must not proceed"),
        }
        assert!(
            scripted.prompts_seen.is_empty(),
            "reject must not prompt the user"
        );
    }

    #[test]
    fn sacrifice_requires_typed_word_not_yes() {
        let plan = sacrifice_plan("abcdef123456");
        let mut scripted = ScriptedReader::new(&["y"]);

        let outcome = evaluate_gate(&plan, &mut scripted.reader()).unwrap();

        match outcome {
            GateOutcome::Cancelled { reason, .. } => {
                assert!(reason.contains("sacrifice"));
                assert!(reason.contains("not typed"));
            }
            GateOutcome::Proceed => panic!("y/N must not satisfy sacrifice gate"),
        }
        assert_eq!(scripted.prompts_seen.len(), 1);
        assert!(
            scripted.prompts_seen[0].contains("Type `sacrifice`"),
            "sacrifice prompt should describe the typed-word requirement: {:?}",
            scripted.prompts_seen[0]
        );
        assert!(
            scripted.prompts_seen[0].contains("abcdef123456"),
            "sacrifice prompt should include the session id"
        );
    }

    #[test]
    fn sacrifice_proceeds_when_typed_word_matches() {
        let plan = sacrifice_plan("abcdef123456");
        let mut scripted = ScriptedReader::new(&["sacrifice"]);

        let outcome = evaluate_gate(&plan, &mut scripted.reader()).unwrap();

        assert_eq!(outcome, GateOutcome::Proceed);
    }

    #[test]
    fn sacrifice_typed_word_is_case_sensitive() {
        let plan = sacrifice_plan("abcdef123456");
        let mut scripted = ScriptedReader::new(&["Sacrifice"]);

        let outcome = evaluate_gate(&plan, &mut scripted.reader()).unwrap();

        assert!(matches!(outcome, GateOutcome::Cancelled { .. }));
    }

    #[test]
    fn yes_no_helper_accepts_common_yes_spellings() {
        for value in &["y", "Y", "yes", "YES", "Yes"] {
            assert!(is_yes(value), "`{value}` should count as yes");
        }
        for value in &["", " ", "n", "no", "NO", "maybe", "y!"] {
            assert!(!is_yes(value), "`{value}` should not count as yes");
        }
    }
}
