pub mod audit;
pub mod auth;
pub mod config;
pub mod contract;
pub mod gateway;
pub mod grant;
pub mod identity;
pub mod pairing;
pub mod registry;

use std::net::SocketAddr;
#[cfg(unix)]
use std::{
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

#[cfg(unix)]
use self::pairing::PairingState;

pub const MOBILE_PROTOCOL_VERSION: u16 = 1;
pub const MAX_MOBILE_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_MOBILE_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MOBILE_REQUEST_WINDOW_SECONDS: i64 = 300;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MobileGatewayStatus {
    configured: bool,
    enabled: bool,
    bind: Option<String>,
    advertised_endpoint: Option<String>,
    device_count: usize,
    active_device_count: usize,
    revoked_device_count: usize,
}

pub fn run_enable(bind: SocketAddr, endpoint: &str) -> Result<()> {
    let config = config::MobileGatewayConfig {
        enabled: true,
        bind,
        advertised_endpoint: endpoint.to_owned(),
    };
    config::validate_mobile_config(&config)?;
    let endpoint = Url::parse(endpoint).context("mobile gateway endpoint must be a valid URL")?;
    let subject_alt_name = endpoint
        .host_str()
        .context("mobile gateway endpoint must contain a host")?;
    let coven_home = crate::coven_home_dir()?;
    identity::load_or_create_host_identity(&coven_home, subject_alt_name)?;
    config::save_mobile_config(&coven_home, &config)?;
    println!(
        "Mobile memory access enabled at {}",
        config.advertised_endpoint
    );
    println!("Restart the Coven daemon to apply this listener configuration.");
    Ok(())
}

pub fn run_disable(forget_devices: bool, confirm_forget_devices: bool) -> Result<()> {
    if forget_devices != confirm_forget_devices {
        bail!("forgetting devices requires both --forget-devices and --confirm-forget-devices");
    }
    let coven_home = crate::coven_home_dir()?;
    let was_configured = config::remove_mobile_config(&coven_home)?;
    if forget_devices {
        registry::DeviceRegistry::load(&coven_home)?.forget_all()?;
    }
    println!(
        "Mobile memory access {}.",
        if was_configured {
            "disabled"
        } else {
            "was already disabled"
        }
    );
    if forget_devices {
        println!("All paired mobile devices were forgotten.");
    } else {
        println!("Host identity and paired devices were retained.");
    }
    Ok(())
}

pub fn run_status(json: bool) -> Result<()> {
    let coven_home = crate::coven_home_dir()?;
    let config = config::load_mobile_config(&coven_home)?;
    let devices = registry::DeviceRegistry::load_if_present(&coven_home)?
        .map(|registry| registry.list_status())
        .transpose()?
        .unwrap_or_default();
    let active_device_count = devices
        .iter()
        .filter(|device| device.revoked_at.is_none())
        .count();
    let status = MobileGatewayStatus {
        configured: config.is_some(),
        enabled: config.as_ref().is_some_and(|config| config.enabled),
        bind: config.as_ref().map(|config| config.bind.to_string()),
        advertised_endpoint: config
            .as_ref()
            .map(|config| config.advertised_endpoint.clone()),
        device_count: devices.len(),
        active_device_count,
        revoked_device_count: devices.len() - active_device_count,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "Mobile memory access: {}",
            if status.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        if let Some(endpoint) = status.advertised_endpoint {
            println!("Endpoint: {endpoint}");
        }
        println!(
            "Devices: {} active, {} revoked",
            status.active_device_count, status.revoked_device_count
        );
    }
    Ok(())
}

pub fn run_devices(json: bool) -> Result<()> {
    let coven_home = crate::coven_home_dir()?;
    let devices = registry::DeviceRegistry::load_if_present(&coven_home)?
        .map(|registry| registry.list_status())
        .transpose()?
        .unwrap_or_default();
    if json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
    } else if devices.is_empty() {
        println!("No mobile devices are paired.");
    } else {
        for device in devices {
            let state = if device.revoked_at.is_some() {
                "revoked"
            } else {
                "active"
            };
            println!("{}\t{}\t{}", device.id, state, device.display_name);
        }
    }
    Ok(())
}

