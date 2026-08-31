//! Coven-native routine automations (coven#816).
//!
//! Routines replace harness-owned schedules with durable Coven definitions.
//! **Coven owns the schedule and the run ledger; runtimes are replaceable
//! workers.** This module owns definition parsing/validation (stored under
//! the Coven store, never a harness home), the RRULE vocabulary the
//! scheduler understands, occurrence planning with a durable
//! `UNIQUE(automation_id, scheduled_for)` fence, bounded claim/lease
//! recovery, familiar-bound dispatch through the shared session-launch seam,
//! the run ledger, and delivery of outputs.

pub mod daemon_tick;
pub mod definition;
pub mod delivery;
pub mod health;
pub mod import_legacy;
pub mod occurrences;
pub mod rrule;
pub mod runner;
pub mod runs;
pub mod schedule;
pub mod store;

pub use definition::RoutineDefinition;

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::Connection;

/// Everything one automations pass did, shared by the daemon cadence and
/// the `coven.automations.tick` control-plane action.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FullTickReport {
    pub tick: occurrences::TickReport,
    pub dispatch: runner::DispatchReport,
    pub settlement: delivery::ReconcileReport,
}

/// The one full tick implementation: plan due slots, recover expired
/// leases, claim due work, then dispatch every valid existing claim — not
/// just freshly claimed ones — and settle finished runs. Both callers (the
/// daemon cadence and the `coven.automations.tick` action) run this exact
/// sequence, so control-plane claims and crash-interrupted claims always
/// reach dispatch instead of waiting for some later tick (coven#816
/// finding 2).
pub fn full_tick(
    coven_home: &Path,
    conn: &Connection,
    runtime: &dyn crate::api::SessionRuntime,
    now: DateTime<Utc>,
) -> Result<FullTickReport, String> {
    let tick = occurrences::tick(conn, now).map_err(|error| format!("{error:#}"))?;
    let dispatch = runner::dispatch_claimed_occurrences(coven_home, conn, runtime, now)?;
    let settlement = delivery::settle_finished_runs(conn, now)?;
    Ok(FullTickReport {
        tick,
        dispatch,
        settlement,
    })
}
