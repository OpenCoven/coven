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
