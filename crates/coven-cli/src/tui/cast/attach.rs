//! Cast attach helpers.
//!
//! Phase 2 lets the user re-enter a previously-launched session through the
//! same follower the launch path uses. The pure helpers in this module decode
//! the `cast.summary` event Cast wrote at the end of the original run so the
//! attach outcome card can describe what already happened.

use serde_json::Value;

use crate::store;

/// Event kind Cast writes when a launched session finishes. See
/// `shell::write_cast_summary_event` for the producer side.
pub(crate) const CAST_SUMMARY_KIND: &str = "cast.summary";

/// Decoded `cast.summary` event. All fields are optional because Cast may
/// have written a partial payload (e.g. an older Cast that didn't record
/// `headline`), and the renderer should degrade gracefully.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CastAttachSummary {
    pub(crate) request: Option<String>,
    pub(crate) headline: Option<String>,
    pub(crate) harness: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) exit_code: Option<i32>,
}

/// Return the most recent `cast.summary` event from `events`, decoded. Cast
/// only writes one summary per session today, but the helper returns the
/// last one so re-runs (which append) remain readable.
pub(crate) fn find_cast_summary(events: &[store::EventRecord]) -> Option<CastAttachSummary> {
    events
        .iter()
        .rev()
        .find(|event| event.kind == CAST_SUMMARY_KIND)
        .map(decode_summary)
}

/// One-line outcome-card note describing what Cast saw on the prior run.
/// Returns `None` when none of the fields are populated, so the caller can
/// skip the note entirely instead of printing an empty bullet.
pub(crate) fn format_summary_note(summary: &CastAttachSummary) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(status) = &summary.status {
        match summary.exit_code {
            Some(code) => parts.push(format!("status `{status}` (exit code {code})")),
            None => parts.push(format!("status `{status}`")),
        }
    } else if let Some(code) = summary.exit_code {
        parts.push(format!("exit code {code}"));
    }
    if let Some(harness) = &summary.harness {
        parts.push(format!("harness {harness}"));
    }
    if let Some(request) = summary.request.as_ref().or(summary.headline.as_ref()) {
        let trimmed = first_chars(request, 60);
        parts.push(format!("request `{trimmed}`"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!("Prior Cast summary: {}.", parts.join(", ")))
    }
}

fn decode_summary(event: &store::EventRecord) -> CastAttachSummary {
    let payload = match serde_json::from_str::<Value>(&event.payload_json) {
        Ok(value) => value,
        Err(_) => return CastAttachSummary::default(),
    };
    CastAttachSummary {
        request: payload
            .get("request")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        headline: payload
            .get("headline")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        harness: payload
            .get("harness")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        status: payload
            .get("status")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        exit_code: payload
            .get("exitCode")
            .and_then(Value::as_i64)
            .map(|v| v as i32)
            .or_else(|| {
                payload
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .map(|v| v as i32)
            }),
    }
}

fn first_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_string();
    }
    let mut out: String = value.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary_event(seq: i64, payload: serde_json::Value) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "session-1".to_string(),
            kind: CAST_SUMMARY_KIND.to_string(),
            payload_json: payload.to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
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

    #[test]
    fn find_cast_summary_returns_none_when_no_summary_event_exists() {
        let events = vec![output_event(1, "hello\n"), output_event(2, "bye\n")];
        assert!(find_cast_summary(&events).is_none());
    }

    #[test]
    fn find_cast_summary_decodes_request_status_and_exit_code() {
        let events = vec![
            output_event(1, "working\n"),
            summary_event(
                2,
                serde_json::json!({
                    "request": "fix the failing tests",
                    "headline": "Cast a project-scoped spell",
                    "harness": "codex",
                    "status": "completed",
                    "exitCode": 0,
                }),
            ),
        ];

        let summary = find_cast_summary(&events).expect("summary should be decoded");
        assert_eq!(summary.request.as_deref(), Some("fix the failing tests"));
        assert_eq!(summary.harness.as_deref(), Some("codex"));
        assert_eq!(summary.status.as_deref(), Some("completed"));
        assert_eq!(summary.exit_code, Some(0));
    }

    #[test]
    fn find_cast_summary_accepts_snake_case_exit_code() {
        let events = vec![summary_event(
            1,
            serde_json::json!({ "status": "failed", "exit_code": 137 }),
        )];

        let summary = find_cast_summary(&events).expect("summary should be decoded");
        assert_eq!(summary.exit_code, Some(137));
    }

    #[test]
    fn find_cast_summary_returns_most_recent_when_multiple_exist() {
        let events = vec![
            summary_event(1, serde_json::json!({ "status": "failed", "exitCode": 1 })),
            output_event(2, "retrying\n"),
            summary_event(
                3,
                serde_json::json!({ "status": "completed", "exitCode": 0 }),
            ),
        ];

        let summary = find_cast_summary(&events).expect("summary should be decoded");
        assert_eq!(summary.status.as_deref(), Some("completed"));
        assert_eq!(summary.exit_code, Some(0));
    }

    #[test]
    fn find_cast_summary_yields_default_summary_for_malformed_payload() {
        let mut event = summary_event(1, serde_json::json!({}));
        event.payload_json = "not json".to_string();
        let events = vec![event];

        let summary = find_cast_summary(&events).expect("summary should still be returned");
        assert_eq!(summary, CastAttachSummary::default());
    }

    #[test]
    fn format_summary_note_returns_none_for_empty_summary() {
        assert_eq!(
            format_summary_note(&CastAttachSummary::default()),
            None,
            "an empty summary should yield no note"
        );
    }

    #[test]
    fn format_summary_note_shows_status_exit_code_harness_and_request() {
        let summary = CastAttachSummary {
            request: Some("fix the failing tests".to_string()),
            headline: Some("Cast a project-scoped spell".to_string()),
            harness: Some("codex".to_string()),
            status: Some("completed".to_string()),
            exit_code: Some(0),
        };

        let note = format_summary_note(&summary).expect("note should be produced");
        assert!(note.contains("Prior Cast summary"));
        assert!(note.contains("status `completed`"));
        assert!(note.contains("exit code 0"));
        assert!(note.contains("harness codex"));
        assert!(note.contains("request `fix the failing tests`"));
    }

    #[test]
    fn format_summary_note_falls_back_to_headline_when_request_missing() {
        let summary = CastAttachSummary {
            request: None,
            headline: Some("Cast a project-scoped spell".to_string()),
            ..Default::default()
        };

        let note = format_summary_note(&summary).expect("note should be produced");
        assert!(note.contains("request `Cast a project-scoped spell`"));
    }

    #[test]
    fn format_summary_note_truncates_long_request_with_ellipsis() {
        let long_request = "a".repeat(120);
        let summary = CastAttachSummary {
            request: Some(long_request),
            ..Default::default()
        };

        let note = format_summary_note(&summary).expect("note should be produced");
        assert!(
            note.contains('…'),
            "long request should be truncated with ellipsis: {note}"
        );
    }

    #[test]
    fn format_summary_note_handles_status_without_exit_code() {
        let summary = CastAttachSummary {
            status: Some("interrupted".to_string()),
            ..Default::default()
        };

        let note = format_summary_note(&summary).expect("note should be produced");
        assert!(note.contains("status `interrupted`"));
        assert!(!note.contains("exit code"));
    }
}
