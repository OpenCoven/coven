//! `coven adapter login <adapter>` shim.
//!
//! Wraps a vendor CLI's own login flow with our preflight cleanup. Today the
//! only adapter that needs this is Codex (OpenAI), whose `codex login` flow
//! spins up a local OAuth callback server on port **1455** and fails hard
//! (`EADDRINUSE`) when a previous `codex login` was killed mid-flow and the
//! orphan listener is still around.
//!
//! Preflight behavior:
//! 1. Try to bind 127.0.0.1:1455 ourselves. If that succeeds, the port is
//!    free — drop the listener and proceed straight to `codex login`.
//! 2. If bind fails with `AddrInUse`, identify the holding process via `lsof`
//!    and only kill it if its command line clearly identifies it as a `codex`
//!    OAuth helper. Never kill an unidentified process — we'd rather print a
//!    clear error than terminate something unrelated that happens to be on
//!    the same port.
//! 3. Wait for the port to actually free up (poll up to ~1.5s), then exec
//!    `codex login`.
//!
//! This shim is deliberately conservative: any time we're not 100% sure the
//! port-holder is a codex OAuth listener, we bail with instructions for the
//! user.

use anyhow::{anyhow, Context, Result};
use std::io::ErrorKind;
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

/// The fixed port that Codex CLI's OAuth callback server tries to bind.
///
/// This is hard-coded in the upstream `@openai/codex` package. There is no
/// configuration to make Codex pick a free port instead, which is why we need
/// this shim in the first place.
pub const CODEX_OAUTH_PORT: u16 = 1455;

/// Result of a port-1455 preflight check. Used by tests and surfaced in CLI
/// output before we exec `codex login`.
#[derive(Debug, PartialEq, Eq)]
pub enum PreflightOutcome {
    /// Port was already free; no cleanup needed.
    PortFree,
    /// Port was held by a confirmed-Codex process; we killed it and
    /// re-confirmed the port is free.
    ClearedStaleCodex { killed_pid: u32 },
    /// Port was held by something that does NOT look like Codex; we did NOT
    /// kill it. Caller should print instructions and abort.
    HeldByOther { pid: u32, descriptor: String },
    /// Port was held but we couldn't even identify the holder (lsof missing
    /// or returned nothing). Conservative: don't kill, let the user investigate.
    HeldUnknown,
}

/// Try to bind the OAuth callback port; if we can bind it, the port is free
/// and the listener is dropped immediately.
pub fn port_is_free(port: u16) -> bool {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => true,
        Err(e) if e.kind() == ErrorKind::AddrInUse => false,
        // Other errors (permission denied, etc) — treat as not free so we
        // surface them later when we re-bind for real.
        Err(_) => false,
    }
}

/// Find the pid holding `port` on localhost via `lsof`, if available.
///
/// Returns `Ok(Some(pid))` when we can confidently identify a single holder,
/// `Ok(None)` when nothing is found (or lsof is missing), and `Err(...)` only
/// for unexpected failures. This is intentionally permissive — "no lsof, no
/// help" is the right answer; we shouldn't kill anything we can't ID.
pub fn pid_holding_port(port: u16) -> Result<Option<u32>> {
    // `lsof -ti tcp:<port>` prints one pid per line; `-t` makes it
    // terse (pid-only) and `-i tcp:<port>` filters to TCP sockets on that
    // port. Works on macOS and Linux. Windows has no lsof.
    let output = match Command::new("lsof")
        .args(["-ti", &format!("tcp:{port}")])
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| "failed to invoke lsof"),
    };

    if !output.status.success() {
        // lsof exits non-zero when no matches; that's not an error for us.
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut pids = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.parse::<u32>().ok());

    let first = pids.next();
    if pids.next().is_some() {
        // Multiple holders on the same port is suspicious — refuse to kill
        // ambiguously. Treat as "unknown holder".
        return Ok(None);
    }
    Ok(first)
}

/// Best-effort description of a pid: its executable name and short command
/// line. Used to ID whether the holder looks like a `codex` process.
///
/// Returns an empty string if we can't tell.
pub fn describe_pid(pid: u32) -> String {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    let Some(proc) = sys.process(Pid::from_u32(pid)) else {
        return String::new();
    };

    let name = proc.name().to_string_lossy().to_string();
    let cmd = proc
        .cmd()
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" ");

    if cmd.is_empty() {
        name
    } else {
        format!("{name} ({cmd})")
    }
}

/// Heuristic: does this descriptor look like a `codex` OAuth helper we can
/// safely terminate? We require the literal `codex` token to appear as a
/// path component or argv element. We deliberately do NOT match substrings
/// like `vscode` or `markdownify` that happen to contain "codex".
pub fn descriptor_looks_like_codex(descriptor: &str) -> bool {
    if descriptor.is_empty() {
        return false;
    }
    descriptor
        .split(|c: char| c.is_whitespace() || c == '/' || c == '\\')
        .any(|token| token == "codex" || token == "codex.js" || token == "@openai/codex")
}

