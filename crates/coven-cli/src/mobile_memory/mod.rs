pub mod assurance;
pub mod audit;
pub mod auth;
pub mod config;
pub mod contract;
pub mod device;
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
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
        LazyLock, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

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
    state: LocalPairingState,
    phrase: Option<[String; 6]>,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LocalPairingState {
    WaitingForDevice,
    WaitingForConfirmation,
    Completed,
    Cancelled,
    Expired,
    Unavailable,
}

#[cfg(unix)]
#[derive(Deserialize)]
struct LocalPairingCancellation {
    state: LocalPairingState,
}

#[cfg(unix)]
static PAIRING_INTERRUPT_REQUESTED: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
static PAIRING_INTERRUPT_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[cfg(unix)]
extern "C" fn request_pairing_interrupt(_signal: libc::c_int) {
    PAIRING_INTERRUPT_REQUESTED.store(true, Ordering::Release);
}

#[cfg(unix)]
struct PairingInterruptGuard {
    _lock: MutexGuard<'static, ()>,
    previous_handler: libc::sigaction,
    previous_mask: libc::sigset_t,
}

#[cfg(unix)]
impl PairingInterruptGuard {
    fn install() -> Result<Self> {
        let lock = PAIRING_INTERRUPT_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile pairing interrupt lock was poisoned"))?;
        let mut signal_set = unsafe { std::mem::zeroed() };
        let mut previous_mask = unsafe { std::mem::zeroed() };
        // SAFETY: the set is initialized before use and contains only SIGINT.
        unsafe {
            libc::sigemptyset(&mut signal_set);
            libc::sigaddset(&mut signal_set, libc::SIGINT);
        }
        // Block SIGINT while replacing its process-wide disposition so no
        // interrupt can land between handler installation and state reset.
        let mask_result =
            unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signal_set, &mut previous_mask) };
        if mask_result != 0 {
            return Err(std::io::Error::from_raw_os_error(mask_result))
                .context("failed to block mobile pairing interrupt");
        }

        PAIRING_INTERRUPT_REQUESTED.store(false, Ordering::Release);
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = request_pairing_interrupt as *const () as usize;
        // SAFETY: `action` is initialized, the handler only stores to an
        // atomic, and `previous_handler` remains valid until restoration.
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
        }
        action.sa_flags = 0;
        let mut previous_handler = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(libc::SIGINT, &action, &mut previous_handler) } != 0 {
            let error = std::io::Error::last_os_error();
            let _ = unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut())
            };
            return Err(error).context("failed to install mobile pairing interrupt handler");
        }
        let restore_result = unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut())
        };
        if restore_result != 0 {
            let error = std::io::Error::from_raw_os_error(restore_result);
            let _ =
                unsafe { libc::sigaction(libc::SIGINT, &previous_handler, std::ptr::null_mut()) };
            return Err(error).context("failed to activate mobile pairing interrupt handler");
        }
        Ok(Self {
            _lock: lock,
            previous_handler,
            previous_mask,
        })
    }

    fn interrupted(&self) -> bool {
        PAIRING_INTERRUPT_REQUESTED.load(Ordering::Acquire)
    }
}

#[cfg(unix)]
impl Drop for PairingInterruptGuard {
    fn drop(&mut self) {
        let mut signal_set = unsafe { std::mem::zeroed() };
        // SAFETY: SIGINT is blocked while its prior disposition is restored.
        unsafe {
            libc::sigemptyset(&mut signal_set);
            libc::sigaddset(&mut signal_set, libc::SIGINT);
            let _ = libc::pthread_sigmask(libc::SIG_BLOCK, &signal_set, std::ptr::null_mut());
            let _ = libc::sigaction(libc::SIGINT, &self.previous_handler, std::ptr::null_mut());
            PAIRING_INTERRUPT_REQUESTED.store(false, Ordering::Release);
            let _ =
                libc::pthread_sigmask(libc::SIG_SETMASK, &self.previous_mask, std::ptr::null_mut());
        }
    }
}

#[cfg(unix)]
struct ActivePairingCancellation<'a> {
    coven_home: &'a Path,
    pairing_id: Uuid,
    active: bool,
}