pub fn run_revoke_device(device_id: Uuid) -> Result<()> {
    let coven_home = crate::coven_home_dir()?;
    let registry = registry::DeviceRegistry::load_if_present(&coven_home)?
        .context("no mobile devices are paired")?;
    registry.revoke(device_id, Utc::now())?;
    audit::append_event(
        &coven_home,
        Utc::now(),
        audit::MobileAuditEvent::DeviceRevoked,
        Some(device_id),
    )?;
    println!("Revoked mobile device {device_id}.");
    Ok(())
}

pub fn run_pair() -> Result<()> {
    #[cfg(not(unix))]
    {
        bail!("mobile pairing control is not implemented on this platform")
    }
    #[cfg(unix)]
    {
        run_pair_unix()
    }
}

#[cfg(unix)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalPairingInvitation {
    id: Uuid,
    terminal_output: String,
    expires_at: DateTime<Utc>,
}

#[cfg(unix)]
#[derive(Deserialize)]
struct LocalPairingStatus {
    state: PairingState,
    phrase: Option<[String; 6]>,
}

#[cfg(unix)]
static PAIRING_INTERRUPT_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_pairing_interrupt(sig: libc::c_int) {
    // Only async-signal-safe work belongs here. AtomicBool uses a lock-free
    // primitive on Coven's supported Unix targets; no allocation, lock, or
    // destructor runs in the handler itself. (Same pattern as the daemon's
    // termination handler.)
    PAIRING_INTERRUPT_REQUESTED.store(true, Ordering::Release);
    let _ = sig;
}

#[cfg(unix)]
fn current_sigint_disposition() -> Result<libc::sigaction> {
    // SAFETY: sigaction with a null act queries the current disposition for
    // SIGINT into the caller-provided storage; it changes nothing.
    unsafe {
        let mut previous: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(
            libc::SIGINT,
            std::ptr::null(),
            &mut previous,
        ) != 0
        {
            return Err(std::io::Error::last_os_error())
                .context("failed to query the current SIGINT disposition");
        }
        Ok(previous)
    }
}

#[cfg(unix)]
fn install_pairing_interrupt_handler() -> Result<()> {
    // SAFETY: sigaction is the documented POSIX API for installing signal
    // handlers; we pass a zero-initialized struct, our handler pointer, and
    // an empty signal mask. Failure returns -1 and sets errno.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handle_pairing_interrupt as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        // Intentionally no SA_RESTART: blocking stdin reads observe the
        // interrupt as ErrorKind::Interrupted instead of waiting it out, so
        // the pairing prompt can react to Ctrl-C instead of retrying.
        action.sa_flags = 0;
        if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to install the pairing interrupt handler");
        }
    }
    Ok(())
}

/// Scoped owner of the process-global pairing SIGINT state.
///
/// Installing a signal handler is a process-global mutation, so it is undone
/// on scope exit: the previous SIGINT disposition captured at install time is
/// restored and the latched flag is reset. Every path out of the pairing flow
/// — success, error, or panic — passes through [`Drop::drop`], so the handler
/// never outlives the pairing session it belongs to.
#[cfg(unix)]
struct PairingInterruptGuard {
    previous: libc::sigaction,
}

#[cfg(unix)]
impl PairingInterruptGuard {
    fn install() -> Result<Self> {
        let previous = current_sigint_disposition()?;
        install_pairing_interrupt_handler()?;
        Ok(Self { previous })
    }
}

#[cfg(unix)]
impl Drop for PairingInterruptGuard {
    fn drop(&mut self) {
        // SAFETY: sigaction restores the disposition captured when the guard
        // was installed; the pointer refers to guard-owned storage.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.previous, std::ptr::null_mut());
        }
        PAIRING_INTERRUPT_REQUESTED.store(false, Ordering::Release);
    }
}

