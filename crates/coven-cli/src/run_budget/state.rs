//! Latest continuation confirmation belongs to the mutable engine history,
//! not to any one of the ledger rows that may name that history.
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::Receipt;

pub struct Claim {
    pub engine_session_id: String,
    ledger_session_id: String,
}

pub fn start(
    conn: &Connection,
    record: &crate::store::SessionRecord,
    engine_session_id: &str,
    require_confirmed: bool,
    negotiated: &mut Option<super::Negotiated>,
) -> Result<Option<Claim>> {
    let previous: Option<(String, bool)> = conn.query_row(
        "SELECT latest_ledger_id, resumable FROM engine_continuations WHERE engine_session_id = ?1",
        [engine_session_id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let Some((previous_ledger, resumable)) = previous else {
        ensure!(!require_confirmed, "Saved engine continuation is unconfirmed or unavailable; unfinished work cannot be replayed as a new conversation.");
        // Preserve the pre-extension path only for an unbudgeted legacy
        // request. Once v1 has observed this engine history, all aliases use
        // its latest confirmation, including unbudgeted ordinary resumes.
        crate::store::insert_session(conn, record)?;
        return Ok(None);
    };
    ensure!(resumable, "The latest engine history was not confirmed saved, or is still running; continuation is unavailable.");
    if negotiated.is_none() {
        *negotiated = Some(super::negotiate(crate::engine::ENGINE_HARNESS_ID, None)?);
    }
    crate::store::durable_transaction(conn, || {
        let changed = conn.execute(
            "UPDATE engine_continuations SET latest_ledger_id = ?2, resumable = 0, updated_at = ?4
             WHERE engine_session_id = ?1 AND latest_ledger_id = ?3 AND resumable = 1",
            params![
                engine_session_id,
                record.id,
                previous_ledger,
                record.updated_at
            ],
        )?;
        ensure!(
            changed == 1,
            "Engine continuation changed before launch; reload its latest state."
        );
        crate::store::insert_session(conn, record)
    })?;
    Ok(Some(Claim {
        engine_session_id: engine_session_id.to_owned(),
        ledger_session_id: record.id.clone(),
    }))
}

pub fn retain(
    conn: &Connection,
    ledger_session_id: &str,
    receipt: &Receipt,
    claim: Option<&Claim>,
    resumable: bool,
) -> Result<()> {
    if let Some(claim) = claim {
        ensure!(
            claim.ledger_session_id == ledger_session_id
                && claim.engine_session_id == receipt.native_session_id,
            "Engine continuation identity changed during the run"
        );
    }
    let now = crate::api::current_timestamp();
    crate::store::durable_transaction(conn, || {
        let changed = conn.execute(
            "INSERT INTO engine_continuations (engine_session_id, latest_ledger_id, resumable, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(engine_session_id) DO UPDATE SET resumable = excluded.resumable, updated_at = excluded.updated_at
             WHERE engine_continuations.latest_ledger_id = excluded.latest_ledger_id",
            params![receipt.native_session_id, ledger_session_id, resumable && receipt.continuation.resumable, now],
        ).context("retaining latest engine continuation state")?;
        ensure!(
            changed == 1,
            "A different run owns the latest engine continuation"
        );
        crate::store::update_session_conversation_id(
            conn,
            ledger_session_id,
            &receipt.native_session_id,
            &now,
        )?;
        crate::store::insert_json_event(
            conn,
            ledger_session_id,
            "engine_run_budget",
            &serde_json::json!({
                "engine_session_id":receipt.native_session_id,
                "receipt":receipt,
                "consumer_resumable":resumable && receipt.continuation.resumable,
            }),
            &now,
        )
    })
}