#[cfg(unix)]
impl ActivePairingCancellation<'_> {
    fn cancel(&mut self) -> Result<LocalPairingState> {
        let path = format!(
            "/api/v1/internal/mobile/pairings/{}/cancel",
            self.pairing_id
        );
        let (status, body) = post_mobile_control_without_interrupt(self.coven_home, &path, "{}")?;
        if status != 200 {
            bail!("Coven daemon rejected pairing cancellation with HTTP {status}: {body}");
        }
        let cancellation: LocalPairingCancellation =
            serde_json::from_str(&body).context("daemon returned invalid pairing cancellation")?;
        if !matches!(
            cancellation.state,
            LocalPairingState::Cancelled
                | LocalPairingState::Completed
                | LocalPairingState::Expired
        ) {
            bail!(
                "pairing cancellation returned non-terminal state {:?}",
                cancellation.state
            );
        }
        self.active = false;
        Ok(cancellation.state)
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

#[cfg(unix)]
impl Drop for ActivePairingCancellation<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.cancel();
        }
    }
}

#[cfg(unix)]
fn read_pairing_confirmation(interrupt: &PairingInterruptGuard) -> Result<Option<String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let _input = thread::spawn(move || {
        let mut confirmation = String::new();
        let result = std::io::stdin()
            .read_line(&mut confirmation)
            .map(|_| confirmation);
        let _ = sender.send(result);
    });
    loop {
        if interrupt.interrupted() {
            return Ok(None);
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => {
                return result
                    .map(Some)
                    .context("failed to read pairing confirmation")
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                bail!("pairing confirmation reader stopped unexpectedly")
            }
        }
    }
}

#[cfg(unix)]
fn run_pair_unix() -> Result<()> {
    let interrupt = PairingInterruptGuard::install()?;
    let coven_home = crate::coven_home_dir()?;
    let (status, body) = post_mobile_control_without_interrupt(
        &coven_home,
        "/api/v1/internal/mobile/pairings",
        "{}",
    )?;
    if status != 201 {
        bail!("Coven daemon rejected mobile pairing with HTTP {status}: {body}");
    }
    let invitation: LocalPairingInvitation =
        serde_json::from_str(&body).context("daemon returned an invalid pairing invitation")?;
    let mut cancellation = ActivePairingCancellation {
        coven_home: &coven_home,
        pairing_id: invitation.id,
        active: true,
    };
    let result = (|| -> Result<()> {
        println!("{}", invitation.terminal_output);

        let phrase = loop {
            if interrupt.interrupted() {
                bail!("mobile pairing was interrupted");
            }
            if Utc::now() >= invitation.expires_at {
                bail!("mobile pairing expired before the device enrolled");
            }
            let path = format!("/api/v1/internal/mobile/pairings/{}/status", invitation.id);
            let (status, body) = post_mobile_control(&coven_home, &path, "{}")?;
            if status != 200 {
                bail!("Coven daemon rejected pairing status with HTTP {status}: {body}");
            }
            let status: LocalPairingStatus =
                serde_json::from_str(&body).context("daemon returned invalid pairing status")?;
            match status.state {
                LocalPairingState::WaitingForDevice => {}
                LocalPairingState::WaitingForConfirmation => {
                    break status
                        .phrase
                        .context("daemon omitted the pairing confirmation phrase")?;
                }
                LocalPairingState::Completed => {
                    cancellation.disarm();
                    println!("Mobile device paired.");
                    return Ok(());
                }
                LocalPairingState::Cancelled => {
                    cancellation.disarm();
                    bail!("mobile pairing was cancelled");
                }
                LocalPairingState::Expired => {
                    cancellation.disarm();
                    bail!("mobile pairing expired before the device enrolled");
                }
                LocalPairingState::Unavailable => {
                    bail!("mobile pairing is unavailable after rejected enrollment");
                }
            }
            thread::sleep(Duration::from_millis(250));
        };

        println!("\nCompare these words with the device:");
        for (index, word) in phrase.iter().enumerate() {
            println!("{}. {word}", index + 1);
        }
        println!("Type `confirm` only if all six words match:");
        let Some(confirmation) = read_pairing_confirmation(&interrupt)? else {
            bail!("mobile pairing was interrupted");
        };
        if confirmation.trim() != "confirm" {
            let state = cancellation
                .cancel()
                .context("host declined pairing and cancellation failed")?;
            match state {
                LocalPairingState::Cancelled => {
                    println!("Mobile pairing cancelled.");
                    return Ok(());
                }
                LocalPairingState::Completed => {
                    bail!("mobile pairing completed before host decline could cancel it");
                }
                LocalPairingState::Expired => {
                    bail!("mobile pairing expired before host decline could cancel it");
                }
                state => bail!("pairing cancellation returned non-terminal state {state:?}"),
            }
        }
        if interrupt.interrupted() {
            bail!("mobile pairing was interrupted");
        }
        let path = format!("/api/v1/internal/mobile/pairings/{}/confirm", invitation.id);
        let body = serde_json::json!({ "phrase": phrase }).to_string();
        // Once sent, learn whether the daemon accepted confirmation before
        // reacting to SIGINT; acceptance preserves the device's remaining window.
        let (status, response) = post_mobile_control_without_interrupt(&coven_home, &path, &body)?;
        match status {
            200 => {
                cancellation.disarm();
                println!("Mobile device paired.");
                Ok(())
            }
            409 => {
                cancellation.disarm();
                println!("Host confirmed. Complete confirmation on the device before it expires.");
                Ok(())
            }
            _ => bail!("Coven daemon rejected host confirmation with HTTP {status}: {response}"),
        }
    })();

    match result {
        Err(error) if cancellation.active => match cancellation.cancel() {
            Ok(LocalPairingState::Completed) => {
                Err(error.context("mobile pairing completed before cancellation could take effect"))
            }
            Ok(LocalPairingState::Cancelled) => {
                Err(error.context("incomplete mobile pairing was cancelled"))
            }
            Ok(LocalPairingState::Expired) => {
                Err(error.context("incomplete mobile pairing had already expired"))
            }
            Ok(state) => Err(error.context(format!(
                "mobile pairing cleanup returned non-terminal state {state:?}"
            ))),
            Err(cancel_error) => Err(anyhow::anyhow!(
                "{error:#}; cancelling incomplete mobile pairing failed: {cancel_error:#}"
            )),
        },
        result => result,
    }
}

