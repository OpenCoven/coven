//! Test-only coordination for process-global environment mutation.
//!
//! `cargo test` runs every module's unit tests in one parallel process, so
//! per-module env locks cannot serialize cross-module access: a `main.rs`
//! test mutating `COVEN_HOME` under its own lock still races a `harness.rs`
//! test reading adapter config under a different lock. Every test that reads
//! or mutates env vars shared across modules (the adapter manifest/dirs vars,
//! `COVEN_HOME`, `HOME`, …) must hold [`lock_env`] instead.

use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

/// The single crate-wide env mutex.
pub(crate) fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Acquire the crate-wide env lock, recovering from poisoning so a single
/// panicked env test reports one failure instead of cascading `PoisonError`
/// panics through every other env-touching test in the binary.
pub(crate) fn lock_env() -> MutexGuard<'static, ()> {
    env_lock().lock().unwrap_or_else(PoisonError::into_inner)
}

/// RAII guard that sets or removes one env var and restores the previous
/// value on drop, so a failed assertion cannot leak state into later tests.
/// Hold [`lock_env`] for the guard's whole lifetime.
pub(crate) struct EnvVarGuard {
    name: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    pub(crate) fn set(name: &'static str, value: impl AsRef<OsStr>) -> Self {
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        Self { name, previous }
    }

    pub(crate) fn remove(name: &'static str) -> Self {
        let previous = std::env::var_os(name);
        std::env::remove_var(name);
        Self { name, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.name, value),
            None => std::env::remove_var(self.name),
        }
    }
}