/// True when a SIGINT arrived since the last reset.
#[cfg(unix)]
fn pairing_interrupted() -> bool {
    PAIRING_INTERRUPT_REQUESTED.load(Ordering::Acquire)
}

/// Read one line from `input`, reacting to SIGINT instead of retrying past it.
///
/// The interrupt handler is installed without SA_RESTART, so an interrupted
/// read surfaces as `ErrorKind::Interrupted` rather than being restarted
/// in place; this loop treats every such error as a chance to observe the
/// flag and, once it is set, to stop reading entirely. The caller must then
/// cancel the pairing and exit immediately — no further input is consumed.
/// Ok(None) means the pairing was interrupted; Ok(Some(line)) is the input
/// read so far, including the empty line at EOF.
#[cfg(unix)]
fn read_confirmation_line(
    input: &mut dyn std::io::BufRead,
    interrupt: &AtomicBool,
) -> std::io::Result<Option<String>> {
    let mut buffer = String::new();
    loop {
        if interrupt.load(Ordering::Acquire) {
            return Ok(None);
        }
        buffer.clear();
        match input.read_line(&mut buffer) {
            Ok(_) => return Ok(Some(buffer)),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Best-effort cancellation of a pending pairing through the owner-only daemon
/// control route. If the daemon is unreachable, its in-memory pending pairings
/// are already gone, so a failed request never leaves a live pairing behind.
#[cfg(unix)]
fn cancel_pending_pairing(coven_home: &Path, pairing_id: Uuid) -> &'static str {
    let path = format!("/api/v1/internal/mobile/pairings/{pairing_id}/cancel");
    let Ok((200, body)) = post_mobile_control(coven_home, &path, "{}") else {
        return "the pending pairing could not be confirmed cancelled with the daemon";
    };
    match serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| value["state"].as_str().map(str::to_owned))
        .as_deref()
    {
        Some("cancelled") => "the pending pairing was cancelled",
        Some("already_completed") => {
            "the pairing had already completed; the paired device was kept"
        }
        Some("already_terminal") => "the pairing was already cancelled or expired",
        _ => "the pending pairing state was not reported by the daemon",
    }
}

/// Owner-local pairing control capabilities as advertised by the daemon.
///
/// The field set is additive: older daemons do not serve the capabilities
/// route at all (which this CLI treats as "no negotiated capability"), and
/// future daemons may add fields this CLI ignores.
#[cfg(unix)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalPairingCapabilities {
    api_version: u16,
    pairing_cancellation: bool,
}

/// Negotiate the pairing control API with the daemon before creating a
/// pairing.
///
/// A `coven mobile pair` process can be newer than the daemon it reaches, and
/// the fail-closed flow below depends on the daemon-side cancel route: without
/// it, an interrupt or error would abandon a live pairing until its deadline.
/// The CLI therefore refuses to create a pairing unless the daemon reports the
/// control API version this CLI was built against and declares the
/// cancellation capability. Any transport, status, or parse failure fails
/// negotiation the same way — a restart of the daemon recovers.
#[cfg(unix)]
fn validate_pairing_capabilities(status: u16, body: &str) -> Result<()> {
    if status != 200 {
        bail!(
            "Coven daemon does not advertise mobile pairing capabilities (HTTP {status}); \
             restart the daemon and retry"
        );
    }
    let capabilities: LocalPairingCapabilities = serde_json::from_str(body)
        .context("daemon returned invalid mobile pairing capabilities")?;
    if capabilities.api_version != gateway::LOCAL_PAIRING_CONTROL_API_VERSION {
        bail!(
            "Coven daemon pairing control API version {} is not supported (expected {}); \
             restart the daemon and retry",
            capabilities.api_version,
            gateway::LOCAL_PAIRING_CONTROL_API_VERSION
        );
    }
    if !capabilities.pairing_cancellation {
        bail!(
            "Coven daemon does not support fail-closed pairing cancellation; \
             restart the daemon and retry"
        );
    }
    Ok(())
}

