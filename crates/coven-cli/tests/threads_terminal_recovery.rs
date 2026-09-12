//! Bounded real-daemon regressions for #932 / #886.
//! Public Tier-1 coherence intake is real; opened-window rows below are explicitly
//! synthetic, inconsistent LEGACY recovery history, not supported scheduled
//! intake/publication. Supersession, the reviewed-pin/human gate, and full #886
//! closure remain deferred.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use coven_threads_core::{
    ProposalApprovalAuditDetail, ProposalWindowAuditDetail, ProposalWindowCloseAuditDetail,
    WindowCloseReason,
};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[path = "fixtures/threads_daemon.rs"]
mod threads_daemon;
use threads_daemon::{run_journey, ThreadsFixture, FAMILIAR_ID};

const EDITS: &str = "/api/v1/familiars/sage/edits";
const PROPOSALS: &str = "/api/v1/threads/proposals";
const TARGET: &str = "reviewed/skill.md";
const BEFORE: &[u8] = b"before";
const AFTER: &str = "after";
// Hang guard, not a promptness claim: several scheduler intervals tolerate
// loaded native CI runners. Only the durable terminal row establishes success.
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(120);

#[test]
fn public_coherence_approval_has_no_fabricated_window_close() -> Result<()> {
    run_journey(|fixture| {
        let proposal = stage_public_coherence_edit(fixture)?;
        let submitted = audit_rows(fixture, &proposal)?;
        fixture.restart_daemon()?;
        assert_eq!(audit_rows(fixture, &proposal)?, submitted);
        assert!(
            proposal.pending.is_file(),
            "human review must remain pending"
        );
        assert_eq!(fs::read(fixture.workspace.join(TARGET))?, BEFORE);
        let approved = fixture.request(
            "POST",
            &proposal.approval_path(),
            Some(&proposal.decision_body()),
        )?;
        assert_eq!(approved.status, 200, "{approved:?}");
        assert_eq!(approved.body["decision"], "approved", "{approved:?}");
        assert_eq!(approved.body["reviewKind"], "coherence", "{approved:?}");
        assert_eq!(fs::read(fixture.workspace.join(TARGET))?, AFTER.as_bytes());
        assert_consumed(fixture, &proposal)?;

        let rows = audit_rows(fixture, &proposal)?;
        let terminal = only_terminal(&rows);
        assert_eq!(terminal.event, "proposal_approved", "{rows:?}");
        let detail: ProposalApprovalAuditDetail =
            serde_json::from_str(terminal.detail.as_deref().context("approval detail")?)?;
        assert!(detail.window_close.is_none(), "{detail:?}");
        assert!(
            rows.iter().all(|row| row.event != "proposal_window_opened"),
            "ordinary human approval invented an opened window: {rows:?}"
        );

        fixture.restart_daemon()?;
        let replay = fixture.request(
            "POST",
            &proposal.approval_path(),
            Some(&proposal.decision_body()),
        )?;
        assert_eq!(replay.status, 200, "{replay:?}");
        assert_eq!(replay.body["idempotent"], true, "{replay:?}");
        assert_eq!(audit_rows(fixture, &proposal)?, rows);
        assert_eq!(fs::read(fixture.workspace.join(TARGET))?, AFTER.as_bytes());
        assert_consumed(fixture, &proposal)
    })
}

#[test]
fn synthetic_legacy_opened_history_is_rejected_once_across_restart() -> Result<()> {
    assert_legacy_recovery(None)
}

#[test]
fn synthetic_legacy_opened_history_with_missing_ward_is_rejected_once() -> Result<()> {
    assert_legacy_recovery(Some(MissingPrerequisite::Ward))
}

#[test]
fn synthetic_legacy_opened_history_with_missing_familiar_is_rejected_once() -> Result<()> {
    assert_legacy_recovery(Some(MissingPrerequisite::Familiar))
}

enum MissingPrerequisite {
    Ward,
    Familiar,
}

fn assert_legacy_recovery(missing: Option<MissingPrerequisite>) -> Result<()> {
    run_journey(|fixture| {
        let proposal = stage_public_coherence_edit(fixture)?;
        let opened = seed_synthetic_legacy_opened_history(fixture, &proposal)?;
        match missing {
            Some(MissingPrerequisite::Ward) => {
                fs::remove_file(fixture.workspace.join("ward.toml"))?;
            }
            Some(MissingPrerequisite::Familiar) => {
                fs::write(fixture.coven_home.join("familiars.toml"), "")?;
            }
            None => {}
        }
        fixture.start_daemon()?;
        wait_for_startup_terminal(fixture, &proposal)?;
        let rows = assert_revalidation_rejected(fixture, &proposal, &opened)?;

        for after_restart in [false, true] {
            if after_restart {
                fixture.restart_daemon()?;
            }
            let replay = fixture.request(
                "POST",
                &proposal.approval_path(),
                Some(&proposal.decision_body()),
            )?;
            assert_eq!(replay.status, 409, "{replay:?}");
            assert_eq!(replay.body["why"], "proposal-already-decided", "{replay:?}");
            assert_eq!(replay.body["eventType"], "proposal_rejected", "{replay:?}");
            let same_decision = fixture.request(
                "POST",
                &format!("{PROPOSALS}/{}/reject", proposal.id),
                Some(&proposal.decision_body()),
            )?;
            assert_eq!(same_decision.status, 200, "{same_decision:?}");
            assert_eq!(same_decision.body["idempotent"], true, "{same_decision:?}");
            assert_eq!(
                assert_revalidation_rejected(fixture, &proposal, &opened)?,
                rows
            );
        }
        Ok(())
    })
}

