//! Coven-native routine automations (coven#816).
//!
//! Routines replace harness-owned schedules with durable Coven definitions.
//! This module owns definition parsing/validation, the RRULE vocabulary the
//! scheduler understands, and definition persistence. Occurrence planning,
//! claim/lease, and run delivery land in follow-up modules on the same
//! seams.

pub mod attempt_retry;
pub mod authority_projection;
pub mod cancel_commands;
pub mod cancellation;
pub mod capability_negotiation;
pub mod command_adoption;
pub mod command_envelope;
pub mod command_matrix;
pub mod conformance_target;
#[cfg(test)]
mod conformance_target_tests;
pub mod contract;
pub mod daemon_tick;
pub mod definition;
pub mod delivery;
pub mod diagnostics;
pub mod doctor;
pub(crate) mod doctor_cli;
pub(crate) mod explain;
// Role keys for the daemon's Runtime Authority signers; no production caller
// until the trusted adapter lands (coven#857).
#[allow(dead_code)]
pub mod authority_keys;
// Accepting verifiers for evidence no production producer signs yet (coven#857).
#[allow(dead_code)]
pub mod ed25519_trust;
pub mod health;
pub mod import_legacy;
pub mod inspection;
pub mod leadership;
pub(crate) mod lease_cli;
pub(crate) mod leases;
pub mod missions;
pub mod occurrences;
pub(crate) mod operator_cli;
// Owner grants: production records them; the trusted adapter (coven#857 slice 6)
// is their only reader, so the resolver has no production caller yet.
#[allow(dead_code)]
pub mod owner_grants;
pub mod receipts;
pub mod recovery;
#[cfg(test)]
mod release_upgrade_tests;
pub(crate) mod retry_cli;
pub mod rich_definition;
pub mod rrule;
pub mod run_now;
// Runtime Authority launch envelopes; slice 6 launches them.
#[allow(dead_code)]
pub mod runtime_envelope;
// The inbox is intentionally internal until a trusted runtime adapter exists.
pub mod runner;
pub mod runs;
#[allow(dead_code)]
pub mod runtime_terminal_evidence;
pub mod schedule;
// The scoped read service's exact-file reads (coven#857); nothing serves them
// until the service channel and harness admission land.
#[allow(dead_code)]
pub(crate) mod scoped_read;
pub mod store;
// The production scoped read gate: P, and the immutable commitments it
// records under the store's write lock (#1603).
#[cfg(unix)]
pub(crate) mod store_gate;
// A scoped read attempt's close, and its reconciliation with the gate's
// commitments before settlement (#1606).
pub(crate) mod scoped_read_settlement;
pub mod stream_observation;
// Wired into every session ending; it observes only Runtime Authority attempts,
// which nothing dispatches until the trusted adapter (coven#857 slice 6).
pub mod terminal_observer;
// Threads decisions: the trusted adapter (coven#857 slice 6) is their only
// caller, so nothing in production decides yet.
#[allow(dead_code)]
pub mod threads_decisions;
pub mod transition_events;
#[cfg(test)]
mod transport_authority_tests;
// The trusted Runtime Authority adapter (coven#857, slice 6).
pub mod trusted_authority;
// Launching a trusted attempt under its decision's scopes (coven#857 slice 6,
// #1603). The daemon launches the scoped read envelope only through it.
#[cfg(unix)]
pub(crate) mod trusted_launch;

#[allow(unused_imports)]
pub use definition::RoutineDefinition;
