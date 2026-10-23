//! Bounded, owner-local lease diagnostics. These reads never renew or recover.
use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::contract::types::OccurrenceState;

pub(crate) const ACTION: &str = "coven.automations.occurrence.leases.v1";
pub(crate) const SCHEMA: &str = "coven.automations.leases.v1";
const CARRIES_LEASE: &str = "(lease_owner IS NOT NULL OR lease_expires_at IS NOT NULL)";
const MAX_CURSOR: usize = 4096;

#[derive(Debug)]
pub(crate) enum ReadError {
    Invalid(&'static str),
    Store,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    pub schema_version: String,
    pub observed_at: String,
    pub leases: Vec<Lease>,
    pub cursor: Cursor,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Cursor {
    pub current: Option<String>,
    pub next: Option<String>,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Lease {
    pub occurrence_id: String,
    pub automation_id: String,
    pub state: Option<OccurrenceState>,
    pub owner_present: bool,
    pub owner_ref: Option<String>,
    pub lease_expires_at: Option<String>,
    pub expiry: Expiry,
    pub scheduler_generation: Option<i64>,
    pub fence_generation: Option<i64>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Expiry {
    Active,
    Expired,
    Missing,
    Invalid,
}

/// The shared bounded timestamp interpretation used by paging and summaries.
pub(crate) fn classify_expiry(
    value: Option<&[u8]>,
    now: DateTime<Utc>,
) -> (Option<DateTime<Utc>>, Expiry) {
    let Some(value) = value else {
        return (None, Expiry::Missing);
    };
    let parsed = (value.len() <= 64)
        .then_some(value)
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));
    match parsed {
        Some(value) => (
            Some(value),
            if value <= now {
                Expiry::Expired
            } else {
                Expiry::Active
            },
        ),
        None => (None, Expiry::Invalid),
    }
}

#[derive(Default)]
pub(crate) struct LeaseSummary {
    pub total: u64,
    pub active: u64,
    pub expired: u64,
    pub missing_expiry: u64,
    pub invalid_expiry: u64,
}

/// The caller owns the coherent read snapshot. No IDs, owner text or history
/// are selected, and each timestamp is bounded before decoding.
pub(crate) fn summarize_expiry(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<LeaseSummary, ReadError> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT substr(CAST(lease_expires_at AS BLOB), 1, 65)
         FROM main.automation_occurrences WHERE {CARRIES_LEASE}"
        ))
        .map_err(|_| ReadError::Store)?;
    let mut rows = statement.query([]).map_err(|_| ReadError::Store)?;
    let mut summary = LeaseSummary::default();
    while let Some(row) = rows.next().map_err(|_| ReadError::Store)? {
        if summary.total == 9_007_199_254_740_991 {
            return Err(ReadError::Store);
        }
        let value: Option<Vec<u8>> = row.get(0).map_err(|_| ReadError::Store)?;
        summary.total += 1;
        match classify_expiry(value.as_deref(), now).1 {
            Expiry::Active => summary.active += 1,
            Expiry::Expired => summary.expired += 1,
            Expiry::Missing => summary.missing_expiry += 1,
            Expiry::Invalid => summary.invalid_expiry += 1,
        }
    }
    Ok(summary)
}

pub(crate) fn outside_active_state_count(conn: &Connection) -> Result<u64, ReadError> {
    let value: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM main.automation_occurrences WHERE {CARRIES_LEASE}
            AND (CASE WHEN state IN ('claimed', 'running') THEN 0 ELSE 1 END)=1"
            ),
            [],
            |row| row.get(0),
        )
        .map_err(|_| ReadError::Store)?;
    let count = u64::try_from(value).map_err(|_| ReadError::Store)?;
    if count > 9_007_199_254_740_991 {
        return Err(ReadError::Store);
    }
    Ok(count)
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && id.trim() == id && !id.chars().any(char::is_control)
}

fn position(cursor: &str, filter: Option<&str>) -> Result<String, ReadError> {
    let invalid = || ReadError::Invalid("Lease cursor is invalid for this automation filter.");
    if cursor.is_empty() || cursor.len() > MAX_CURSOR {
        return Err(invalid());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid())?;
    let value: (Option<String>, String) = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if value.0.as_deref() != filter
        || !valid_id(&value.1)
        || serde_json::to_vec(&value).map_err(|_| invalid())? != bytes
        || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes) != cursor
    {
        return Err(invalid());
    }
    Ok(value.1)
}

