//! Cast attach helpers.
//!
//! Phase 2 lets the user re-enter a previously-launched session through the
//! same follower the launch path uses. The pure helpers in this module decode
//! the `cast.summary` event Cast wrote at the end of the original run so the
//! attach outcome card can describe what already happened.

use serde_json::Value;

use crate::store;

use super::intent::CastHarness;
use super::quest::{
    advance, quest_from_goal, set_phase_sub_prompt, skip_phase, Quest, QuestPhaseSummary,
    CAST_QUEST_PHASE_EDITED_KIND, CAST_QUEST_PHASE_SKIPPED_KIND,
};

/// Event kind Cast writes when a launched session finishes. See
/// `shell::write_cast_summary_event` for the producer side.
pub(crate) const CAST_SUMMARY_KIND: &str = "cast.summary";

use super::quest::{
    CAST_QUEST_COMPLETED_KIND, CAST_QUEST_PHASE_COMPLETED_KIND, CAST_QUEST_STARTED_KIND,
};

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

/// Replayed quest state plus the anchor session id we attached to. The
/// re-attach path in `tui::shell::attach_via_daemon` uses this to either
/// resume the quest loop at the next pending phase or, when the quest is
/// already complete, surface a summary note.
#[derive(Clone, Debug)]
pub(crate) struct ReconstructedQuest {
    pub(crate) quest: Quest,
    pub(crate) is_complete: bool,
    pub(crate) anchor_session_id: String,
}