/// SIGTERM then SIGKILL (after a brief grace period) the given pid. Returns
/// true if the process exited.
fn kill_pid(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

    let target = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[target]), true);
    let Some(proc) = sys.process(target) else {
        // Already gone.
        return true;
    };

    let _ = proc.kill_with(Signal::Term);

    // Wait up to ~1s for graceful exit.
    let deadline = Instant::now() + Duration::from_millis(1000);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        sys.refresh_processes(ProcessesToUpdate::Some(&[target]), true);
        if sys.process(target).is_none() {
            return true;
        }
    }

    // Fall back to SIGKILL.
    if let Some(proc) = sys.process(target) {
        let _ = proc.kill_with(Signal::Kill);
    }

    // Final check.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        sys.refresh_processes(ProcessesToUpdate::Some(&[target]), true);
        if sys.process(target).is_none() {
            return true;
        }
    }
    false
}

/// Wait for the OAuth port to become rebindable. Returns true if it did.
fn wait_for_port_free(port: u16, max: Duration) -> bool {
    let deadline = Instant::now() + max;
    while Instant::now() < deadline {
        if port_is_free(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(75));
    }
    port_is_free(port)
}

/// Execute the preflight for `codex login`: check the OAuth port, clear it
/// if a stale codex is holding it, and report what we did.
pub fn preflight_codex_oauth_port() -> Result<PreflightOutcome> {
    if port_is_free(CODEX_OAUTH_PORT) {
        return Ok(PreflightOutcome::PortFree);
    }

    let Some(pid) = pid_holding_port(CODEX_OAUTH_PORT)? else {
        return Ok(PreflightOutcome::HeldUnknown);
    };

    let descriptor = describe_pid(pid);
    if !descriptor_looks_like_codex(&descriptor) {
        return Ok(PreflightOutcome::HeldByOther { pid, descriptor });
    }

    if !kill_pid(pid) {
        return Err(anyhow!(
            "identified stale codex pid {pid} on port {CODEX_OAUTH_PORT} but could not terminate it; \
             try `kill -9 {pid}` manually and retry `coven adapter login codex`"
        ));
    }

    if !wait_for_port_free(CODEX_OAUTH_PORT, Duration::from_millis(1500)) {
        return Err(anyhow!(
            "killed stale codex pid {pid} on port {CODEX_OAUTH_PORT} but the port is still held; \
             another process may be using it"
        ));
    }

    Ok(PreflightOutcome::ClearedStaleCodex { killed_pid: pid })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- port_is_free ---

    #[test]
    fn port_is_free_returns_true_for_unbound_high_port() {
        // Pick a high random port we're unlikely to collide with.
        // We use port 0 trick: bind to 0, get the assigned port, drop, then
        // probe — should still be free briefly.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let assigned = listener.local_addr().expect("local_addr").port();
        drop(listener);
        assert!(
            port_is_free(assigned),
            "ephemeral port should be free after drop"
        );
    }

    #[test]
    fn port_is_free_returns_false_when_held() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let assigned = listener.local_addr().expect("local_addr").port();
        assert!(
            !port_is_free(assigned),
            "port {assigned} should be in use while listener is alive"
        );
        // Keep listener alive to end of test
        drop(listener);
    }

    // --- descriptor_looks_like_codex ---

    #[test]
    fn descriptor_recognizes_codex_binary() {
        assert!(descriptor_looks_like_codex("codex (codex login)"));
        assert!(descriptor_looks_like_codex(
            "node (/opt/homebrew/lib/node_modules/@openai/codex/bin/codex.js login)"
        ));
        assert!(descriptor_looks_like_codex("codex.js (codex)"));
    }

    #[test]
    fn descriptor_rejects_unrelated_processes() {
        assert!(!descriptor_looks_like_codex(""));
        assert!(!descriptor_looks_like_codex("nginx (worker process)"));
        assert!(!descriptor_looks_like_codex("python (server.py)"));
        // Substring 'codex' inside another token must not match
        assert!(!descriptor_looks_like_codex("vscodex (some script)"));
        assert!(!descriptor_looks_like_codex("not-codex-tool (start)"));
    }

    #[test]
    fn descriptor_handles_path_separators() {
        // Tokens are split on whitespace and slashes; recognize codex in a
        // path like /usr/local/bin/codex
        assert!(descriptor_looks_like_codex(
            "node (/usr/local/bin/codex login)"
        ));
    }

    // --- pid_holding_port ---

    #[test]
    fn pid_holding_port_returns_none_when_port_is_free() {
        // Get a port nobody's using.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let assigned = listener.local_addr().expect("local_addr").port();
        drop(listener);
        // If lsof isn't installed (CI sandbox), we just get None too — same
        // observable result.
        let result = pid_holding_port(assigned).expect("pid_holding_port should not error");
        assert!(result.is_none(), "port {assigned} should have no holder");
    }

    // --- preflight_codex_oauth_port (port-free path) ---

    #[test]
    fn preflight_returns_port_free_when_1455_unbound() {
        // This test only runs cleanly if port 1455 happens to be free.
        // Skip silently if it's occupied — we don't want to flake CI.
        if !port_is_free(CODEX_OAUTH_PORT) {
            eprintln!(
                "skipping test: port {CODEX_OAUTH_PORT} is in use, can't test the port-free path"
            );
            return;
        }
        let outcome = preflight_codex_oauth_port().expect("preflight should not error");
        assert_eq!(outcome, PreflightOutcome::PortFree);
    }
}
