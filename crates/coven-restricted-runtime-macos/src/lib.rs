//! macOS Seatbelt backend for `coven_restricted_runtime::Driver`.
//!
//! This crate supplies one reviewed-in-progress backend for the single-use
//! restricted worker controller. It is conformance evidence about kernel
//! enforcement of one offline worker on macOS; it is **not** session-policy
//! v1 support, a permission grant, or a change to production launch paths.
//! See the crate README for the exact obligations it meets and the ones it
//! still leaves open.
//!
//! On non-macOS targets the crate exposes only the role constants so the
//! workspace keeps building; there is no backend and no fallback.

/// Basename the sealed worker executable must carry inside the workspace.
pub const TARGET_NAME: &str = "coven-worker-target";
/// `argv[0]` the backend uses when re-executing the host binary as guardian.
pub const GUARDIAN_NAME: &str = "coven-worker-guardian";

/// Role a host binary was launched in, derived from `argv[0]` only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Guardian,
    Target,
}

/// Detects the guardian/target role from `argv[0]`'s basename. Neither role
/// reads the environment or accepts configurable arguments from the worker.
pub fn role() -> Option<Role> {
    let argv0 = std::env::args_os().next()?;
    let name = std::path::Path::new(&argv0).file_name()?;
    if name == GUARDIAN_NAME {
        Some(Role::Guardian)
    } else if name == TARGET_NAME {
        Some(Role::Target)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
mod backend;
#[cfg(target_os = "macos")]
mod guardian;
#[cfg(target_os = "macos")]
pub use backend::{
    ControllerStdio, InstantClock, SealError, SeatbeltConfig, SeatbeltDriver, WorkerStdio,
};
#[cfg(target_os = "macos")]
pub use guardian::guardian_main;