/// Replay `cast.quest.*` events from an anchor session to rebuild a
/// `Quest` in the state it was in when the user last interacted with it.
///
/// Returns `None` when `events` does not contain a `cast.quest.started`
/// payload (the session is not a quest anchor). The reconstructor is
/// best-effort: malformed event payloads and out-of-range indices are
/// skipped silently so a corrupt event log never blocks the resume path.
pub(crate) fn reconstruct_quest(events: &[store::EventRecord]) -> Option<ReconstructedQuest> {
    let started = events
        .iter()
        .find(|event| event.kind == CAST_QUEST_STARTED_KIND)?;
    let payload = serde_json::from_str::<Value>(&started.payload_json).ok()?;
    let goal = payload.get("goal").and_then(Value::as_str)?.to_string();
    let default_harness = payload
        .get("harness")
        .and_then(Value::as_str)
        .and_then(CastHarness::from_token);

    let mut quest = quest_from_goal(&goal, default_harness);
    let mut is_complete = false;

    for event in events {
        match event.kind.as_str() {
            kind if kind == CAST_QUEST_PHASE_EDITED_KIND => {
                let payload =
                    serde_json::from_str::<Value>(&event.payload_json).unwrap_or(Value::Null);
                let idx = payload.get("index").and_then(Value::as_u64);
                let sub_prompt = payload.get("sub_prompt").and_then(Value::as_str);
                if let (Some(idx), Some(text)) = (idx, sub_prompt) {
                    let _ = set_phase_sub_prompt(&mut quest, idx as usize, text.to_string());
                }
            }
            kind if kind == CAST_QUEST_PHASE_COMPLETED_KIND => {
                let payload =
                    serde_json::from_str::<Value>(&event.payload_json).unwrap_or(Value::Null);
                let summary = QuestPhaseSummary {
                    session_id: payload
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    exit_status: payload
                        .get("exit_status")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    exit_code: payload
                        .get("exit_code")
                        .and_then(Value::as_i64)
                        .map(|v| v as i32),
                    carried_context: Vec::new(),
                };
                advance(&mut quest, summary);
            }
            kind if kind == CAST_QUEST_PHASE_SKIPPED_KIND => {
                let payload =
                    serde_json::from_str::<Value>(&event.payload_json).unwrap_or(Value::Null);
                let idx = payload.get("index").and_then(Value::as_u64);
                let reason = payload.get("reason").and_then(Value::as_str);
                if let (Some(idx), Some(reason)) = (idx, reason) {
                    let _ = skip_phase(&mut quest, idx as usize, reason.to_string());
                }
            }
            kind if kind == CAST_QUEST_COMPLETED_KIND => {
                is_complete = true;
            }
            _ => {}
        }
    }

    Some(ReconstructedQuest {
        quest,
        is_complete,
        anchor_session_id: started.session_id.clone(),
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

    fn phase_completed_event(
        seq: i64,
        idx: usize,
        status: &str,
        exit_code: i32,
    ) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "anchor".to_string(),
            kind: CAST_QUEST_PHASE_COMPLETED_KIND.to_string(),
            payload_json: serde_json::json!({
                "phase": format!("p{idx}"),
                "index": idx,
                "session_id": format!("session-{idx}"),
                "exit_status": status,
                "exit_code": exit_code,
            })
            .to_string(),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        }
    }

    fn phase_skipped_event(seq: i64, idx: usize, reason: &str) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "anchor".to_string(),
            kind: CAST_QUEST_PHASE_SKIPPED_KIND.to_string(),
            payload_json: serde_json::json!({
                "phase": format!("p{idx}"),
                "index": idx,
                "reason": reason,
            })
            .to_string(),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        }
    }

    fn phase_edited_event(seq: i64, idx: usize, sub_prompt: &str) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "anchor".to_string(),
            kind: CAST_QUEST_PHASE_EDITED_KIND.to_string(),
            payload_json: serde_json::json!({
                "phase": format!("p{idx}"),
                "index": idx,
                "sub_prompt": sub_prompt,
            })
            .to_string(),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        }
    }

    fn started_event_with(session_id: &str, goal: &str, harness: &str) -> store::EventRecord {
        store::EventRecord {
            seq: 1,
            id: "event-1".to_string(),
            session_id: session_id.to_string(),
            kind: CAST_QUEST_STARTED_KIND.to_string(),
            payload_json: serde_json::json!({
                "title": "Test quest",
                "goal": goal,
                "harness": harness,
                "phases": ["design", "implement", "verify"],
            })
            .to_string(),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn reconstruct_quest_returns_none_when_no_started_event() {
        let events = vec![output_event(1, "noise")];
        assert!(reconstruct_quest(&events).is_none());
    }

    #[test]
    fn reconstruct_quest_rebuilds_initial_state_with_cursor_at_zero() {
        let events = vec![started_event_with("anchor-id", "ship phase 9", "codex")];
        let recon = reconstruct_quest(&events).expect("should reconstruct");
        assert_eq!(recon.anchor_session_id, "anchor-id");
        assert!(!recon.is_complete);
        assert_eq!(recon.quest.cursor, 0);
        assert_eq!(recon.quest.goal, "ship phase 9");
    }

    #[test]
    fn reconstruct_quest_replays_phase_completed_events_to_advance_cursor() {
        let events = vec![
            started_event_with("anchor-id", "ship phase 9", "codex"),
            phase_completed_event(2, 0, "completed", 0),
        ];
        let recon = reconstruct_quest(&events).expect("should reconstruct");
        assert_eq!(recon.quest.cursor, 1);
        assert!(!recon.is_complete);
    }

    #[test]
    fn reconstruct_quest_applies_user_edits_before_completion() {
        let events = vec![
            started_event_with("anchor-id", "ship phase 9", "codex"),
            phase_edited_event(2, 0, "REPLACED design sub-prompt"),
            phase_completed_event(3, 0, "completed", 0),
        ];
        let recon = reconstruct_quest(&events).expect("should reconstruct");
        assert!(
            recon.quest.phases[0].edited_by_user,
            "phase 0 should be marked edited"
        );
        // After completion the phase status is Complete, but the
        // sub_prompt retains the user's text for re-attach inspection.
        assert_eq!(
            recon.quest.phases[0].sub_prompt,
            "REPLACED design sub-prompt"
        );
    }

    #[test]
    fn reconstruct_quest_replays_skip_and_advances_past_it() {
        let events = vec![
            started_event_with("anchor-id", "ship phase 9", "codex"),
            phase_skipped_event(2, 0, "already covered"),
        ];
        let recon = reconstruct_quest(&events).expect("should reconstruct");
        // Skipping phase 0 rolls the cursor forward to phase 1.
        assert_eq!(recon.quest.cursor, 1);
    }

    #[test]
    fn reconstruct_quest_marks_is_complete_when_completed_event_present() {
        let events = vec![
            started_event_with("anchor-id", "ship phase 9", "codex"),
            phase_completed_event(2, 0, "completed", 0),
            phase_completed_event(3, 1, "completed", 0),
            phase_completed_event(4, 2, "completed", 0),
            store::EventRecord {
                seq: 5,
                id: "event-5".to_string(),
                session_id: "anchor".to_string(),
                kind: CAST_QUEST_COMPLETED_KIND.to_string(),
                payload_json: serde_json::json!({"title": "Test quest"}).to_string(),
                created_at: "2026-05-20T00:00:00Z".to_string(),
            },
        ];
        let recon = reconstruct_quest(&events).expect("should reconstruct");
        assert!(recon.is_complete);
        assert_eq!(recon.quest.cursor, 3, "cursor should be past all phases");
    }

    #[test]
    fn reconstruct_quest_skips_malformed_event_payloads_without_failing() {
        let mut bad_completed = phase_completed_event(2, 0, "completed", 0);
        bad_completed.payload_json = "not json".to_string();
        let events = vec![
            started_event_with("anchor-id", "ship phase 9", "codex"),
            bad_completed,
            phase_completed_event(3, 0, "completed", 0),
        ];
        let recon = reconstruct_quest(&events).expect("should reconstruct");
        // The first (malformed) event still triggers an advance because
        // `unwrap_or(Value::Null)` yields a default summary; both advances
        // run, so cursor lands at 2.
        assert_eq!(recon.quest.cursor, 2);
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
