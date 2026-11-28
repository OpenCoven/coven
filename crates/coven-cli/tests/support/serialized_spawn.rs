//! Serialized process creation for integration test binaries.
//!
//! On macOS, std creates each pipe with `pipe()` and sets close-on-exec in a
//! second call, and `posix_spawn` runs without `POSIX_SPAWN_CLOEXEC_DEFAULT`.
//! Tests in one binary run on parallel threads, so a spawn on one thread can
//! land between those two calls on another and inherit that test's pipe end.
//! A leaked read end keeps a "closed" stdout writable; a leaked write end
//! delays the owner's EOF until the inheriting process exits (#1814).
//!
//! Holding one lock across `spawn()` only, where std creates and seals the
//! pipes, closes that window while tests still run in parallel. It only works
//! if every launch in the binary goes through it: an unlocked spawn on another
//! thread can still inherit a pipe that a locked spawn is creating.
#![allow(dead_code)] // Each binary uses the subset of launchers it needs.

use std::io;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Mutex, PoisonError};

static SPAWN: Mutex<()> = Mutex::new(());

pub trait SerializedSpawn {
    /// `spawn()` under the binary-wide spawn lock.
    fn spawn_serialized(&mut self) -> io::Result<Child>;
    /// `output()` with its defaults (stdin null, stdout and stderr piped),
    /// spawned under the lock and waited on outside it.
    fn output_serialized(&mut self) -> io::Result<Output>;
    /// `status()` with its default (inherited stdio), spawned under the lock
    /// and waited on outside it.
    fn status_serialized(&mut self) -> io::Result<ExitStatus>;
}

impl SerializedSpawn for Command {
    fn spawn_serialized(&mut self) -> io::Result<Child> {
        let _spawning = SPAWN.lock().unwrap_or_else(PoisonError::into_inner);
        self.spawn()
    }

    fn output_serialized(&mut self) -> io::Result<Output> {
        self.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn_serialized()?
            .wait_with_output()
    }

    fn status_serialized(&mut self) -> io::Result<ExitStatus> {
        self.spawn_serialized()?.wait()
    }
}