/// Cancels the pending pairing if the flow exits without reaching a terminal
/// state or handing confirmation to the device.
///
/// Armed immediately after the daemon accepts pairing creation, this guard
/// centralizes cleanup for every unexpected nonterminal exit — transport and
/// HTTP failures, malformed status responses, unreadable stdin — so no error
/// path can leave a live pairing behind. Paths that end in a terminal state
/// (the pairing expired or was cancelled) or that hand off to the device
/// (confirmation accepted) disarm the guard first.
#[cfg(unix)]
struct PendingPairingCleanup {
    armed: bool,
    cancel: Option<Box<dyn FnOnce() -> &'static str>>,
}

#[cfg(unix)]
impl PendingPairingCleanup {
    fn armed(coven_home: &Path, pairing_id: Uuid) -> Self {
        let coven_home = coven_home.to_path_buf();
        Self {
            armed: true,
            cancel: Some(Box::new(move || {
                cancel_pending_pairing(&coven_home, pairing_id)
            })),
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(unix)]
impl Drop for PendingPairingCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(cancel) = self.cancel.take() {
            let outcome = cancel();
            eprintln!("coven mobile: pairing abandoned without confirmation; {outcome}");
        }
    }
}

#[cfg(unix)]
fn run_pair_unix() -> Result<()> {
    let coven_home = crate::coven_home_dir()?;
    // The handler is restored and the flag reset when this scope exits, on
    // every path out of the pairing flow.
    let _interrupt_guard = PairingInterruptGuard::install()?;
    // Negotiate before creating: a daemon that cannot fail closed must never
    // be handed a live pairing.
    let (status, body) = request_mobile_control(
        &coven_home,
        "GET",
        "/api/v1/internal/mobile/capabilities",
        "",
    )?;
    validate_pairing_capabilities(status, &body)?;
    let (status, body) =
        post_mobile_control(&coven_home, "/api/v1/internal/mobile/pairings", "{}")?;
    if status != 201 {
        bail!("Coven daemon rejected mobile pairing with HTTP {status}: {body}");
    }
    let invitation: LocalPairingInvitation =
        serde_json::from_str(&body).context("daemon returned an invalid pairing invitation")?;
    println!("{}", invitation.terminal_output);
    // Every unexpected exit from here on retires the pairing; the guard is
    // disarmed only by a terminal state or an accepted confirmation handoff.
    // (A creation response whose invitation cannot be parsed never yields the
    // pairing id, so it cannot be addressed; such a pairing relies on the
    // daemon's own deadline, five minutes out.)
    let mut cleanup = PendingPairingCleanup::armed(&coven_home, invitation.id);

    let phrase = loop {
        if Utc::now() >= invitation.expires_at {
            cleanup.disarm();
            bail!("mobile pairing expired before the device enrolled");
        }
        if pairing_interrupted() {
            let outcome = cancel_pending_pairing(&coven_home, invitation.id);
            cleanup.disarm();
            bail!("mobile pairing interrupted before the device enrolled; {outcome}");
        }
        let path = format!("/api/v1/internal/mobile/pairings/{}/status", invitation.id);
        // Transport, HTTP, and parse failures here leave the pairing live in
        // the daemon, so they fall through to the cleanup guard on bail.
        let (status, body) = post_mobile_control(&coven_home, &path, "{}")?;
        if status != 200 {
            bail!("Coven daemon rejected pairing status with HTTP {status}: {body}");
        }
        let status: LocalPairingStatus =
            serde_json::from_str(&body).context("daemon returned invalid pairing status")?;
        match status.state {
            // Terminal states need no cleanup: the pairing can no longer be
            // enrolled or confirmed.
            PairingState::Cancelled => {
                cleanup.disarm();
                bail!("mobile pairing was cancelled before completion")
            }
            PairingState::Expired => {
                cleanup.disarm();
                bail!("mobile pairing expired before the device enrolled")
            }
            _ => {}
        }
        if let Some(phrase) = status.phrase {
            break phrase;
        }
        thread::sleep(Duration::from_millis(250));
    };

    println!("\nCompare these words with the device:");
    for (index, word) in phrase.iter().enumerate() {
        println!("{}. {word}", index + 1);
    }
    println!("Type `confirm` only if all six words match:");
    let stdin = std::io::stdin();
    let confirmation = match read_confirmation_line(&mut stdin.lock(), &PAIRING_INTERRUPT_REQUESTED)
    {
        Ok(Some(confirmation)) => confirmation,
        // An interrupt while waiting for input cancels the pairing and exits
        // immediately; nothing else is read or sent.
        Ok(None) => {
            let outcome = cancel_pending_pairing(&coven_home, invitation.id);
            cleanup.disarm();
            bail!("mobile pairing interrupted before host confirmation; {outcome}");
        }
        // The read itself failed, but an interrupt observed now still wins:
        // the pairing is cancelled and the process exits immediately.
        Err(_error) if pairing_interrupted() => {
            let outcome = cancel_pending_pairing(&coven_home, invitation.id);
            cleanup.disarm();
            bail!("mobile pairing interrupted before host confirmation; {outcome}");
        }
        // A stdin failure without an interrupt is an unexpected nonterminal
        // exit; the cleanup guard retires the pairing on the way out.
        Err(error) => return Err(error).context("failed to read pairing confirmation"),
    };
    if confirmation.trim() != "confirm" {
        let interrupted = pairing_interrupted();
        let outcome = cancel_pending_pairing(&coven_home, invitation.id);
        cleanup.disarm();
        if interrupted {
            bail!("mobile pairing interrupted; {outcome}");
        }
        bail!("mobile pairing declined; {outcome}");
    }
    // Recheck the interrupt state immediately before confirmation: a Ctrl-C
    // that races with the user pressing Enter must not let the pairing be
    // confirmed.
    if pairing_interrupted() {
        let outcome = cancel_pending_pairing(&coven_home, invitation.id);
        cleanup.disarm();
        bail!("mobile pairing interrupted before host confirmation; {outcome}");
    }
    let path = format!("/api/v1/internal/mobile/pairings/{}/confirm", invitation.id);
    let body = serde_json::json!({ "phrase": phrase }).to_string();
    // A failed confirmation request leaves the pairing live, so failures here
    // also fall through to the cleanup guard.
    let (status, response) = post_mobile_control(&coven_home, &path, &body)?;
    match status {
        200 => {
            cleanup.disarm();
            println!("Mobile device paired.");
            Ok(())
        }
        409 => {
            cleanup.disarm();
            println!("Host confirmed. Complete confirmation on the device before it expires.");
            Ok(())
        }
        _ => bail!("Coven daemon rejected host confirmation with HTTP {status}: {response}"),
    }
}