fn wait_for_startup_terminal(fixture: &ThreadsFixture, proposal: &PublicProposal) -> Result<()> {
    let started = Instant::now();
    loop {
        let rows = audit_rows(fixture, proposal)?;
        if rows.iter().any(AuditRow::is_terminal) {
            return Ok(());
        }
        if started.elapsed() >= RECOVERY_TIMEOUT {
            anyhow::bail!(
                "startup recovery did not terminalize legacy opened history without a decision POST \
                 after {:?}; pending exists: {}; target bytes: {:?}; audit rows: {rows:?}; \
                 recovery log: {:?}",
                started.elapsed(),
                proposal.pending.exists(),
                fs::read(fixture.workspace.join(TARGET))?,
                fs::read_to_string(fixture.coven_home.join("daemon-recovery.log")),
            );
        }
        // Polling backoff only; elapsed time never establishes recovery success.
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct PublicProposal {
    id: String,
    pending: PathBuf,
    revision: String,
}

impl PublicProposal {
    fn approval_path(&self) -> String {
        format!("{PROPOSALS}/{}/approve", self.id)
    }

    fn decision_body(&self) -> Value {
        json!({"expectedRevision": self.revision, "note": "Synthetic human coherence review"})
    }
}

fn stage_public_coherence_edit(fixture: &ThreadsFixture) -> Result<PublicProposal> {
    fs::create_dir_all(fixture.workspace.join("reviewed"))?;
    fs::write(fixture.workspace.join(TARGET), BEFORE)?;
    let staged = fixture.request(
        "POST",
        EDITS,
        Some(&json!({"edits": [{"target": TARGET, "contents": AFTER}]})),
    )?;
    assert_eq!(staged.status, 202, "{staged:?}");
    assert_eq!(staged.body["disposition"], "staged", "{staged:?}");
    assert_eq!(staged.body["reviewKind"], "coherence", "{staged:?}");
    let id = staged.body["proposalId"]
        .as_str()
        .context("proposal id")?
        .to_owned();
    let pending = PathBuf::from(
        staged.body["pendingPath"]
            .as_str()
            .context("pending path")?,
    );
    assert!(
        pending.is_file(),
        "public intake did not persist {pending:?}"
    );
    assert_eq!(fs::read(fixture.workspace.join(TARGET))?, BEFORE);
    let detail = fixture.request("GET", &format!("{PROPOSALS}/{id}"), None)?;
    assert_eq!(detail.status, 200, "{detail:?}");
    assert_eq!(
        detail.body["proposal"]["reviewKind"], "coherence",
        "{detail:?}"
    );
    let revision = detail.body["proposal"]["proposalRevision"]
        .as_str()
        .context("public proposal revision")?
        .to_owned();
    let proposal = PublicProposal {
        id,
        pending,
        revision,
    };
    let rows = audit_rows(fixture, &proposal)?;
    assert_eq!(
        rows.iter()
            .filter(|row| row.event == "proposal_submitted")
            .count(),
        1,
        "public intake must submit the proposal: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|row| row.event != "proposal_window_opened" && !row.is_terminal()),
        "coherence intake unexpectedly opened or decided a window: {rows:?}"
    );
    Ok(proposal)
}

fn seed_synthetic_legacy_opened_history(
    fixture: &mut ThreadsFixture,
    proposal: &PublicProposal,
) -> Result<String> {
    fixture.stop_daemon()?;
    let pending_before = fs::read(&proposal.pending)?;
    let document: Value = serde_json::from_slice(&pending_before)?;
    assert!(document.get("decisionRequest").is_none(), "{document}");
    assert!(document.get("decisionState").is_none(), "{document}");
    let now = OffsetDateTime::now_utc();
    let detail = serde_json::to_string(&ProposalWindowAuditDetail {
        approval_path_label: "familiar_review".to_owned(),
        deadline: now + time::Duration::minutes(5),
        earliest_close: now + time::Duration::minutes(1),
        evidence_replay_hash_hex: "00".repeat(32),
        affected_regions: Vec::new(),
    })?;
    // The ONLY writable store access: append fixture-only legacy history to a
    // stopped daemon's existing database. Keep its real pending document and
    // submission intact; never seed approved/applying authority or alter triggers.
    let conn = Connection::open_with_flags(
        fixture.coven_home.join("coven.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    let inserted = conn.execute(
        "INSERT INTO ward_audit (
            event_type, proposal_id, familiar_id, ward_hash, decision, detail,
            files_touched, submitted_at, decided_at
         )
         SELECT 'proposal_window_opened', proposal_id, familiar_id, ward_hash,
                'window-opened', ?2, files_touched, submitted_at, ?3
         FROM ward_audit
         WHERE proposal_id = ?1 AND event_type = 'proposal_submitted'",
        params![proposal.id, detail, now.format(&Rfc3339)?],
    )?;
    assert_eq!(
        inserted, 1,
        "legacy fixture must append exactly one opened row"
    );
    drop(conn);
    assert_eq!(fs::read(&proposal.pending)?, pending_before);
    Ok(detail)
}

#[derive(Debug, PartialEq, Eq)]
struct AuditRow {
    id: i64,
    event: String,
    decision: String,
    detail: Option<String>,
    decided_at: String,
}

impl AuditRow {
    fn is_terminal(&self) -> bool {
        matches!(
            self.event.as_str(),
            "proposal_approved" | "proposal_rejected" | "proposal_vetoed"
        )
    }
}

fn audit_rows(fixture: &ThreadsFixture, proposal: &PublicProposal) -> Result<Vec<AuditRow>> {
    let conn = fixture.store()?;
    let mut statement = conn.prepare(
        "SELECT id, event_type, decision, detail, decided_at FROM ward_audit
         WHERE proposal_id = ?1 AND familiar_id = ?2 ORDER BY id",
    )?;
    let rows = statement.query_map(params![proposal.id, FAMILIAR_ID], |row| {
        Ok(AuditRow {
            id: row.get(0)?,
            event: row.get(1)?,
            decision: row.get(2)?,
            detail: row.get(3)?,
            decided_at: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn only_terminal(rows: &[AuditRow]) -> &AuditRow {
    let terminals: Vec<_> = rows.iter().filter(|row| row.is_terminal()).collect();
    assert_eq!(
        terminals.len(),
        1,
        "expected one durable terminal row: {rows:?}"
    );
    terminals[0]
}

fn assert_revalidation_rejected(
    fixture: &ThreadsFixture,
    proposal: &PublicProposal,
    opened_detail: &str,
) -> Result<Vec<AuditRow>> {
    let rows = audit_rows(fixture, proposal)?;
    let terminal = only_terminal(&rows);
    assert_eq!(terminal.event, "proposal_rejected", "{rows:?}");
    let raw_detail = terminal
        .detail
        .as_deref()
        .context("typed terminal detail")?;
    let close: ProposalWindowCloseAuditDetail = serde_json::from_str(raw_detail)?;
    assert_eq!(
        close.reason,
        WindowCloseReason::RevalidationFailed,
        "{close:?}"
    );
    assert_eq!(close.replay_hash_matched, Some(false), "{close:?}");
    assert_eq!(
        serde_json::from_str::<Value>(raw_detail)?["reason"],
        "revalidation_failed"
    );
    let opened: Vec<_> = rows
        .iter()
        .filter(|row| row.event == "proposal_window_opened")
        .collect();
    assert_eq!(
        opened.len(),
        1,
        "recovery invented another opened row: {rows:?}"
    );
    assert_eq!(opened[0].detail.as_deref(), Some(opened_detail));
    assert!(
        rows.iter()
            .all(|row| row.event != "apply_audit" && row.decision != "proposal-apply-intent"),
        "rejected history gained write authority: {rows:?}"
    );
    assert_eq!(fs::read(fixture.workspace.join(TARGET))?, BEFORE);
    assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");
    assert_consumed(fixture, proposal)?;
    Ok(rows)
}

fn assert_consumed(fixture: &ThreadsFixture, proposal: &PublicProposal) -> Result<()> {
    assert!(
        !proposal.pending.exists(),
        "terminal proposal remains pending"
    );
    let pending_dir = fixture.coven_home.join("pending");
    if pending_dir.exists() {
        let entries = fs::read_dir(&pending_dir)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        // Persistent directory bookkeeping is not a pending proposal artifact.
        assert!(
            entries
                .iter()
                .all(|name| name == ".quota.lock" || name == ".scheduler-cursor"),
            "terminal proposal left pending artifacts: {entries:?}"
        );
    }
    let listed = fixture.request("GET", PROPOSALS, None)?;
    assert_eq!(listed.status, 200, "{listed:?}");
    assert_eq!(listed.body["proposals"], json!([]), "{listed:?}");
    let reservations: i64 = fixture.store()?.query_row(
        "SELECT COUNT(*) FROM coven_ward_audit_reservations",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(reservations, 0, "terminal proposal retained audit capacity");
    Ok(())
}
