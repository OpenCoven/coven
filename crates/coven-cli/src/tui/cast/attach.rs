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

/// Event kind Cast writes on the anchor session when a quest begins. See
/// `shell::dispatch_cast_quest` for the producer side.
pub(crate) const CAST_QUEST_STARTED_KIND: &str = "cast.quest.started";

/// Event kind Cast writes on the anchor session when a quest's last phase
/// has finished.
pub(crate) const CAST_QUEST_COMPLETED_KIND: &str = "cast.quest.completed";

/// Event kind Cast writes on the anchor session right after each phase
/// finishes. Used by the re-attach detector to compute the phase index.
pub(crate) const CAST_QUEST_PHASE_COMPLETED_KIND: &str = "cast.quest.phase_completed";

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

/// Decoded `cast.quest.started` event plus the count of
/// `cast.quest.phase_completed` events seen on the same session. Both come
/// from the *anchor* session of a quest (per the Phase 7 first-session
/// anchor decision).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CastQuestAttachInfo {
    pub(crate) title: Option<String>,
    pub(crate) goal: Option<String>,
    pub(crate) harness: Option<String>,
    pub(crate) total_phases: Option<usize>,
    pub(crate) completed_phases: usize,
    pub(crate) is_complete: bool,
}

/// Detect that `events` belong to a quest's anchor session. Returns
/// `None` when no `cast.quest.started` event is present (i.e., the session
/// is a plain Cast launch, not a quest anchor). Currently used as a
/// minimal re-attach aid: callers print a note pointing the user at the
/// quest title and progress so they can re-run `/quest <goal>` if they
/// want to continue. Full state rebuild (replay handoffs, render the next
/// card) is deferred to a later phase.
pub(crate) fn find_cast_quest_info(events: &[store::EventRecord]) -> Option<CastQuestAttachInfo> {
    let started = events
        .iter()
        .rev()
        .find(|event| event.kind == CAST_QUEST_STARTED_KIND)?;
    let payload = serde_json::from_str::<Value>(&started.payload_json).unwrap_or(Value::Null);
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let goal = payload
        .get("goal")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let harness = payload
        .get("harness")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let total_phases = payload
        .get("phases")
        .and_then(Value::as_array)
        .map(|arr| arr.len());
    let completed_phases = events
        .iter()
        .filter(|event| event.kind == CAST_QUEST_PHASE_COMPLETED_KIND)
        .count();
    let is_complete = events
        .iter()
        .any(|event| event.kind == CAST_QUEST_COMPLETED_KIND);
    Some(CastQuestAttachInfo {
        title,
        goal,
        harness,
        total_phases,
        completed_phases,
        is_complete,
    })
}

/// One-line note for the attach outcome card describing the quest this
/// session anchors. Returns `None` when the info carries no usable text
/// (defensive — `find_cast_quest_info` already short-circuits on the
/// happy path).
pub(crate) fn format_quest_attach_note(info: &CastQuestAttachInfo) -> Option<String> {
    let title = info.title.as_deref().unwrap_or("(untitled quest)");
    let progress = match info.total_phases {
        Some(total) if total > 0 => format!("phase {}/{total}", info.completed_phases.min(total)),
        _ => format!("{} phases run", info.completed_phases),
    };
    let state = if info.is_complete {
        "complete"
    } else if info.completed_phases == 0 {
        "starting"
    } else {
        "in progress"
    };
    Some(format!(
        "Quest anchor — `{title}` ({progress}, {state}). Re-run `/quest <goal>` to continue."
    ))
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

    fn quest_started_event(seq: i64, payload: serde_json::Value) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "session-1".to_string(),
            kind: CAST_QUEST_STARTED_KIND.to_string(),
            payload_json: payload.to_string(),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        }
    }

    fn quest_kind_event(seq: i64, kind: &str) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "session-1".to_string(),
            kind: kind.to_string(),
            payload_json: "{}".to_string(),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn find_cast_quest_info_returns_none_when_no_quest_event_exists() {
        let events = vec![output_event(1, "hello\n")];
        assert!(find_cast_quest_info(&events).is_none());
    }

    #[test]
    fn find_cast_quest_info_decodes_title_goal_harness_and_phase_count() {
        let events = vec![quest_started_event(
            1,
            serde_json::json!({
                "title": "Ship phase 7",
                "goal": "ship phase 7",
                "harness": "codex",
                "phases": ["design", "implement", "verify"],
            }),
        )];
        let info = find_cast_quest_info(&events).expect("quest info should be present");
        assert_eq!(info.title.as_deref(), Some("Ship phase 7"));
        assert_eq!(info.goal.as_deref(), Some("ship phase 7"));
        assert_eq!(info.harness.as_deref(), Some("codex"));
        assert_eq!(info.total_phases, Some(3));
        assert_eq!(info.completed_phases, 0);
        assert!(!info.is_complete);
    }

    #[test]
    fn find_cast_quest_info_counts_phase_completed_events() {
        let events = vec![
            quest_started_event(
                1,
                serde_json::json!({
                    "title": "Ship phase 7",
                    "phases": ["design", "implement", "verify"],
                }),
            ),
            quest_kind_event(2, CAST_QUEST_PHASE_COMPLETED_KIND),
            quest_kind_event(3, CAST_QUEST_PHASE_COMPLETED_KIND),
        ];
        let info = find_cast_quest_info(&events).expect("quest info should be present");
        assert_eq!(info.completed_phases, 2);
        assert!(!info.is_complete);
    }

    #[test]
    fn find_cast_quest_info_marks_complete_when_completed_event_is_present() {
        let events = vec![
            quest_started_event(1, serde_json::json!({ "title": "X", "phases": ["a"] })),
            quest_kind_event(2, CAST_QUEST_PHASE_COMPLETED_KIND),
            quest_kind_event(3, CAST_QUEST_COMPLETED_KIND),
        ];
        let info = find_cast_quest_info(&events).expect("quest info should be present");
        assert!(info.is_complete);
    }

    #[test]
    fn format_quest_attach_note_describes_in_progress_quest() {
        let info = CastQuestAttachInfo {
            title: Some("Ship phase 7".to_string()),
            total_phases: Some(3),
            completed_phases: 1,
            ..Default::default()
        };
        let note = format_quest_attach_note(&info).expect("note should be produced");
        assert!(note.contains("Quest anchor"));
        assert!(note.contains("Ship phase 7"));
        assert!(note.contains("phase 1/3"));
        assert!(note.contains("in progress"));
    }

    #[test]
    fn format_quest_attach_note_describes_complete_quest() {
        let info = CastQuestAttachInfo {
            title: Some("Ship phase 7".to_string()),
            total_phases: Some(3),
            completed_phases: 3,
            is_complete: true,
            ..Default::default()
        };
        let note = format_quest_attach_note(&info).expect("note should be produced");
        assert!(note.contains("phase 3/3"));
        assert!(note.contains("complete"));
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
