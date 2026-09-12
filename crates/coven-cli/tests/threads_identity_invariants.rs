//! Real-daemon identity regressions for #969 / #885.
//! All mutations and proposals use supported public intake/approval routes.
//! Restarts exercise durable human-review proposals, not synthetic opened-window
//! recovery or protected proposal authority. Store access is read-only.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};

#[path = "fixtures/threads_daemon.rs"]
mod threads_daemon;
use threads_daemon::{run_journey, ThreadsFixture, FAMILIAR_ID, PRINCIPAL_FINGERPRINT};

const EDITS: &str = "/api/v1/familiars/sage/edits";
const PROPOSALS: &str = "/api/v1/threads/proposals";
const NOTE: &str = "notes/today.md";
const REVIEWED: &str = "reviewed/skill.md";
const BEFORE: &str = "before";
const AFTER: &str = "after";

#[test]
fn active_identity_allows_public_tier_two_write_across_restart() -> Result<()> {
    run_journey(|fixture| {
        configure_identity(fixture, true)?;
        assert_note_applies(fixture)?;
        fixture.restart_daemon()?;
        assert_eq!(fs::read_to_string(fixture.workspace.join(NOTE))?, AFTER);
        assert_no_pending(fixture)?;
        assert_eq!(audit_count(fixture, "apply_audit")?, 1);
        Ok(())
    })
}

#[test]
fn no_invariants_preserve_normal_public_intake_without_identity_sources() -> Result<()> {
    run_journey(|fixture| {
        assert!(!fixture.workspace.join("IDENTITY.md").exists());
        assert_note_applies(fixture)?;
        let proposal = stage_coherence(fixture)?;
        let approved = fixture.request(
            "POST",
            &proposal.approval_path(),
            Some(&proposal.decision_body()),
        )?;
        assert_eq!(approved.status, 200, "{approved:?}");
        assert_eq!(approved.body["decision"], "approved", "{approved:?}");
        assert_eq!(fs::read_to_string(fixture.workspace.join(REVIEWED))?, AFTER);
        assert_no_pending(fixture)?;
        assert_eq!(audit_count(fixture, "proposal_approved")?, 1);
        Ok(())
    })
}

#[test]
fn active_identity_does_not_enable_public_protected_proposals() -> Result<()> {
    run_journey(|fixture| {
        configure_identity(fixture, true)?;
        let before = source_snapshot(fixture)?;
        let response = fixture.request(
            "POST",
            EDITS,
            Some(&json!({
                "edits": [{"target": "SOUL.md", "contents": soul("research and synthesis")}],
                "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
            })),
        )?;
        assert_eq!(response.status, 403, "{response:?}");
        assert_eq!(
            response.body["error"]["code"], "protected_proposal_forbidden",
            "{response:?}"
        );
        assert_eq!(
            response.body["error"]["details"]["targets"],
            json!(["SOUL.md"]),
            "{response:?}"
        );
        assert_eq!(source_snapshot(fixture)?, before);
        assert_no_pending(fixture)?;
        assert_no_write_authority(fixture)?;
        assert_eq!(audit_count(fixture, "proposal_submitted")?, 0);
        Ok(())
    })
}

#[test]
fn divergent_purpose_refuses_public_tier_two_write() -> Result<()> {
    assert_intake_refused(NOTE, IdentityChange::Purpose)
}

#[test]
fn divergent_purpose_refuses_public_tier_one_staging() -> Result<()> {
    assert_intake_refused(REVIEWED, IdentityChange::Purpose)
}

#[test]
fn missing_identity_refuses_public_tier_two_write() -> Result<()> {
    assert_intake_refused(NOTE, IdentityChange::MissingIdentity)
}

#[test]
fn missing_identity_refuses_public_tier_one_staging() -> Result<()> {
    assert_intake_refused(REVIEWED, IdentityChange::MissingIdentity)
}