fn query(filtered: bool, resumed: bool) -> String {
    // Only fixed SQL fragments enter the statement. Parameters remain bound.
    // Partial indexes restrict work to lease-bearing rows; id is immutable.
    // Slice bytes before decoding: SQLite TEXT substr stops at embedded NUL,
    // which could otherwise turn malformed metadata into a valid prefix.
    // Invalid UTF-8 refuses the page through the row decoder.
    format!(
        "SELECT CAST(substr(CAST(id AS BLOB), 1, 513) AS TEXT), CAST(substr(CAST(automation_id AS BLOB), 1, 513) AS TEXT),
                CAST(substr(CAST(state AS BLOB), 1, 65) AS TEXT), CAST(substr(CAST(lease_owner AS BLOB), 1, 513) AS TEXT),
                length(CAST(lease_owner AS BLOB)), CAST(substr(CAST(lease_expires_at AS BLOB), 1, 65) AS TEXT),
                scheduler_generation, attempt
         FROM automation_occurrences
         WHERE {scope}{carries}{after}
         ORDER BY id ASC LIMIT ?3",
        scope = if filtered {
            "automation_id = ?1 AND "
        } else {
            ""
        },
        carries = CARRIES_LEASE,
        after = if resumed { " AND id > ?2" } else { "" },
    )
}

struct RawLease {
    id: String,
    automation: String,
    state: String,
    owner: Option<String>,
    owner_bytes: Option<i64>,
    expires: Option<String>,
    scheduler: Option<i64>,
    fence: i64,
}

fn project(row: RawLease, now: DateTime<Utc>) -> Result<Lease, ReadError> {
    if !valid_id(&row.id) || !valid_id(&row.automation) {
        return Err(ReadError::Store);
    }
    let mut issues = Vec::new();
    let owner_present = row.owner_bytes.is_some_and(|bytes| bytes > 0);
    let owner_ref = match row.owner {
        Some(owner)
            if owner_present
                && row.owner_bytes.is_some_and(|bytes| bytes <= 512)
                && !owner.chars().any(char::is_control) =>
        {
            Some(format!(
                "sha256:{}",
                super::contract::sha256_hex(owner.as_bytes())
            ))
        }
        _ => {
            issues.push(
                if owner_present {
                    "invalid_owner"
                } else {
                    "missing_owner"
                }
                .to_owned(),
            );
            None
        }
    };
    let (expires, expiry) = classify_expiry(row.expires.as_deref().map(str::as_bytes), now);
    let lease_expires_at = expires.map(|value| value.to_rfc3339_opts(SecondsFormat::AutoSi, true));
    match expiry {
        Expiry::Missing => issues.push("missing_expiry".to_owned()),
        Expiry::Invalid => issues.push("invalid_expiry".to_owned()),
        Expiry::Active | Expiry::Expired => {}
    }
    let state: Option<OccurrenceState> = serde_json::from_value(Value::String(row.state)).ok();
    if !matches!(
        state,
        Some(OccurrenceState::Claimed | OccurrenceState::Running)
    ) {
        issues.push("lease_outside_active_state".to_owned());
    }
    let safe = |value: i64| (0..=9_007_199_254_740_991).contains(&value);
    let scheduler_generation = row.scheduler.filter(|value| *value > 0 && safe(*value));
    if scheduler_generation != row.scheduler {
        issues.push("invalid_scheduler_generation".to_owned());
    }
    let fence_generation = safe(row.fence).then_some(row.fence);
    if fence_generation.is_none() {
        issues.push("invalid_fence_generation".to_owned());
    }
    Ok(Lease {
        occurrence_id: row.id,
        automation_id: row.automation,
        state,
        owner_present,
        owner_ref,
        lease_expires_at,
        expiry,
        scheduler_generation,
        fence_generation,
        issues,
    })
}

