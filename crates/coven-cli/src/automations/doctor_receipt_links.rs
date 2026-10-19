//! Bounded reciprocal index-link checks, without reading receipt evidence bodies.
use super::super::contract::types::{ReceiptId, RunId};
use anyhow::Result;
use rusqlite::{Connection, Row};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const MAX_ROWS: usize = 256;
pub(super) const MAX_INPUT_BYTES: usize = 131_072;
pub(super) const PREREQUISITES: &[(&str, &[&str])] = &[
    ("automation_runs", &["id", "receipt_id"]),
    ("automation_receipts", &["id", "run_id"]),
];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Observation {
    Consistent,
    Inconsistent,
    InvalidMetadata,
    BudgetExceeded,
}

enum Key {
    Null,
    Text(String),
    Invalid,
}
fn key(row: &Row<'_>, offset: usize, receipt: bool) -> Result<Key> {
    let kind: String = row.get(offset)?;
    if kind == "null" {
        return Ok(Key::Null);
    }
    if kind != "text" {
        return Ok(Key::Invalid);
    }
    let bytes: Vec<u8> = row.get(offset + 1)?;
    if bytes.len() > 160 {
        return Ok(Key::Invalid);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(Key::Invalid);
    };
    let valid = if receipt {
        ReceiptId::new(text.clone()).is_ok()
    } else {
        RunId::new(text.clone()).is_ok()
    };
    Ok(if valid { Key::Text(text) } else { Key::Invalid })
}

pub(super) fn inspect(conn: &Connection) -> Result<Observation> {
    let mut runs = BTreeMap::new();
    let mut receipts = BTreeMap::new();
    let mut receipt_runs = BTreeSet::new();
    let mut bytes = 0usize;
    for (table, second, is_receipt) in [
        ("automation_runs", "receipt_id", false),
        ("automation_receipts", "run_id", true),
    ] {
        // Fixed identifiers only. Projection caps bound each materialized key even
        // when schema constraints are absent; no receipt JSON/digest is selected.
        let mut statement = conn.prepare(&format!("SELECT typeof(id), CASE WHEN typeof(id)='text' THEN substr(CAST(id AS BLOB),1,161) END,
            typeof({second}), CASE WHEN typeof({second})='text' THEN substr(CAST({second} AS BLOB),1,161) END
            FROM main.{table} LIMIT {}", MAX_ROWS+1))?;
        let mut rows = statement.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            if count == MAX_ROWS {
                return Ok(Observation::BudgetExceeded);
            }
            count += 1;
            let Key::Text(id) = key(row, 0, is_receipt)? else {
                return Ok(Observation::InvalidMetadata);
            };
            let second = match key(row, 2, !is_receipt)? {
                Key::Text(value) => Some(value),
                Key::Null if !is_receipt => None,
                _ => return Ok(Observation::InvalidMetadata),
            };
            bytes += id.len() + second.as_ref().map_or(0, String::len);
            if bytes > MAX_INPUT_BYTES {
                return Ok(Observation::BudgetExceeded);
            }
            if is_receipt {
                let run = second.expect("receipt run key was required above");
                if !receipt_runs.insert(run.clone()) || receipts.insert(id, run).is_some() {
                    return Ok(Observation::InvalidMetadata);
                }
            } else if runs.insert(id, second).is_some() {
                return Ok(Observation::InvalidMetadata);
            }
        }
    }
    let consistent = runs.iter().all(|(run, receipt)| {
        receipt
            .as_ref()
            .is_none_or(|receipt| receipts.get(receipt) == Some(run))
    }) && receipts
        .iter()
        .all(|(receipt, run)| runs.get(run).and_then(Option::as_ref) == Some(receipt));
    Ok(if consistent {
        Observation::Consistent
    } else {
        Observation::Inconsistent
    })
}
