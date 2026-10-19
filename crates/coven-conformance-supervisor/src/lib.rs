//! Independent audit process mechanics, not Runtime Authority or a sandbox.
pub const ABI: &str = "coven.automations.windows-job-supervisor.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    InvalidLaunch,
    ContainmentUnavailable,
    CleanupUnproven,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidLaunch => "launch_invalid",
            Self::ContainmentUnavailable => "containment_unavailable",
            Self::CleanupUnproven => "cleanup_unproven",
        })
    }
}
impl std::error::Error for Failure {}

pub mod command;
pub mod control;
pub mod image;
pub mod lease;
pub mod protocol;
pub mod transfer;

#[cfg(windows)]
pub mod job;

#[cfg(windows)]
mod pipe;

#[cfg(any(windows, test))]
pub mod worker;

#[cfg(windows)]
pub mod coordinator;