pub(crate) fn read(
    conn: &Connection,
    payload: &Value,
    observed_at: &str,
) -> Result<Page, ReadError> {
    let limit = match payload.get("limit") {
        None => 20,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=100).contains(value))
            .ok_or(ReadError::Invalid(
                "Lease limit must be an integer from 1 to 100.",
            ))? as usize,
    };
    let filter = match payload.get("automationId") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or(ReadError::Invalid("Lease automation filter is invalid."))?,
        ),
    };
    let current = match payload.get("cursor") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(ReadError::Invalid("Lease cursor must be a string."))?,
        ),
    };
    let after = current.map(|cursor| position(cursor, filter)).transpose()?;
    let now = DateTime::parse_from_rfc3339(observed_at)
        .map_err(|_| ReadError::Store)?
        .with_timezone(&Utc);
    let transaction = conn.unchecked_transaction().map_err(|_| ReadError::Store)?;
    let mut statement = transaction
        .prepare(&query(filter.is_some(), after.is_some()))
        .map_err(|_| ReadError::Store)?;
    let bound = i64::try_from(limit + 1).map_err(|_| ReadError::Store)?;
    let rows = statement
        .query_map(params![filter, after, bound], |row| {
            Ok(RawLease {
                id: row.get(0)?,
                automation: row.get(1)?,
                state: row.get(2)?,
                owner: row.get(3)?,
                owner_bytes: row.get(4)?,
                expires: row.get(5)?,
                scheduler: row.get(6)?,
                fence: row.get(7)?,
            })
        })
        .map_err(|_| ReadError::Store)?;
    let mut leases = rows
        .map(|row| project(row.map_err(|_| ReadError::Store)?, now))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    transaction.commit().map_err(|_| ReadError::Store)?;
    let has_more = leases.len() > limit;
    leases.truncate(limit);
    let next = if has_more {
        let id = &leases.last().ok_or(ReadError::Store)?.occurrence_id;
        Some(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&(filter, id)).map_err(|_| ReadError::Store)?),
        )
    } else {
        None
    };
    Ok(Page {
        schema_version: SCHEMA.to_owned(),
        observed_at: observed_at.to_owned(),
        leases,
        cursor: Cursor {
            current: current.map(ToOwned::to_owned),
            next,
            has_more,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: &str = "2026-09-01T09:00:00.000Z";

    #[test]
    fn malformed_requests_refuse_before_store_access_without_echoing_input() {
        let conn = Connection::open_in_memory().unwrap();
        for payload in [
            json!({"limit":0}),
            json!({"limit":101}),
            json!({"limit":true}),
            json!({"automationId":""}),
            json!({"automationId":" private-input "}),
            json!({"cursor":null}),
            json!({"cursor":"private-input"}),
            json!({"cursor":"A".repeat(MAX_CURSOR + 1)}),
        ] {
            match read(&conn, &payload, NOW) {
                Err(ReadError::Invalid(reason)) => assert!(!reason.contains("private-input")),
                _ => panic!("invalid lease request reached the store"),
            }
        }
        let cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&(Some("other"), "position")).unwrap());
        assert!(matches!(
            read(
                &conn,
                &json!({"automationId":"target","cursor":cursor}),
                NOW
            ),
            Err(ReadError::Invalid(_))
        ));
        let noncanonical =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"[null, "position"]"#);
        assert!(matches!(
            read(&conn, &json!({"cursor":noncanonical}), NOW),
            Err(ReadError::Invalid(_))
        ));
        assert_eq!(conn.total_changes(), 0);
    }

    fn fixture() -> (tempfile::TempDir, Connection) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = Connection::open(path).unwrap();
        (root, conn)
    }

    #[test]
    fn inconsistent_and_terminal_lease_metadata_is_visible_without_private_text() {
        let (_root, conn) = fixture();
        for (id, owner, expiry, state) in [
            ("a", None, Some("2026-09-01T08:00:00Z"), "failed"),
            ("b", Some("private-owner-marker"), None, "running"),
            (
                "c",
                Some("private-owner-marker"),
                Some("private-expiry-marker"),
                "private-state-marker",
            ),
        ] {
            conn.execute(
                "INSERT INTO automation_occurrences
                 (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at, created_at, updated_at)
                 VALUES (?1, 'orphaned-definition', ?1, ?4, ?2, ?3, ?1, ?1)",
                params![id, owner, expiry, state],
            ).unwrap();
        }
        let changes = conn.total_changes();
        conn.pragma_update(None, "query_only", true).unwrap();
        let result = serde_json::to_value(read(&conn, &json!({}), NOW).unwrap()).unwrap();
        assert_eq!(result["leases"].as_array().unwrap().len(), 3);
        assert_eq!(result["leases"][0]["ownerPresent"], false);
        assert_eq!(result["leases"][0]["expiry"], "expired");
        assert_eq!(result["leases"][0]["state"], "failed");
        assert!(result["leases"][0]["issues"]
            .as_array()
            .unwrap()
            .contains(&json!("lease_outside_active_state")));
        assert_eq!(result["leases"][1]["expiry"], "missing");
        assert_eq!(result["leases"][2]["expiry"], "invalid");
        assert!(result["leases"][2]["state"].is_null());
        assert!(result["leases"][2]["leaseExpiresAt"].is_null());
        let text = result.to_string();
        for private in [
            "private-owner-marker",
            "private-expiry-marker",
            "private-state-marker",
        ] {
            assert!(!text.contains(private));
        }
        assert_eq!(conn.total_changes(), changes);
    }

    #[test]
    fn oversized_expiry_never_becomes_a_valid_truncated_timestamp() {
        let (_root, conn) = fixture();
        let expires = format!("2026-09-01T10:00:00.{}Zprivate-suffix", "0".repeat(44));
        conn.execute(
            "INSERT INTO automation_occurrences
             (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at, created_at, updated_at)
             VALUES ('a', 'target', 'now', 'claimed', 'owner', ?1, 'now', 'now')",
            params![expires],
        ).unwrap();
        let result = serde_json::to_value(read(&conn, &json!({}), NOW).unwrap()).unwrap();
        assert_eq!(
            result["leases"][0]["expiry"], "invalid",
            "oversized malformed expiry was accepted as its prefix"
        );
        assert!(result["leases"][0]["leaseExpiresAt"].is_null());
    }

    #[test]
    fn embedded_nul_metadata_never_becomes_a_valid_prefix() {
        for column in [
            "id",
            "automation_id",
            "state",
            "lease_owner",
            "lease_expires_at",
        ] {
            let (_root, conn) = fixture();
            conn.execute(
                "INSERT INTO automation_occurrences
                 (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at, created_at, updated_at)
                 VALUES ('a', 'target', 'now', 'claimed', 'owner', '2026-09-01T10:00:00Z', 'now', 'now')",
                [],
            ).unwrap();
            let prefix = match column {
                "id" => "a",
                "automation_id" => "target",
                "state" => "claimed",
                "lease_owner" => "owner",
                _ => "2026-09-01T10:00:00Z",
            };
            // Column names come only from the fixed list above, never input.
            conn.execute(
                &format!("UPDATE automation_occurrences SET {column} = ?1"),
                params![format!("{prefix}\0private-suffix")],
            )
            .unwrap();
            let changes = conn.total_changes();
            conn.pragma_update(None, "query_only", true).unwrap();
            let result = read(&conn, &json!({"limit": 1}), NOW);
            if matches!(column, "id" | "automation_id") {
                assert!(
                    matches!(result, Err(ReadError::Store)),
                    "invalid {column} was accepted as its prefix"
                );
            } else {
                let value = serde_json::to_value(result.unwrap()).unwrap();
                let lease = &value["leases"][0];
                match column {
                    "state" => assert!(lease["state"].is_null(), "invalid state became claimed"),
                    "lease_owner" => {
                        assert_eq!(lease["ownerPresent"], true);
                        assert!(
                            lease["ownerRef"].is_null(),
                            "invalid owner was hashed as its prefix"
                        );
                        assert!(lease["issues"]
                            .as_array()
                            .unwrap()
                            .contains(&json!("invalid_owner")));
                    }
                    _ => {
                        assert_eq!(lease["expiry"], "invalid");
                        assert!(lease["leaseExpiresAt"].is_null());
                    }
                }
                assert!(!value.to_string().contains("private-suffix"));
            }
            assert_eq!(conn.total_changes(), changes);
        }
    }

    #[test]
    fn lease_pages_use_partial_indexes_without_sorting_all_occurrence_history() {
        let (_root, conn) = fixture();
        for filtered in [false, true] {
            for resumed in [false, true] {
                let plan = conn
                    .prepare(&format!("EXPLAIN QUERY PLAN {}", query(filtered, resumed)))
                    .unwrap()
                    .query_map(params![Some("target"), Some("position"), 21_i64], |row| {
                        row.get::<_, String>(3)
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
                    .join("; ");
                let index = if filtered {
                    "idx_automation_occurrences_leases_by_automation"
                } else {
                    "idx_automation_occurrences_leases"
                };
                assert!(
                    plan.contains(index),
                    "lease query did not use its partial index: {plan}"
                );
                assert!(
                    !plan.contains("TEMP B-TREE"),
                    "lease query sorted history: {plan}"
                );
            }
        }
    }
}