#[cfg(unix)]
fn post_mobile_control(coven_home: &Path, path: &str, body: &str) -> Result<(u16, String)> {
    request_mobile_control(coven_home, "POST", path, body)
}

#[cfg(unix)]
fn request_mobile_control(
    coven_home: &Path,
    method: &str,
    path: &str,
    body: &str,
) -> Result<(u16, String)> {
    use std::os::unix::net::UnixStream;

    let socket = crate::daemon::daemon_socket_path(coven_home);
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let mut stream = UnixStream::connect(&socket).with_context(|| {
        format!(
            "failed to connect to Coven daemon socket {}; start or restart the daemon after enabling mobile memory",
            socket.display()
        )
    })?;
    stream
        .write_all(request.as_bytes())
        .context("failed to write mobile pairing control request")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to finish mobile pairing control request")?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("failed to read mobile pairing control response")?;
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .context("invalid mobile pairing control response")?;
    let (_, body) = response
        .split_once("\r\n\r\n")
        .context("mobile pairing control response omitted a body")?;
    Ok((status, body.to_owned()))
}

#[cfg(all(test, unix))]
mod pairing_flow_tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{BufRead, Read};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    /// Stdin stand-in whose reads replay a scripted sequence of byte chunks
    /// and I/O errors, so the confirmation loop can be driven without a tty.
    struct ScriptedInput {
        steps: VecDeque<std::io::Result<Vec<u8>>>,
        buffer: Vec<u8>,
        position: usize,
    }

    impl ScriptedInput {
        fn new(steps: Vec<std::io::Result<Vec<u8>>>) -> Self {
            Self {
                steps: steps.into(),
                buffer: Vec::new(),
                position: 0,
            }
        }
    }

    impl Read for ScriptedInput {
        fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
            let available = self.fill_buf()?;
            let amount = available.len().min(target.len());
            target[..amount].copy_from_slice(&available[..amount]);
            self.consume(amount);
            Ok(amount)
        }
    }

    impl BufRead for ScriptedInput {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            if self.position >= self.buffer.len() {
                match self.steps.pop_front() {
                    Some(Ok(chunk)) => {
                        self.buffer = chunk;
                        self.position = 0;
                    }
                    Some(Err(error)) => return Err(error),
                    None => {
                        self.buffer.clear();
                        self.position = 0;
                    }
                }
            }
            Ok(&self.buffer[self.position..])
        }

        fn consume(&mut self, amount: usize) {
            self.position = (self.position + amount).min(self.buffer.len());
        }
    }

    fn interrupted_error() -> std::io::Error {
        std::io::Error::from_raw_os_error(libc::EINTR)
    }

    #[test]
    fn interrupt_guard_restores_disposition_and_resets_flag() {
        let before = current_sigint_disposition().unwrap();
        let guard = PairingInterruptGuard::install().unwrap();

        let installed = current_sigint_disposition().unwrap();
        assert_eq!(
            installed.sa_sigaction,
            handle_pairing_interrupt as *const () as usize
        );

        // The handler latches the flag; the guard must reset it and restore
        // the previous disposition on scope exit.
        handle_pairing_interrupt(libc::SIGINT);
        assert!(pairing_interrupted());

        drop(guard);
        let after = current_sigint_disposition().unwrap();
        assert!(!pairing_interrupted());
        assert_eq!(after.sa_sigaction, before.sa_sigaction);
        assert_eq!(after.sa_flags, before.sa_flags);
    }

    #[test]
    fn interrupt_guard_restore_survives_a_double_install() {
        let before = current_sigint_disposition().unwrap();
        let outer = PairingInterruptGuard::install().unwrap();
        let inner = PairingInterruptGuard::install().unwrap();
        drop(inner);
        // The outer guard still owns the restored-from state; dropping it
        // returns to the disposition observed before either install.
        drop(outer);
        let after = current_sigint_disposition().unwrap();
        assert_eq!(after.sa_sigaction, before.sa_sigaction);
        assert!(!pairing_interrupted());
    }

    #[test]
    fn confirmation_reader_returns_the_line_without_an_interrupt() {
        let mut input =
            ScriptedInput::new(vec![Err(interrupted_error()), Ok(b"confirm\n".to_vec())]);
        let interrupt = AtomicBool::new(false);
        let line = read_confirmation_line(&mut input, &interrupt).unwrap();
        assert_eq!(line.as_deref(), Some("confirm\n"));
    }

    #[test]
    fn confirmation_reader_stops_before_reading_when_already_interrupted() {
        let mut input = ScriptedInput::new(vec![Ok(b"confirm\n".to_vec())]);
        let interrupt = AtomicBool::new(true);
        // The pending line must not be consumed: an interrupted pairing is
        // cancelled and exits before any further input is read.
        let line = read_confirmation_line(&mut input, &interrupt).unwrap();
        assert_eq!(line, None);
        assert_eq!(input.fill_buf().unwrap(), b"confirm\n");
    }

    #[test]
    fn confirmation_reader_observes_an_interrupt_that_races_the_read() {
        let mut input = ScriptedInput::new(vec![
            Err(interrupted_error()),
            Ok(b"confirm\n".to_vec()),
        ]);
        let interrupt = AtomicBool::new(false);
        interrupt.store(true, Ordering::Release);
        let line = read_confirmation_line(&mut input, &interrupt).unwrap();
        assert_eq!(line, None);
    }

    #[test]
    fn confirmation_reader_propagates_unrelated_errors() {
        let mut input = ScriptedInput::new(vec![Err(std::io::Error::other("closed"))]);
        let interrupt = AtomicBool::new(false);
        let error = read_confirmation_line(&mut input, &interrupt).unwrap_err();
        assert_eq!(error.to_string(), "closed");
    }

    #[test]
    fn confirmation_reader_treats_eof_as_an_empty_line() {
        let mut input = ScriptedInput::new(vec![]);
        let interrupt = AtomicBool::new(false);
        let line = read_confirmation_line(&mut input, &interrupt).unwrap();
        assert_eq!(line.as_deref(), Some(""));
    }

    #[test]
    fn pending_pairing_cleanup_cancels_once_when_still_armed_on_drop() {
        let cancellations = Arc::new(AtomicUsize::new(0));
        let cleanup = PendingPairingCleanup {
            armed: true,
            cancel: Some(Box::new({
                let cancellations = Arc::clone(&cancellations);
                move || {
                    cancellations.fetch_add(1, Ordering::AcqRel);
                    "the pending pairing was cancelled"
                }
            })),
        };
        drop(cleanup);
        assert_eq!(cancellations.load(Ordering::Acquire), 1);
    }

    #[test]
    fn pending_pairing_cleanup_does_nothing_after_disarm() {
        let cancellations = Arc::new(AtomicUsize::new(0));
        let mut cleanup = PendingPairingCleanup {
            armed: true,
            cancel: Some(Box::new({
                let cancellations = Arc::clone(&cancellations);
                move || {
                    cancellations.fetch_add(1, Ordering::AcqRel);
                    "the pending pairing was cancelled"
                }
            })),
        };
        // Terminal state or accepted handoff: the pairing is no longer live,
        // so dropping the guard must not touch the daemon.
        cleanup.disarm();
        drop(cleanup);
        assert_eq!(cancellations.load(Ordering::Acquire), 0);
    }

    #[test]
    fn pairing_capabilities_reject_an_unsupported_daemon() {
        let supported = serde_json::json!({
            "apiVersion": gateway::LOCAL_PAIRING_CONTROL_API_VERSION,
            "pairingCancellation": true,
        })
        .to_string();
        validate_pairing_capabilities(200, &supported).unwrap();

        // An older daemon does not serve the route at all.
        let error = validate_pairing_capabilities(404, "not found").unwrap_err();
        assert!(error.to_string().contains("capabilities"));

        // A daemon speaking a different control API version.
        let future = serde_json::json!({
            "apiVersion": gateway::LOCAL_PAIRING_CONTROL_API_VERSION + 1,
            "pairingCancellation": true,
        })
        .to_string();
        assert!(validate_pairing_capabilities(200, &future)
            .unwrap_err()
            .to_string()
            .contains("API version"));

        // A daemon without the fail-closed cancellation capability.
        let unable = serde_json::json!({
            "apiVersion": gateway::LOCAL_PAIRING_CONTROL_API_VERSION,
            "pairingCancellation": false,
        })
        .to_string();
        assert!(validate_pairing_capabilities(200, &unable)
            .unwrap_err()
            .to_string()
            .contains("fail-closed"));

        // A malformed capabilities body fails negotiation rather than
        // defaulting to "supported".
        assert!(validate_pairing_capabilities(200, "{}").is_err());
    }
}