#[test]
fn active_identity_allows_public_coherence_approval_after_restart() -> Result<()> {
    run_journey(|fixture| {
        configure_identity(fixture, true)?;
        let sources = source_snapshot(fixture)?;
        let proposal = stage_coherence(fixture)?;
        fixture.restart_daemon()?;
        assert!(proposal.pending.is_file(), "human review must stay pending");
        assert_eq!(
            fs::read_to_string(fixture.workspace.join(REVIEWED))?,
            BEFORE
        );
        assert_no_write_authority(fixture)?;

        let approved = fixture.request(
            "POST",
            &proposal.approval_path(),
            Some(&proposal.decision_body()),
        )?;
        assert_eq!(approved.status, 200, "{approved:?}");
        assert_eq!(approved.body["decision"], "approved", "{approved:?}");
        assert_eq!(approved.body["reviewKind"], "coherence", "{approved:?}");
        assert_eq!(fs::read_to_string(fixture.workspace.join(REVIEWED))?, AFTER);
        assert_eq!(source_snapshot(fixture)?, sources);
        assert_eq!(audit_count(fixture, "proposal_approved")?, 1);
        assert_eq!(audit_count(fixture, "proposal_window_opened")?, 0);
        assert_no_pending(fixture)?;
        assert!(!proposal.pending.exists());

        fixture.restart_daemon()?;
        let replay = fixture.request(
            "POST",
            &proposal.approval_path(),
            Some(&proposal.decision_body()),
        )?;
        assert_eq!(replay.status, 200, "{replay:?}");
        assert_eq!(replay.body["idempotent"], true, "{replay:?}");
        assert_eq!(audit_count(fixture, "proposal_approved")?, 1);
        assert_eq!(fs::read_to_string(fixture.workspace.join(REVIEWED))?, AFTER);
        assert_no_pending(fixture)
    })
}

#[test]
fn changed_authoritative_purpose_refuses_public_coherence_approval() -> Result<()> {
    assert_approval_refused(IdentityChange::Purpose, false)
}

#[test]
fn missing_authoritative_identity_refuses_approval_after_restart() -> Result<()> {
    assert_approval_refused(IdentityChange::MissingIdentity, true)
}

#[test]
fn changed_authoritative_roster_person_refuses_approval_after_restart() -> Result<()> {
    assert_approval_refused(IdentityChange::Person, true)
}

#[test]
fn changed_identity_bytes_that_still_satisfy_predicates_refuse_approval() -> Result<()> {
    assert_approval_refused(IdentityChange::IdentityBytes, false)
}

#[test]
fn changed_valid_identity_evidence_refuses_approval_after_restart() -> Result<()> {
    assert_approval_refused(IdentityChange::IdentityBytes, true)
}

#[derive(Clone, Copy, Debug)]
enum IdentityChange {
    Purpose,
    MissingIdentity,
    Person,
    IdentityBytes,
}

impl IdentityChange {
    fn apply(self, fixture: &ThreadsFixture) -> Result<()> {
        // Owner-side fixture drift, never a forged proposal or protected HTTP edit.
        match self {
            Self::Purpose => fs::write(fixture.workspace.join("SOUL.md"), soul("sabotage"))?,
            Self::MissingIdentity => fs::remove_file(fixture.workspace.join("IDENTITY.md"))?,
            Self::Person => fs::write(fixture.coven_home.join("familiars.toml"), roster("Other"))?,
            Self::IdentityBytes => {
                let path = fixture.workspace.join("IDENTITY.md");
                let before = fs::read_to_string(&path)?;
                fs::write(path, format!("{before}\nResearch notes stay local.\n"))?;
            }
        }
        Ok(())
    }

    fn intake_reason(self) -> &'static str {
        match self {
            Self::Purpose => "Purpose identity invariant did not hold",
            Self::MissingIdentity => "identity fact unavailable",
            Self::Person => "Person identity invariant did not hold",
            Self::IdentityBytes => unreachable!("valid source drift is an approval-only case"),
        }
    }

    fn approval_reason(self) -> &'static str {
        match self {
            Self::IdentityBytes => "proposal-identity-evidence-diverged",
            _ => "proposal-revalidation-failed",
        }
    }
}

fn assert_intake_refused(target: &str, change: IdentityChange) -> Result<()> {
    run_journey(|fixture| {
        // Missing IDENTITY must fail even with only mandatory Name and Person.
        configure_identity(fixture, !matches!(change, IdentityChange::MissingIdentity))?;
        change.apply(fixture)?;
        let path = fixture.workspace.join(target);
        fs::create_dir_all(path.parent().context("target parent")?)?;
        fs::write(&path, BEFORE)?;
        let before = source_snapshot(fixture)?;
        let response = fixture.request(
            "POST",
            EDITS,
            Some(&json!({"edits": [{"target": target, "contents": AFTER}]})),
        )?;
        let contents = fs::read_to_string(&path)?;
        eprintln!(
            "identity intake {change:?} {target}: {response:?}; target={contents:?}; \
             submitted={}; applied={}",
            audit_count(fixture, "proposal_submitted")?,
            audit_count(fixture, "apply_audit")?,
        );
        assert_eq!(response.status, 403, "{response:?}");
        assert_eq!(
            response.body["error"]["code"], "ward_refused",
            "{response:?}"
        );
        let gate = &response.body["error"]["details"]["threadsGate"];
        assert_eq!(gate["outcome"]["kind"], "rejected", "{response:?}");
        assert!(
            gate.to_string().contains(change.intake_reason()),
            "refusal must identify the failed identity predicate: {response:?}"
        );
        assert_eq!(contents, BEFORE, "refused intake wrote {target}");
        assert_eq!(source_snapshot(fixture)?, before);
        assert_no_pending(fixture)?;
        assert_no_write_authority(fixture)?;
        assert_eq!(audit_count(fixture, "proposal_submitted")?, 0);
        Ok(())
    })
}