#[cfg(unix)]
fn post_mobile_control(coven_home: &Path, path: &str, body: &str) -> Result<(u16, String)> {
    post_mobile_control_inner(coven_home, path, body, true)
}

#[cfg(unix)]
fn post_mobile_control_without_interrupt(
    coven_home: &Path,
    path: &str,
    body: &str,
) -> Result<(u16, String)> {
    post_mobile_control_inner(coven_home, path, body, false)
}

#[cfg(unix)]
fn post_mobile_control_inner(
    coven_home: &Path,
    path: &str,
    body: &str,
    observe_interrupt: bool,
) -> Result<(u16, String)> {
    use std::os::unix::net::UnixStream;

    const IO_POLL_INTERVAL: Duration = Duration::from_millis(100);
    const IO_DEADLINE: Duration = Duration::from_secs(5);
    const MAX_CONTROL_RESPONSE_BYTES: usize = MAX_MOBILE_RESPONSE_BYTES + 16 * 1024;

    let socket = crate::daemon::daemon_socket_path(coven_home);
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
        .set_write_timeout(Some(IO_POLL_INTERVAL))
        .context("failed to bound mobile pairing control writes")?;
    stream
        .set_read_timeout(Some(IO_POLL_INTERVAL))
        .context("failed to bound mobile pairing control reads")?;
    if observe_interrupt && PAIRING_INTERRUPT_REQUESTED.load(Ordering::Acquire) {
        bail!("mobile pairing was interrupted");
    }
    stream
        .write_all(request.as_bytes())
        .context("failed to write mobile pairing control request")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to finish mobile pairing control request")?;
    let deadline = Instant::now() + IO_DEADLINE;
    let mut response = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        if observe_interrupt && PAIRING_INTERRUPT_REQUESTED.load(Ordering::Acquire) {
            bail!("mobile pairing was interrupted");
        }
        if Instant::now() >= deadline {
            bail!("timed out reading mobile pairing control response");
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if response.len().saturating_add(read) > MAX_CONTROL_RESPONSE_BYTES {
                    bail!("mobile pairing control response exceeded the size limit");
                }
                response.extend_from_slice(&chunk[..read]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                return Err(error).context("failed to read mobile pairing control response")
            }
        }
    }
    let response =
        String::from_utf8(response).context("mobile pairing control response was not UTF-8")?;
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
