//! The apply intent is the durable record, written before any file changes,
//! that recovery reads back to finish or roll back an approved proposal.
use super::*;

fn store() -> (tempfile::TempDir, rusqlite::Connection) {
    let home = tempfile::tempdir().unwrap();
    let conn = crate::store::open_store(&crate::api::store_path(home.path())).unwrap();
    (home, conn)
}

fn state(decision: &str) -> ProposalApplyingState {
    serde_json::from_value(serde_json::json!({
        "decision": decision,
        "recoveryCommitment": [1, 2, 3],
        "weaveHash": [4, 5, 6],
        "beforeImages": [{"target": "TOOLS.md", "resolved": "TOOLS.md"}],
        "rationale": "because",
    }))
    .unwrap()
}

fn now() -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn append(conn: &rusqlite::Connection, proposal_id: &str, state: &ProposalApplyingState) {
    append_proposal_apply_intent(
        conn,
        proposal_id,
        "familiar-1",
        Some(&coven_threads_core::WriterId::new("principal:fpr-val")),
        &["TOOLS.md".to_string()],
        state,
        coven_threads_core::Channel::Mutation,
        now(),
    )
    .unwrap();
}

fn insert_raw(
    conn: &rusqlite::Connection,
    proposal_id: &str,
    detail: &str,
    approver: &dyn rusqlite::ToSql,
) {
    conn.execute(
        "INSERT INTO ward_audit (
            event_type, proposal_id, familiar_id, ward_hash, decision, approver,
            detail, files_touched, submitted_at, decided_at
         ) VALUES (
            'validation_verdict', ?1, 'familiar-1', x'00', 'proposal-apply-intent', ?2,
            ?3, '[]', '2023-11-14T22:13:20Z', '2023-11-14T22:13:20Z'
         )",
        rusqlite::params![proposal_id, approver, detail],
    )
    .unwrap();
}

#[test]
fn proposal_apply_intent_is_absent_until_appended() {
    let (_home, conn) = store();
    assert_eq!(load_proposal_apply_intent(&conn, "p-1").unwrap(), None);
}

#[test]
fn proposal_apply_intent_round_trips_state_and_approver() {
    let (_home, conn) = store();
    append(&conn, "p-1", &state("approve"));
    assert_eq!(
        load_proposal_apply_intent(&conn, "p-1").unwrap(),
        Some(RecordedProposalApplyIntent {
            state: state("approve"),
            approver: Some("principal:fpr-val".to_string()),
        })
    );
    assert_eq!(load_proposal_apply_intent(&conn, "p-2").unwrap(), None);
}

#[test]
fn proposal_apply_intent_records_its_audit_columns() {
    let (_home, conn) = store();
    append(&conn, "p-1", &state("approve"));
    let row: (
        String,
        String,
        Vec<u8>,
        String,
        String,
        String,
        String,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT event_type, decision, ward_hash, files_touched, channel,
                    submitted_at, decided_at, thread_id
             FROM ward_audit WHERE proposal_id = 'p-1'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "validation_verdict".to_string(),
            "proposal-apply-intent".to_string(),
            vec![4, 5, 6],
            r#"["TOOLS.md"]"#.to_string(),
            "mutation".to_string(),
            "2023-11-14T22:13:20Z".to_string(),
            "2023-11-14T22:13:20Z".to_string(),
            None,
        )
    );
}

#[test]
fn proposal_apply_intent_reads_back_the_latest_append() {
    let (_home, conn) = store();
    append(&conn, "p-1", &state("approve"));
    append(&conn, "p-1", &state("reject"));
    let recorded = load_proposal_apply_intent(&conn, "p-1").unwrap().unwrap();
    assert_eq!(recorded.state, state("reject"));
}

#[test]
fn proposal_apply_intent_classifies_unreadable_records_as_invalid() {
    let (_home, conn) = store();
    insert_raw(&conn, "bad-detail", "{not json", &"principal:fpr-val");
    let error = load_proposal_apply_intent(&conn, "bad-detail").unwrap_err();
    assert!(invalid_proposal_apply_intent(&error), "{error:#}");
    assert!(error
        .to_string()
        .contains("proposal apply intent detail is invalid"));

    let detail = serde_json::to_string(&state("approve")).unwrap();
    // TEXT affinity keeps a BLOB a BLOB, which cannot be read as the approver.
    insert_raw(&conn, "bad-column", &detail, &vec![0xff_u8]);
    let error = load_proposal_apply_intent(&conn, "bad-column").unwrap_err();
    assert!(invalid_proposal_apply_intent(&error), "{error:#}");
    assert!(error
        .to_string()
        .contains("proposal apply intent columns are invalid"));

    let other = anyhow::anyhow!("unrelated");
    assert!(!invalid_proposal_apply_intent(&other));
}

#[test]
fn proposal_applying_state_refuses_unknown_fields() {
    let mut value = serde_json::to_value(state("approve")).unwrap();
    value["unexpected"] = true.into();
    assert!(serde_json::from_value::<ProposalApplyingState>(value).is_err());
}
