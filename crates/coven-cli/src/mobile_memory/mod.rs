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
fn install_pairing_interrupt_handler() -> Result<()> {
    // SAFETY: sigaction is the documented POSIX API for installing signal
    // handlers; we pass a zero-initialized struct, our handler pointer, and
    // an empty signal mask. Failure returns -1 and sets errno.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handle_pairing_interrupt as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        // Intentionally no SA_RESTART: blocking stdin reads observe the
        // interrupt instead of waiting it out, and std retries the
        // interrupted read so the prompt keeps working.
        action.sa_flags = 0;
        if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to install the pairing interrupt handler");
        }
    }
    Ok(())
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

#[cfg(unix)]
fn run_pair_unix() -> Result<()> {
    let coven_home = crate::coven_home_dir()?;
    // The handler stays installed for the lifetime of this pairing flow; the
    // command runs to process exit, so no restore step is needed.
    install_pairing_interrupt_handler()?;
    let (status, body) =
        post_mobile_control(&coven_home, "/api/v1/internal/mobile/pairings", "{}")?;
    if status != 201 {
        bail!("Coven daemon rejected mobile pairing with HTTP {status}: {body}");
    }
    let invitation: LocalPairingInvitation =
        serde_json::from_str(&body).context("daemon returned an invalid pairing invitation")?;
    println!("{}", invitation.terminal_output);

    let phrase = loop {
        if Utc::now() >= invitation.expires_at {
            bail!("mobile pairing expired before the device enrolled");
        }
        if PAIRING_INTERRUPT_REQUESTED.load(Ordering::Acquire) {
            let outcome = cancel_pending_pairing(&coven_home, invitation.id);
            bail!("mobile pairing interrupted before the device enrolled; {outcome}");
        }
        let path = format!("/api/v1/internal/mobile/pairings/{}/status", invitation.id);
        let (status, body) = post_mobile_control(&coven_home, &path, "{}")?;
        if status != 200 {
            bail!("Coven daemon rejected pairing status with HTTP {status}: {body}");
        }
        let status: LocalPairingStatus =
            serde_json::from_str(&body).context("daemon returned invalid pairing status")?;
        match status.state {
            PairingState::Cancelled => bail!("mobile pairing was cancelled before completion"),
            PairingState::Expired => bail!("mobile pairing expired before the device enrolled"),
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
    let mut confirmation = String::new();
    if let Err(error) = std::io::stdin().read_line(&mut confirmation) {
        if PAIRING_INTERRUPT_REQUESTED.load(Ordering::Acquire) {
            let outcome = cancel_pending_pairing(&coven_home, invitation.id);
            bail!("mobile pairing interrupted before host confirmation; {outcome}");
        }
        return Err(error).context("failed to read pairing confirmation");
    }
    if confirmation.trim() != "confirm" {
        let outcome = cancel_pending_pairing(&coven_home, invitation.id);
        if PAIRING_INTERRUPT_REQUESTED.load(Ordering::Acquire) {
            bail!("mobile pairing interrupted; {outcome}");
        }
        bail!("mobile pairing declined; {outcome}");
    }
    let path = format!("/api/v1/internal/mobile/pairings/{}/confirm", invitation.id);
    let body = serde_json::json!({ "phrase": phrase }).to_string();
    let (status, response) = post_mobile_control(&coven_home, &path, &body)?;
    match status {
        200 => {
            println!("Mobile device paired.");
            Ok(())
        }
        409 => {
            println!("Host confirmed. Complete confirmation on the device before it expires.");
            Ok(())
        }
        _ => bail!("Coven daemon rejected host confirmation with HTTP {status}: {response}"),
    }
}

#[cfg(unix)]
fn post_mobile_control(coven_home: &Path, path: &str, body: &str) -> Result<(u16, String)> {
    use std::os::unix::net::UnixStream;

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
