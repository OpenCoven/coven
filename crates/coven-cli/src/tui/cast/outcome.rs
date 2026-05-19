//! Cast outcome model.
//!
//! Every dispatched plan produces a `CastOutcome` that the renderer turns
//! into the post-run card the user sees. Outcomes are intentionally small:
//! Cast's job is to point the user at the durable evidence (session id,
//! daemon, ledger) instead of inventing parallel state.

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub(crate) struct CastOutcome {
    /// User-facing request text shown in the outcome card (often normalized).
    pub(crate) request: String,
    /// One-line summary of what Cast launched, if anything.
    pub(crate) launched: Option<String>,
    /// Session id Cast created or attached to, if any.
    pub(crate) session_id: Option<String>,
    /// Concrete next thing the user can do — typed as a copy/pastable command.
    pub(crate) next_step: Option<String>,
    /// Free-form notes Cast wants to surface (risk, verification, follow-ups).
    pub(crate) notes: Vec<String>,
}

impl CastOutcome {
    pub(crate) fn for_request(request: impl Into<String>) -> Self {
        Self {
            request: request.into(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_request_sets_request_and_clears_other_fields() {
        let outcome = CastOutcome::for_request("fix the failing tests");
        assert_eq!(outcome.request, "fix the failing tests");
        assert!(outcome.launched.is_none());
        assert!(outcome.session_id.is_none());
        assert!(outcome.next_step.is_none());
        assert!(outcome.notes.is_empty());
    }
}