fn assert_approval_refused(change: IdentityChange, restart: bool) -> Result<()> {
    run_journey(|fixture| {
        configure_identity(fixture, !matches!(change, IdentityChange::MissingIdentity))?;
        let proposal = stage_coherence(fixture)?;
        let staged_sources = source_snapshot(fixture)?;
        change.apply(fixture)?;
        let before = source_snapshot(fixture)?;
        assert_ne!(before, staged_sources, "fixture must change source bytes");
        if restart {
            fixture.restart_daemon()?;
        }
        assert_eq!(source_snapshot(fixture)?, before);
        assert_eq!(
            fs::read_to_string(fixture.workspace.join(REVIEWED))?,
            BEFORE
        );
        assert!(
            proposal.pending.is_file(),
            "human review must remain pending"
        );
        assert_no_write_authority(fixture)?;
        let refused = fixture.request(
            "POST",
            &proposal.approval_path(),
            Some(&proposal.decision_body()),
        )?;
        let contents = fs::read_to_string(fixture.workspace.join(REVIEWED))?;
        eprintln!(
            "identity approval {change:?} restart={restart}: {refused:?}; \
             target={contents:?}; approved={}; applied={}",
            audit_count(fixture, "proposal_approved")?,
            audit_count(fixture, "apply_audit")?,
        );
        assert_eq!(refused.status, 409, "{refused:?}");
        assert_eq!(refused.body["blocked"], true, "{refused:?}");
        assert_eq!(refused.body["why"], change.approval_reason(), "{refused:?}");
        if matches!(change, IdentityChange::IdentityBytes) {
            assert_eq!(
                refused.body.get("verdict"),
                Some(&Value::Null),
                "stale source evidence must refuse even when identity predicates hold: {refused:?}"
            );
        }
        assert_eq!(contents, BEFORE, "refused approval wrote the target");
        assert_eq!(source_snapshot(fixture)?, before);
        assert_no_write_authority(fixture)?;
        assert_eq!(audit_count(fixture, "proposal_submitted")?, 1);
        assert_eq!(audit_count(fixture, "proposal_window_opened")?, 0);
        // Ordinary human-review refusal retains the real proposal for a later
        // decision; it is not a terminal scheduled-window recovery rejection.
        assert!(
            proposal.pending.is_file(),
            "refusal lost the human-review proposal"
        );
        assert_no_reservations(fixture)
    })
}

fn configure_identity(fixture: &ThreadsFixture, include_purpose: bool) -> Result<()> {
    fs::write(fixture.coven_home.join("familiars.toml"), roster("Val"))?;
    fs::write(fixture.workspace.join("SOUL.md"), soul("research"))?;
    fs::write(
        fixture.workspace.join("IDENTITY.md"),
        "# IDENTITY.md - Sage\n- **Name:** Sage\n- **Pronouns:** she/her\n",
    )?;
    fs::write(fixture.workspace.join("MEMORY.md"), "facts stay local\n")?;
    let purpose = if include_purpose {
        r#"
[[identity_invariant]]
fact = "purpose"
operator = "includes"
expected = "research"
"#
    } else {
        ""
    };
    fs::write(
        fixture.workspace.join("ward.toml"),
        format!(
            r#"principal_key_fingerprint = "{PRINCIPAL_FINGERPRINT}"
protected_surface = ["SOUL.md", "IDENTITY.md", "MEMORY.md"]

[[identity_invariant]]
fact = "name"
operator = "equals"
expected = "Sage"

[[identity_invariant]]
fact = "person"
operator = "equals"
expected = "Val"
{purpose}
[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "IDENTITY.md"
tier = 0

[[surface]]
path = "MEMORY.md"
tier = 0

[[surface]]
path = "reviewed/"
tier = 1

[[probe]]
surface = "reviewed/**"
id = "size-delta"

[[probe]]
surface = "reviewed/**"
id = "pattern-lint"
forbidden = ["(?i)ignore previous"]
"#
        ),
    )?;
    Ok(())
}

fn soul(purpose: &str) -> String {
    format!("# SOUL\n## I am Sage\nMy purpose is {purpose}.\n")
}

fn roster(person: &str) -> String {
    format!(
        r#"[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Reads and synthesizes."
pronouns = "she/her"
person = "{person}"
coven = "OpenCoven"
"#
    )
}

fn source_snapshot(fixture: &ThreadsFixture) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
    let paths = ["SOUL.md", "IDENTITY.md", "MEMORY.md", "ward.toml"]
        .map(|name| fixture.workspace.join(name))
        .into_iter()
        .chain([fixture.coven_home.join("familiars.toml")]);
    paths
        .map(|path| {
            let bytes = if path.try_exists()? {
                Some(fs::read(&path)?)
            } else {
                None
            };
            Ok((path, bytes))
        })
        .collect()
}

