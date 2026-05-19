//! Cast safety gate.
//!
//! Phase 1 keeps risk classification deterministic and local: a small token
//! list flags spells that would push, publish, merge, release, or broadly
//! delete. The classifier never executes anything — it only labels intents
//! so the planner can decide whether to proceed, confirm, or reject. The
//! daemon remains the authority for any actual side effect.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CastRisk {
    /// Safe local read, plan, or harness work.
    Safe,
    /// Requires explicit confirmation before the daemon side effect.
    Confirm,
    /// Cast refuses to plan this in phase 1; suggest a safer alternative.
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SafetyDecision {
    Proceed,
    Confirm { reason: String, suggestion: String },
    Reject { reason: String, alternative: String },
}

impl SafetyDecision {
    pub(crate) fn risk(&self) -> CastRisk {
        match self {
            Self::Proceed => CastRisk::Safe,
            Self::Confirm { .. } => CastRisk::Confirm,
            Self::Reject { .. } => CastRisk::Reject,
        }
    }
}

/// Classify the risk of a free-text spell that will be routed to a harness.
///
/// Phase 1 is deliberately conservative: it only looks at the literal text the
/// user typed, not at what the harness will eventually do. The job of this
/// classifier is to surface the *user's stated intent* so the Cast frame can
/// warn the user before launch. The harness, the project-root guard, and the
/// daemon still own the real safety checks.
pub(crate) fn classify_prompt_risk(prompt: &str) -> SafetyDecision {
    let normalized = prompt.to_ascii_lowercase();

    if mentions_any(&normalized, REJECT_TOKENS) {
        return SafetyDecision::Reject {
            reason: "spell asks for a broadly destructive shell command".to_string(),
            alternative:
                "describe the outcome instead (for example, \"remove the unused crate\") and \
                 let Cast plan a scoped change you can review."
                    .to_string(),
        };
    }

    if mentions_any(&normalized, PUBLISH_TOKENS) {
        return SafetyDecision::Confirm {
            reason: "spell mentions publishing, pushing, merging, or releasing".to_string(),
            suggestion: "Cast will route this to the harness, but you should review the diff and \
                 confirm before any push/merge/release lands."
                .to_string(),
        };
    }

    if mentions_any(&normalized, DESTRUCTIVE_TOKENS) {
        return SafetyDecision::Confirm {
            reason: "spell mentions a destructive file or repo operation".to_string(),
            suggestion:
                "Cast will pass this to the harness, but please double-check what it changes \
                 before approving any irreversible step."
                    .to_string(),
        };
    }

    SafetyDecision::Proceed
}

const REJECT_TOKENS: &[&str] = &[
    "rm -rf /",
    "rm -rf ~",
    "rm -rf *",
    "rm /*",
    ":(){:|:&};:",
    "format the disk",
    "wipe the disk",
    "drop database",
];

const PUBLISH_TOKENS: &[&str] = &[
    "git push",
    "force push",
    "force-push",
    "push to main",
    "push to master",
    "publish ",
    "publish to",
    "npm publish",
    "cargo publish",
    "release ",
    "cut a release",
    "merge to main",
    "merge into main",
    "merge to master",
    "post to slack",
    "send to slack",
    "send email",
    "tweet ",
];

const DESTRUCTIVE_TOKENS: &[&str] = &[
    "rm -rf",
    "delete the repo",
    "delete this repo",
    "wipe ",
    "purge ",
    "sacrifice ",
    "drop table",
];

fn mentions_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_prompt_proceeds() {
        assert_eq!(
            classify_prompt_risk("fix the failing tests"),
            SafetyDecision::Proceed
        );
        assert_eq!(
            classify_prompt_risk("explain this repo in 5 bullets"),
            SafetyDecision::Proceed
        );
    }

    #[test]
    fn publish_words_require_confirmation() {
        match classify_prompt_risk("git push the changes to main") {
            SafetyDecision::Confirm { .. } => {}
            other => panic!("expected confirm, got {other:?}"),
        }
        match classify_prompt_risk("publish the new crate to crates.io") {
            SafetyDecision::Confirm { .. } => {}
            other => panic!("expected confirm, got {other:?}"),
        }
        match classify_prompt_risk("merge to main once tests pass") {
            SafetyDecision::Confirm { .. } => {}
            other => panic!("expected confirm, got {other:?}"),
        }
    }

    #[test]
    fn destructive_words_require_confirmation() {
        match classify_prompt_risk("rm -rf build/") {
            SafetyDecision::Confirm { .. } => {}
            other => panic!("expected confirm, got {other:?}"),
        }
        match classify_prompt_risk("purge the cache directory") {
            SafetyDecision::Confirm { .. } => {}
            other => panic!("expected confirm, got {other:?}"),
        }
    }

    #[test]
    fn obviously_dangerous_shell_is_rejected() {
        match classify_prompt_risk("rm -rf / now") {
            SafetyDecision::Reject { .. } => {}
            other => panic!("expected reject, got {other:?}"),
        }
        match classify_prompt_risk("drop database production") {
            SafetyDecision::Reject { .. } => {}
            other => panic!("expected reject, got {other:?}"),
        }
    }

    #[test]
    fn case_insensitivity() {
        match classify_prompt_risk("Git Push To Main") {
            SafetyDecision::Confirm { .. } => {}
            other => panic!("expected confirm, got {other:?}"),
        }
    }

    #[test]
    fn safety_decision_reports_matching_risk() {
        assert_eq!(SafetyDecision::Proceed.risk(), CastRisk::Safe);
        assert_eq!(
            SafetyDecision::Confirm {
                reason: String::new(),
                suggestion: String::new(),
            }
            .risk(),
            CastRisk::Confirm
        );
        assert_eq!(
            SafetyDecision::Reject {
                reason: String::new(),
                alternative: String::new(),
            }
            .risk(),
            CastRisk::Reject
        );
    }
}
