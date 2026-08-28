//! Coven-native routine automations (coven#816).
//!
//! Routines replace harness-owned schedules with durable Coven definitions.
//! This module owns definition parsing/validation, the RRULE vocabulary the
//! scheduler understands, and definition persistence. Occurrence planning,
//! claim/lease, and run delivery land in follow-up modules on the same
//! seams.

pub mod daemon_tick;
pub mod definition;
pub mod import_legacy;
pub mod occurrences;
pub mod rrule;
pub mod runner;
pub mod runs;
pub mod schedule;
pub mod store;

pub use definition::RoutineDefinition;