fn assert_note_applies(fixture: &ThreadsFixture) -> Result<()> {
    let response = fixture.request(
        "POST",
        EDITS,
        Some(&json!({"edits": [{"target": NOTE, "contents": AFTER}]})),
    )?;
    assert_eq!(response.status, 200, "{response:?}");
    assert_eq!(response.body["disposition"], "applied", "{response:?}");
    assert_eq!(
        response.body["threadsGate"]["outcome"]["kind"], "permitted",
        "{response:?}"
    );
    assert_eq!(fs::read_to_string(fixture.workspace.join(NOTE))?, AFTER);
    assert_eq!(audit_count(fixture, "apply_audit")?, 1);
    assert_no_pending(fixture)
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
        json!({"expectedRevision": self.revision, "note": "Synthetic human identity review"})
    }
}

fn stage_coherence(fixture: &ThreadsFixture) -> Result<PublicProposal> {
    fs::create_dir_all(fixture.workspace.join("reviewed"))?;
    fs::write(fixture.workspace.join(REVIEWED), BEFORE)?;
    let staged = fixture.request(
        "POST",
        EDITS,
        Some(&json!({"edits": [{"target": REVIEWED, "contents": AFTER}]})),
    )?;
    assert_eq!(staged.status, 202, "{staged:?}");
    assert_eq!(staged.body["disposition"], "staged", "{staged:?}");
    assert_eq!(staged.body["reviewKind"], "coherence", "{staged:?}");
    let id = staged.body["proposalId"]
        .as_str()
        .context("public proposal id")?
        .to_owned();
    let pending = PathBuf::from(
        staged.body["pendingPath"]
            .as_str()
            .context("public pending path")?,
    );
    assert!(
        pending.is_file(),
        "public intake did not persist {pending:?}"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join(REVIEWED))?,
        BEFORE
    );
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
    assert_eq!(audit_count(fixture, "proposal_submitted")?, 1);
    assert_eq!(audit_count(fixture, "proposal_approved")?, 0);
    assert_eq!(audit_count(fixture, "proposal_window_opened")?, 0);
    Ok(PublicProposal {
        id,
        pending,
        revision,
    })
}

fn audit_count(fixture: &ThreadsFixture, event: &str) -> Result<i64> {
    fixture
        .store()?
        .query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE familiar_id = ?1 AND event_type = ?2",
            [FAMILIAR_ID, event],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn assert_no_write_authority(fixture: &ThreadsFixture) -> Result<()> {
    assert_eq!(
        audit_count(fixture, "apply_audit")?,
        0,
        "refusal gained apply evidence"
    );
    assert_eq!(
        audit_count(fixture, "proposal_approved")?,
        0,
        "refusal gained approval"
    );
    let intents: i64 = fixture.store()?.query_row(
        "SELECT COUNT(*) FROM ward_audit
         WHERE familiar_id = ?1 AND decision = 'proposal-apply-intent'",
        [FAMILIAR_ID],
        |row| row.get(0),
    )?;
    assert_eq!(intents, 0, "refusal gained apply intent");
    Ok(())
}

fn assert_no_pending(fixture: &ThreadsFixture) -> Result<()> {
    let listed = fixture.request("GET", PROPOSALS, None)?;
    assert_eq!(listed.status, 200, "{listed:?}");
    assert_eq!(listed.body["proposals"], json!([]), "{listed:?}");
    let pending = fixture.coven_home.join("pending");
    if pending.try_exists()? {
        let entries = fs::read_dir(pending)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        assert!(
            entries
                .iter()
                .all(|name| name == ".quota.lock" || name == ".scheduler-cursor"),
            "unexpected pending artifacts: {entries:?}"
        );
    }
    assert_no_reservations(fixture)
}

fn assert_no_reservations(fixture: &ThreadsFixture) -> Result<()> {
    let reservations: i64 = fixture.store()?.query_row(
        "SELECT COUNT(*) FROM coven_ward_audit_reservations",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(reservations, 0, "identity journey retained audit capacity");
    Ok(())
}
