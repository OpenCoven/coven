use async_trait::async_trait;

use crate::BoxError;

/// The decision a [`ProposalReview`] returns for one resolved tool call.
///
/// Only [`ReviewVerdict::Permit`] lets the runner execute the tool. Every other
/// verdict, including [`ReviewVerdict::Unavailable`], keeps the tool from
/// running: the runner records the proposal and the verdict in the transcript
/// and the run continues without the tool's side effect. There is no default
/// permit.
///
/// Verdicts are evidence, not authorization. A `Permit` records that a named
/// reviewer raised no objection to the proposal it was shown; it does not
/// grant the tool any capability it did not already have, and it does not
/// stand in for an operator approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewVerdict {
    /// The reviewer raised no objection; the runner executes the tool.
    Permit,
    /// The call is recorded as a proposal only and is not executed.
    ProposalOnly { reason: String },
    /// The call is refused and is not executed.
    Reject { reason: String },
    /// The reviewer could not reach a decision; the call is not executed.
    ///
    /// The runner also produces this verdict itself when a reviewer returns an
    /// error, so a broken or unreachable reviewer fails closed.
    Unavailable { reason: String },
}

impl ReviewVerdict {
    pub fn proposal_only(reason: impl Into<String>) -> Self {
        Self::ProposalOnly {
            reason: reason.into(),
        }
    }

    pub fn reject(reason: impl Into<String>) -> Self {
        Self::Reject {
            reason: reason.into(),
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    /// The payload-free shape of this verdict, suitable for observers.
    pub const fn outcome(&self) -> ReviewOutcome {
        match self {
            Self::Permit => ReviewOutcome::Permit,
            Self::ProposalOnly { .. } => ReviewOutcome::ProposalOnly,
            Self::Reject { .. } => ReviewOutcome::Reject,
            Self::Unavailable { .. } => ReviewOutcome::Unavailable,
        }
    }

    /// The reviewer's stated reason, if the verdict carries one.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Permit => None,
            Self::ProposalOnly { reason }
            | Self::Reject { reason }
            | Self::Unavailable { reason } => Some(reason),
        }
    }
}

/// The payload-free kind of a [`ReviewVerdict`].
///
/// Observers receive this instead of the verdict so that reviewer reasons,
/// like tool arguments and outputs, stay out of the metadata event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewOutcome {
    Permit,
    ProposalOnly,
    Reject,
    Unavailable,
}

impl ReviewOutcome {
    /// Stable lower-case label used in transcript tool results.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permit => "permit",
            Self::ProposalOnly => "proposal_only",
            Self::Reject => "reject",
            Self::Unavailable => "unavailable",
        }
    }
}

impl std::fmt::Display for ReviewOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A resolved tool call as the model proposed it, before dispatch.
///
/// The runner has already matched `tool` to a registered tool on the current
/// agent; `arguments` are exactly what the model sent, unmodified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolProposal<'a> {
    pub call_id: &'a str,
    pub tool: &'a str,
    pub arguments: &'a serde_json::Value,
}

#[async_trait]
/// Reviews a resolved tool call before the runner dispatches it.
///
/// Reviewers are registered per agent and consulted in registration order
/// before every tool execution by that agent, including a handoff target's own
/// tool calls after control transfers to it. The first non-`Permit` verdict
/// wins and later reviewers are not consulted.
///
/// The seam fails closed: a reviewer that returns `Err` is treated as
/// [`ReviewVerdict::Unavailable`] and the tool does not run. The run itself
/// continues; the model sees a tool result stating that the call was not
/// executed and why.
///
/// This crate ships the seam only. It carries no HTTP client or hosted review
/// transport; a live reviewer is an adapter that implements this trait.
pub trait ProposalReview<C>: Send + Sync
where
    C: Sync,
{
    fn name(&self) -> &str;

    async fn review(
        &self,
        proposal: &ToolProposal<'_>,
        context: &C,
    ) -> Result<ReviewVerdict, BoxError>;
}
