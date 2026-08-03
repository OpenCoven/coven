use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde::Serialize;
use uuid::Uuid;

use super::audit::{append_event, MobileAuditEvent};
use super::auth::{MobileAuthError, MobileAuthenticator, MobileRequestAuth};
use super::config::{load_mobile_config, load_mobile_config_unvalidated, MobileGatewayConfig};
use super::contract::{
    MobileAttestationMetadata, MobileCapabilities, MobileContentFormat, MobileEnvelope,
    MobileErrorCode, MobileMemoryCapabilities, MobileMemoryDetail, MobileMemoryPrivacyDetail,
    MobileMemoryPrivacySummary, MobileMemorySource, MobileMemorySummary,
    MobileMemoryVerificationDetail, MobileMemoryVerificationSummary, MobileOverview,
    MobileOverviewTotals, MobileOverviewVerification, MobileSupersession, MobileVerificationState,
};
use super::identity::load_or_create_host_identity;
use super::pairing::{PairingError, PairingManager, PairingProgress};
use super::registry::DeviceRegistry;
use super::{MAX_MOBILE_REQUEST_BYTES, MAX_MOBILE_RESPONSE_BYTES};

const MOBILE_IO_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HEADER_LINE_BYTES: usize = 4 * 1024;
const MAX_HEADER_COUNT: usize = 32;
const MAX_INFLIGHT_CONNECTIONS: usize = 32;
const PAIRING_LIFETIME: chrono::Duration = chrono::Duration::minutes(5);

static ACTIVE_GATEWAY: LazyLock<Mutex<Option<Weak<MobileGatewayState>>>> =
    LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileRoute {
    Capabilities,
    Pairings,
    PairingConfirmation,
    MemoryOverview,
    MemoryList,
    MemoryDetail,
    DeviceRead,
    DeviceDelete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    Route(MobileRoute),
    MethodNotAllowed(&'static str),
    NotFound,
}

pub fn route_request(method: &str, path: &str) -> RouteDecision {
    match path {
        "/api/v1/mobile/capabilities" => {
            return method_route(method, "GET", MobileRoute::Capabilities)
        }
        "/api/v1/mobile/pairings" => return method_route(method, "POST", MobileRoute::Pairings),
        "/api/v1/mobile/memory/overview" => {
            return method_route(method, "GET", MobileRoute::MemoryOverview)
        }
        "/api/v1/mobile/memory" => return method_route(method, "GET", MobileRoute::MemoryList),
        "/api/v1/mobile/device" => {
            return match method {
                "GET" => RouteDecision::Route(MobileRoute::DeviceRead),
                "DELETE" => RouteDecision::Route(MobileRoute::DeviceDelete),
                _ => RouteDecision::MethodNotAllowed("GET, DELETE"),
            }
        }
        _ => {}
    }
    if let Some(id) = path.strip_prefix("/api/v1/mobile/pairings/") {
        if id
            .strip_suffix("/confirm")
            .is_some_and(|id| Uuid::parse_str(id).is_ok())
        {
            return method_route(method, "POST", MobileRoute::PairingConfirmation);
        }
    }
    if path
        .strip_prefix("/api/v1/mobile/memory/")
        .is_some_and(|id| Uuid::parse_str(id).is_ok())
    {
        return method_route(method, "GET", MobileRoute::MemoryDetail);
    }
    RouteDecision::NotFound
}

fn method_route(method: &str, allowed: &'static str, route: MobileRoute) -> RouteDecision {
    if method == allowed {
        RouteDecision::Route(route)
    } else {
        RouteDecision::MethodNotAllowed(allowed)
    }
}

pub fn mobile_list(coven_home: &Path) -> Result<Vec<MobileMemorySummary>> {
    crate::cockpit_sources::scan_memory(coven_home)?
        .into_iter()
        .map(convert_summary)
        .collect()
}

pub fn mobile_overview(coven_home: &Path) -> Result<MobileOverview> {
    let source = crate::cockpit_sources::memory_overview(coven_home)?;
    Ok(MobileOverview {
        generated_at: parse_timestamp(&source.generated_at)?,
        totals: MobileOverviewTotals {
            entries: source.totals.entries,
            familiars: source.totals.familiars,
            verified: source.totals.verified,
            needs_review: source.totals.needs_review,
            unknown: source.totals.unknown,
        },
        last_updated_at: source
            .last_updated_at
            .as_deref()
            .map(parse_timestamp)
            .transpose()?,
        capabilities: MobileMemoryCapabilities {
            detail: source.capabilities.detail,
            verification: source.capabilities.verification,
            attestation_metadata: source.capabilities.attestation_metadata,
            supersession_history: source.capabilities.supersession_history,
            mutations: false,
        },
        verification: MobileOverviewVerification {
            state: verification_state(&source.verification.state)?,
            checked_at: parse_timestamp(&source.verification.checked_at)?,
            manifest: source
                .verification
                .manifest
                .unwrap_or_else(|| "unavailable".to_owned()),
            index: source
                .verification
                .index
                .unwrap_or_else(|| "unavailable".to_owned()),
            issues: source
                .verification
                .issues
                .into_iter()
                .map(|issue| bounded(issue, 512, "verification issue"))
                .collect::<Result<Vec<_>>>()?,
        },
    })
}

pub fn mobile_detail(coven_home: &Path, id: &str) -> Result<Option<MobileMemoryDetail>> {
    crate::cockpit_sources::read_memory_detail(coven_home, id)?
        .map(convert_detail)
        .transpose()
}

fn convert_summary(source: crate::cockpit_sources::MemoryFileDto) -> Result<MobileMemorySummary> {
    Ok(MobileMemorySummary {
        id: Uuid::parse_str(&source.id).context("memory summary id is not a UUID")?,
        familiar_id: bounded(source.familiar_id, 128, "familiar id")?,
        title: bounded(source.title, 512, "title")?,
        updated_at: parse_timestamp(&source.updated_at_iso)?,
        relative_updated_at: bounded(source.updated_at, 128, "relative timestamp")?,
        excerpt: bounded(source.excerpt, 1_024, "excerpt")?,
        source: MobileMemorySource {
            kind: bounded(source.source.kind, 64, "source kind")?,
            label: bounded(source.source.label, 128, "source label")?,
        },
        privacy: MobileMemoryPrivacySummary {
            classification: source
                .privacy_classification
                .map(|value| bounded(value, 128, "privacy classification"))
                .transpose()?,
            reveal_required: source.reveal_required,
        },
        verification: MobileMemoryVerificationSummary {
            state: verification_state(&source.verification_state)?,
        },
    })
}

fn convert_detail(source: crate::cockpit_sources::MemoryDetailDto) -> Result<MobileMemoryDetail> {
    let attestation_metadata = match source.attestation {
        None => None,
        Some(serde_json::Value::Object(fields)) => Some(MobileAttestationMetadata {
            field_count: fields.len(),
        }),
        Some(_) => bail!("memory attestation metadata is not an object"),
    };
    if source.content_format != "markdown" {
        bail!("unsupported memory content format");
    }
    Ok(MobileMemoryDetail {
        id: Uuid::parse_str(&source.id).context("memory detail id is not a UUID")?,
        familiar_id: bounded(source.familiar_id, 128, "familiar id")?,
        title: bounded(source.title, 512, "title")?,
        updated_at: parse_timestamp(&source.updated_at)?,
        source: MobileMemorySource {
            kind: bounded(source.source.kind, 64, "source kind")?,
            label: bounded(source.source.label, 128, "source label")?,
        },
        content: bounded(source.content, super::MAX_MOBILE_RESPONSE_BYTES, "content")?,
        content_format: MobileContentFormat::Markdown,
        privacy: MobileMemoryPrivacyDetail {
            classification: source
                .privacy
                .classification
                .map(|value| bounded(value, 128, "privacy classification"))
                .transpose()?,
            reveal_required: source.privacy.reveal_required,
            reason: bounded(source.privacy.reason, 512, "privacy reason")?,
        },
        verification: MobileMemoryVerificationDetail {
            state: verification_state(&source.verification.state)?,
            reason: bounded(source.verification.reason, 512, "verification reason")?,
        },
        attestation_metadata,
        supersession: MobileSupersession {
            supersedes: source
                .supersession
                .supersedes
                .as_deref()
                .map(Uuid::parse_str)
                .transpose()
                .context("supersedes id is not a UUID")?,
            superseded_by: source
                .supersession
                .superseded_by
                .as_deref()
                .map(Uuid::parse_str)
                .transpose()
                .context("superseded-by id is not a UUID")?,
        },
    })
}

fn bounded(value: String, max_bytes: usize, field: &str) -> Result<String> {
    if value.len() > max_bytes {
        bail!("mobile {field} exceeds its response bound");
    }
    Ok(value)
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .context("invalid mobile timestamp in domain response")
}

fn verification_state(value: &str) -> Result<MobileVerificationState> {
    match value {
        "verified" => Ok(MobileVerificationState::Verified),
        "needs-review" => Ok(MobileVerificationState::NeedsReview),
        "unknown" | "unavailable" => Ok(MobileVerificationState::Unknown),
        _ => bail!("unknown memory verification state"),
    }
}

pub struct MobileGatewayHandle {
    local_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MobileGatewayHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for MobileGatewayHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.local_addr, Duration::from_millis(100));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct MobileGatewayState {
    coven_home: PathBuf,
    host_fingerprint: [u8; 32],
    registry: Arc<DeviceRegistry>,
    pairing: PairingManager,
    authenticator: MobileAuthenticator,
    advertised_endpoint: String,
}

pub fn start_mobile_gateway(coven_home: &Path) -> Result<Option<MobileGatewayHandle>> {
    let Some(config) = load_mobile_config(coven_home)? else {
        return Ok(None);
    };
    if !config.enabled {
        return Ok(None);
    }
    start_mobile_gateway_with_config(coven_home, &config).map(Some)
}

/// Start the optional listener for the daemon. An unreadable or malformed
/// optional config is quarantined to the mobile surface so the owner-only
/// local daemon remains available; a valid enabled config that cannot bind or
/// initialize still fails startup instead of silently disabling remote access.
pub fn start_mobile_gateway_for_daemon(coven_home: &Path) -> Result<Option<MobileGatewayHandle>> {
    let config = match load_mobile_config_unvalidated(coven_home) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("coven mobile gateway disabled: invalid private configuration: {error:#}");
            return Ok(None);
        }
    };
    let Some(config) = config else {
        return Ok(None);
    };
    if !config.enabled {
        return Ok(None);
    }
    start_mobile_gateway_with_config(coven_home, &config).map(Some)
}

fn start_mobile_gateway_with_config(
    coven_home: &Path,
    config: &MobileGatewayConfig,
) -> Result<MobileGatewayHandle> {
    super::config::validate_mobile_config(config)?;
    let endpoint = url::Url::parse(&config.advertised_endpoint)?;
    let identity = load_or_create_host_identity(
        coven_home,
        endpoint
            .host_str()
            .context("mobile endpoint omitted host")?,
    )?;
    let tls_config =
        tls_server_config(identity.certificate_der, identity.private_key_der.to_vec())?;
    let listener = TcpListener::bind(config.bind)
        .with_context(|| format!("failed to bind mobile gateway {}", config.bind))?;
    listener
        .set_nonblocking(true)
        .context("failed to configure mobile gateway listener")?;
    let local_addr = listener.local_addr()?;
    let registry = Arc::new(DeviceRegistry::load(coven_home)?);
    let state = Arc::new(MobileGatewayState {
        coven_home: coven_home.to_path_buf(),
        host_fingerprint: identity.public_key_fingerprint,
        pairing: PairingManager::new(Arc::clone(&registry)),
        authenticator: MobileAuthenticator::new(Arc::clone(&registry)),
        registry,
        advertised_endpoint: config.advertised_endpoint.clone(),
    });
    *ACTIVE_GATEWAY
        .lock()
        .map_err(|_| anyhow::anyhow!("mobile gateway control lock was poisoned"))? =
        Some(Arc::downgrade(&state));
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = Arc::clone(&shutdown);
    let thread_home = coven_home.to_path_buf();
    let inflight = Arc::new(AtomicUsize::new(0));
    let thread = std::thread::Builder::new()
        .name("coven-mobile-memory".to_owned())
        .spawn(move || {
            let _ = append_event(
                &thread_home,
                Utc::now(),
                MobileAuditEvent::GatewayStarted,
                None,
            );
            while !thread_shutdown.load(Ordering::Acquire) {
                let (stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    Err(error) => {
                        eprintln!("coven mobile gateway accept error: {error}");
                        continue;
                    }
                };
                if let Err(error) = stream.set_nonblocking(false) {
                    eprintln!("coven mobile gateway connection rejected: {error}");
                    continue;
                }
                if thread_shutdown.load(Ordering::Acquire) {
                    break;
                }
                if inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT_CONNECTIONS {
                    if let Err(error) =
                        serve_tls_connection(stream, Arc::clone(&tls_config), &state)
                    {
                        eprintln!("coven mobile gateway connection rejected: {error}");
                    }
                    continue;
                }
                inflight.fetch_add(1, Ordering::Relaxed);
                let config = Arc::clone(&tls_config);
                let state = Arc::clone(&state);
                let connection_inflight = Arc::clone(&inflight);
                let spawn = std::thread::Builder::new()
                    .name("coven-mobile-request".to_owned())
                    .spawn(move || {
                        if let Err(error) = serve_tls_connection(stream, config, &state) {
                            eprintln!("coven mobile gateway connection rejected: {error}");
                        }
                        connection_inflight.fetch_sub(1, Ordering::Relaxed);
                    });
                if let Err(error) = spawn {
                    inflight.fetch_sub(1, Ordering::Relaxed);
                    eprintln!("coven mobile gateway request thread rejected: {error}");
                }
            }
            let _ = append_event(
                &thread_home,
                Utc::now(),
                MobileAuditEvent::GatewayStopped,
                None,
            );
        })
        .context("failed to spawn mobile gateway thread")?;
    Ok(MobileGatewayHandle {
        local_addr,
        shutdown,
        thread: Some(thread),
    })
}

fn tls_server_config(certificate: Vec<u8>, private_key: Vec<u8>) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("failed to restrict mobile gateway to TLS 1.3")?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key)),
        )
        .context("failed to configure mobile TLS identity")?;
    Ok(Arc::new(config))
}

fn serve_tls_connection(
    stream: TcpStream,
    config: Arc<ServerConfig>,
    state: &MobileGatewayState,
) -> Result<()> {
    stream.set_read_timeout(Some(MOBILE_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(MOBILE_IO_TIMEOUT))?;
    let connection = ServerConnection::new(config).context("failed to create TLS connection")?;
    let mut tls = StreamOwned::new(connection, stream);
    let request = read_mobile_request(&mut tls)?;
    let response = handle_mobile_request(state, request);
    tls.write_all(&response.to_http())?;
    tls.flush()?;
    Ok(())
}

struct MobileHttpRequest {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_mobile_request(stream: &mut impl Read) -> Result<MobileHttpRequest> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader
        .by_ref()
        .take(MAX_HEADER_LINE_BYTES as u64)
        .read_line(&mut request_line)?;
    if request_line.len() >= MAX_HEADER_LINE_BYTES && !request_line.ends_with('\n') {
        bail!("mobile HTTP request line exceeds limit");
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("request omitted method")?.to_owned();
    let target = parts.next().context("request omitted target")?.to_owned();
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        bail!("invalid mobile HTTP request line");
    }
    let mut headers = HashMap::new();
    for _ in 0..MAX_HEADER_COUNT {
        let mut line = String::new();
        reader
            .by_ref()
            .take(MAX_HEADER_LINE_BYTES as u64)
            .read_line(&mut line)?;
        if line.len() >= MAX_HEADER_LINE_BYTES && !line.ends_with('\n') {
            bail!("mobile HTTP header line exceeds limit");
        }
        if line == "\r\n" {
            if headers.contains_key("transfer-encoding") {
                bail!("mobile gateway does not accept transfer encoding");
            }
            let content_length = headers
                .get("content-length")
                .map(|value: &String| value.parse::<usize>())
                .transpose()?
                .unwrap_or(0);
            if content_length > MAX_MOBILE_REQUEST_BYTES {
                bail!("mobile request body exceeds limit");
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body)?;
            return Ok(MobileHttpRequest {
                method,
                target,
                headers,
                body,
            });
        }
        if line.is_empty() {
            bail!("mobile request headers ended unexpectedly");
        }
        let (name, value) = line
            .trim_end_matches(['\r', '\n'])
            .split_once(':')
            .context("invalid mobile HTTP header")?;
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            bail!("duplicate mobile HTTP header");
        }
    }
    bail!("too many mobile HTTP headers")
}

struct MobileHttpResponse {
    status: u16,
    allow: Option<&'static str>,
    body: String,
}

impl MobileHttpResponse {
    fn to_http(&self) -> Vec<u8> {
        let allow = self
            .allow
            .map(|value| format!("Allow: {value}\r\n"))
            .unwrap_or_default();
        format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.status,
            reason(self.status),
            allow,
            self.body.len(),
            self.body
        )
        .into_bytes()
    }
}

fn handle_mobile_request(
    state: &MobileGatewayState,
    request: MobileHttpRequest,
) -> MobileHttpResponse {
    if request.target.contains('?') {
        return error_response(404, MobileErrorCode::InvalidRequest);
    }
    let path = request
        .target
        .split_once('?')
        .map_or(request.target.as_str(), |value| value.0)
        .to_owned();
    match route_request(&request.method, &path) {
        RouteDecision::NotFound => error_response(404, MobileErrorCode::InvalidRequest),
        RouteDecision::MethodNotAllowed(allow) => MobileHttpResponse {
            status: 405,
            allow: Some(allow),
            body: envelope_error(MobileErrorCode::InvalidRequest),
        },
        RouteDecision::Route(MobileRoute::Capabilities) => success_response(
            200,
            MobileCapabilities {
                minimum_protocol_version: 1,
                current_protocol_version: 1,
                maximum_protocol_version: 1,
            },
        ),
        RouteDecision::Route(MobileRoute::Pairings) => handle_pairing_enrollment(state, request),
        RouteDecision::Route(MobileRoute::PairingConfirmation) => {
            handle_pairing_confirmation(state, &path, request)
        }
        RouteDecision::Route(route) => handle_protected_route(state, route, &path, request),
    }
}

fn handle_pairing_enrollment(
    state: &MobileGatewayState,
    request: MobileHttpRequest,
) -> MobileHttpResponse {
    let parsed: super::contract::MobilePairingRequest = match serde_json::from_slice(&request.body)
    {
        Ok(parsed) => parsed,
        Err(_) => return error_response(400, MobileErrorCode::InvalidRequest),
    };
    let nonce = match URL_SAFE_NO_PAD
        .decode(&parsed.pairing_nonce)
        .ok()
        .and_then(|nonce| <[u8; 32]>::try_from(nonce).ok())
    {
        Some(nonce) => nonce,
        None => return error_response(400, MobileErrorCode::InvalidRequest),
    };
    match state
        .pairing
        .enroll_by_nonce(nonce, parsed, state.host_fingerprint, Utc::now())
    {
        Ok(enrolled) => success_response(202, enrolled),
        Err(error) => {
            let _ = append_event(
                &state.coven_home,
                Utc::now(),
                MobileAuditEvent::PairingRejected,
                None,
            );
            pairing_error_response(error)
        }
    }
}

fn handle_pairing_confirmation(
    state: &MobileGatewayState,
    path: &str,
    request: MobileHttpRequest,
) -> MobileHttpResponse {
    let id = path
        .strip_prefix("/api/v1/mobile/pairings/")
        .and_then(|path| path.strip_suffix("/confirm"))
        .and_then(|id| Uuid::parse_str(id).ok());
    let confirmation: super::contract::MobilePairingConfirmation =
        match serde_json::from_slice(&request.body) {
            Ok(value) => value,
            Err(_) => return error_response(400, MobileErrorCode::InvalidRequest),
        };
    let Some(id) = id else {
        return error_response(404, MobileErrorCode::InvalidRequest);
    };
    match state
        .pairing
        .confirm_device(id, &confirmation.phrase, Utc::now())
    {
        Ok(PairingProgress::Pending) => {
            error_response(409, MobileErrorCode::PairingConfirmationRequired)
        }
        Ok(PairingProgress::Complete { device, replayed }) => {
            success_response(if replayed { 200 } else { 201 }, device)
        }
        Err(error) => {
            let _ = append_event(
                &state.coven_home,
                Utc::now(),
                MobileAuditEvent::PairingRejected,
                None,
            );
            pairing_error_response(error)
        }
    }
}

fn handle_protected_route(
    state: &MobileGatewayState,
    route: MobileRoute,
    path: &str,
    request: MobileHttpRequest,
) -> MobileHttpResponse {
    if request.headers.get("x-coven-protocol").map(String::as_str) != Some("1") {
        return error_response(400, MobileErrorCode::ProtocolUnsupported);
    }
    let auth = match parse_auth_headers(&request.headers) {
        Ok(auth) => auth,
        Err(error) => {
            let _ = append_event(
                &state.coven_home,
                Utc::now(),
                MobileAuditEvent::AuthenticationRejected,
                None,
            );
            return auth_error_response(error);
        }
    };
    let verified = match state.authenticator.verify(
        &request.method,
        &request.target,
        &request.body,
        &auth,
        Utc::now(),
    ) {
        Ok(verified) => verified,
        Err(error) => {
            let event = if error == MobileAuthError::RateLimited {
                MobileAuditEvent::RateLimited
            } else {
                MobileAuditEvent::AuthenticationRejected
            };
            let _ = append_event(&state.coven_home, Utc::now(), event, Some(auth.device_id));
            return auth_error_response(error);
        }
    };
    let result: Result<serde_json::Value> = match route {
        MobileRoute::MemoryOverview => mobile_overview(&state.coven_home).and_then(json_value),
        MobileRoute::MemoryList => mobile_list(&state.coven_home).and_then(json_value),
        MobileRoute::MemoryDetail => {
            let id = path.trim_start_matches("/api/v1/mobile/memory/");
            match mobile_detail(&state.coven_home, id) {
                Ok(Some(value)) => json_value(value),
                Ok(None) => return error_response(404, MobileErrorCode::MemoryNotFound),
                Err(error) => return memory_detail_error_response(&error),
            }
        }
        MobileRoute::DeviceRead => state.registry.list_redacted().and_then(|devices| {
            devices
                .into_iter()
                .find(|device| device.id == verified.device_id)
                .context("authenticated device disappeared")
                .and_then(json_value)
        }),
        MobileRoute::DeviceDelete => {
            if state
                .registry
                .revoke(verified.device_id, Utc::now())
                .is_err()
            {
                return error_response(503, MobileErrorCode::DaemonUnavailable);
            }
            let _ = append_event(
                &state.coven_home,
                Utc::now(),
                MobileAuditEvent::DeviceRevoked,
                Some(verified.device_id),
            );
            Ok(serde_json::Value::Null)
        }
        _ => return error_response(404, MobileErrorCode::InvalidRequest),
    };
    if route != MobileRoute::DeviceDelete
        && state.authenticator.ensure_still_active(&verified).is_err()
    {
        return error_response(403, MobileErrorCode::DeviceRevoked);
    }
    match result {
        Ok(value) => success_response(200, value),
        Err(_) => error_response(500, MobileErrorCode::ResponseInvalid),
    }
}

fn memory_detail_error_response(error: &anyhow::Error) -> MobileHttpResponse {
    match error.downcast_ref::<crate::cockpit_sources::MemoryContentError>() {
        Some(crate::cockpit_sources::MemoryContentError::TooLarge { .. }) => {
            error_response(413, MobileErrorCode::MemoryContentTooLarge)
        }
        Some(crate::cockpit_sources::MemoryContentError::InvalidUtf8) => {
            error_response(422, MobileErrorCode::MemoryContentInvalid)
        }
        Some(crate::cockpit_sources::MemoryContentError::MissingOrUnsafe) => {
            error_response(404, MobileErrorCode::MemoryNotFound)
        }
        Some(crate::cockpit_sources::MemoryContentError::Unavailable(_)) | None => {
            error_response(503, MobileErrorCode::MemoryContentUnavailable)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalPairingInvitation {
    id: Uuid,
    terminal_output: String,
    expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalPairingStatus {
    phrase: Option<[String; 6]>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LocalPairingConfirmation {
    phrase: [String; 6],
}

fn active_gateway() -> Result<Arc<MobileGatewayState>> {
    ACTIVE_GATEWAY
        .lock()
        .map_err(|_| anyhow::anyhow!("mobile gateway control lock was poisoned"))?
        .as_ref()
        .and_then(Weak::upgrade)
        .context("mobile gateway is not running")
}

pub(crate) fn handle_local_control(
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Option<Result<crate::api::ApiResponse>> {
    const ROOT: &str = "/api/v1/internal/mobile/pairings";
    if path == ROOT {
        if method != "POST" {
            return Some(crate::api::api_error(
                405,
                "method_not_allowed",
                "Method not allowed.",
                None,
            ));
        }
        return Some((|| {
            let state = active_gateway()?;
            let invitation = state
                .pairing
                .begin_pairing(rand::random(), Utc::now() + PAIRING_LIFETIME)?;
            let url = super::pairing::build_pairing_url(
                &invitation,
                &state.advertised_endpoint,
                state.host_fingerprint,
            )?;
            let terminal_output =
                super::pairing::render_pairing_invitation(&url, invitation.expires_at)?;
            append_event(
                &state.coven_home,
                Utc::now(),
                MobileAuditEvent::PairingCreated,
                None,
            )?;
            crate::api::json_response(
                201,
                &LocalPairingInvitation {
                    id: invitation.id,
                    terminal_output,
                    expires_at: invitation.expires_at,
                },
            )
        })());
    }
    let suffix = path.strip_prefix(&(ROOT.to_owned() + "/"))?;
    let Some((id, action)) = suffix.split_once('/') else {
        return Some(crate::api::api_error(
            404,
            "not_found",
            "Route not found.",
            None,
        ));
    };
    let id = match Uuid::parse_str(id) {
        Ok(id) => id,
        Err(_) => {
            return Some(crate::api::api_error(
                400,
                "invalid_request",
                "Pairing id must be a UUID.",
                None,
            ))
        }
    };
    Some((|| {
        let state = active_gateway()?;
        match (method, action) {
            ("POST", "status") => crate::api::json_response(
                200,
                &LocalPairingStatus {
                    phrase: state.pairing.phrase(id, Utc::now())?,
                },
            ),
            ("POST", "confirm") => {
                let confirmation: LocalPairingConfirmation =
                    serde_json::from_str(body.context("pairing confirmation omitted body")?)?;
                match state
                    .pairing
                    .confirm_host(id, &confirmation.phrase, Utc::now())?
                {
                    PairingProgress::Complete { device, replayed } => {
                        if !replayed {
                            append_event(
                                &state.coven_home,
                                Utc::now(),
                                MobileAuditEvent::PairingCompleted,
                                Some(device.id),
                            )?;
                        }
                        crate::api::json_response(200, &device)
                    }
                    PairingProgress::Pending => crate::api::api_error(
                        409,
                        "pairing_confirmation_required",
                        "The device must also confirm the pairing phrase.",
                        None,
                    ),
                }
            }
            _ => crate::api::api_error(404, "not_found", "Route not found.", None),
        }
    })())
}

fn json_value(value: impl Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(value).context("failed to serialize mobile response")
}

fn parse_auth_headers(
    headers: &HashMap<String, String>,
) -> std::result::Result<MobileRequestAuth, MobileAuthError> {
    let get = |name: &str| headers.get(name).ok_or(MobileAuthError::InvalidEncoding);
    Ok(MobileRequestAuth {
        device_id: get("x-coven-device")?
            .parse()
            .map_err(|_| MobileAuthError::InvalidEncoding)?,
        timestamp: get("x-coven-timestamp")?
            .parse()
            .map_err(|_| MobileAuthError::InvalidEncoding)?,
        nonce: get("x-coven-nonce")?.clone(),
        body_digest: get("x-coven-body-sha256")?.clone(),
        signature: get("x-coven-signature")?.clone(),
    })
}

fn success_response<T: Serialize>(status: u16, data: T) -> MobileHttpResponse {
    let envelope = MobileEnvelope::success(request_id(), data);
    let body = match serde_json::to_string(&envelope) {
        Ok(body) => body,
        Err(_) => return error_response(500, MobileErrorCode::ResponseInvalid),
    };
    if body.len() > MAX_MOBILE_RESPONSE_BYTES {
        return error_response(413, MobileErrorCode::MemoryContentTooLarge);
    }
    MobileHttpResponse {
        status,
        allow: None,
        body,
    }
}

fn error_response(status: u16, code: MobileErrorCode) -> MobileHttpResponse {
    MobileHttpResponse {
        status,
        allow: None,
        body: envelope_error(code),
    }
}

fn envelope_error(code: MobileErrorCode) -> String {
    serde_json::to_string(&MobileEnvelope::<()>::error(request_id(), code))
        .expect("mobile error envelope is serializable")
}

fn auth_error_response(error: MobileAuthError) -> MobileHttpResponse {
    let (status, code) = match error {
        MobileAuthError::DeviceUnknown => (401, MobileErrorCode::DeviceUnknown),
        MobileAuthError::DeviceRevoked => (403, MobileErrorCode::DeviceRevoked),
        MobileAuthError::RequestExpired => (401, MobileErrorCode::RequestExpired),
        MobileAuthError::RequestReplayed => (409, MobileErrorCode::RequestReplayed),
        MobileAuthError::RateLimited => (429, MobileErrorCode::RateLimited),
        MobileAuthError::SignatureInvalid => (401, MobileErrorCode::SignatureInvalid),
        _ => (400, MobileErrorCode::InvalidRequest),
    };
    error_response(status, code)
}

fn pairing_error_response(error: PairingError) -> MobileHttpResponse {
    let (status, code) = match error {
        PairingError::PairingExpired => (410, MobileErrorCode::PairingExpired),
        PairingError::PairingConsumed => (409, MobileErrorCode::PairingConsumed),
        PairingError::PairingConfirmationRequired => {
            (409, MobileErrorCode::PairingConfirmationRequired)
        }
        PairingError::PairingPhraseMismatch => (400, MobileErrorCode::PairingPhraseMismatch),
        PairingError::InvalidRequest => (400, MobileErrorCode::InvalidRequest),
    };
    error_response(status, code)
}

fn request_id() -> String {
    const ALPHABET: [[u8; 8]; 4] = [*b"01234567", *b"89ABCDEF", *b"GHJKMNPQ", *b"RSTVWXYZ"];
    let mut bytes = [0_u8; 16];
    let milliseconds = Utc::now().timestamp_millis().max(0) as u64;
    bytes[..6].copy_from_slice(&milliseconds.to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&Uuid::new_v4().as_bytes()[6..]);
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        let index = (value & 31) as usize;
        *character = ALPHABET[index / 8][index % 8];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec()).expect("ULID alphabet is UTF-8")
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore};
    use std::collections::HashMap;
    use std::net::{IpAddr, UdpSocket};

    static TEST_GATEWAY_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn mobile_route_table_never_forwards_local_api_paths() {
        for path in [
            "/api/v1/sessions",
            "/api/v1/ward",
            "/api/v1/hub",
            "/api/v1/familiars",
            "/api/v1/skills",
            "/api/v1/store",
            "/health",
        ] {
            assert_eq!(route_request("GET", path), RouteDecision::NotFound);
        }
    }

    #[test]
    fn unsupported_methods_have_exact_allow_values() {
        assert_eq!(
            route_request("POST", "/api/v1/mobile/memory"),
            RouteDecision::MethodNotAllowed("GET")
        );
        assert_eq!(
            route_request("PATCH", "/api/v1/mobile/device"),
            RouteDecision::MethodNotAllowed("GET, DELETE")
        );
    }

    #[test]
    fn mobile_conversion_omits_local_paths() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("memory/sage")).unwrap();
        std::fs::write(
            temp.path().join("memory/sage/synthetic.md"),
            "# Synthetic note\n\nSynthetic content.",
        )
        .unwrap();
        let encoded = serde_json::to_value(mobile_list(temp.path()).unwrap()).unwrap();
        assert!(encoded[0].get("path").is_none());
    }

    #[test]
    fn disabled_config_starts_no_mobile_listener() {
        let temp = tempfile::tempdir().unwrap();
        assert!(start_mobile_gateway(temp.path()).unwrap().is_none());
    }

    #[test]
    fn mobile_listener_requires_pinned_tls13() {
        let _guard = TEST_GATEWAY_LOCK.lock().unwrap();
        let Some((temp, config, certificate)) = test_listener_config() else {
            return;
        };
        let gateway = start_mobile_gateway_with_config(temp.path(), &config).unwrap();

        let unpinned = tls_client(gateway.local_addr(), config.bind.ip(), None, true);
        assert!(unpinned.is_err(), "an unpinned certificate must fail");

        let tls12 = tls_client(
            gateway.local_addr(),
            config.bind.ip(),
            Some(certificate.clone()),
            false,
        );
        assert!(tls12.is_err(), "TLS 1.2 negotiation must fail");

        let response = tls_client(
            gateway.local_addr(),
            config.bind.ip(),
            Some(certificate),
            true,
        )
        .unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"currentProtocolVersion\":1"));
    }

    #[test]
    fn local_control_creates_memory_only_pairing() {
        let _guard = TEST_GATEWAY_LOCK.lock().unwrap();
        let Some((temp, config, _)) = test_listener_config() else {
            return;
        };
        let _gateway = start_mobile_gateway_with_config(temp.path(), &config).unwrap();

        let response = handle_local_control("POST", "/api/v1/internal/mobile/pairings", Some("{}"))
            .unwrap()
            .unwrap();
        assert_eq!(response.status, 201);
        let invitation: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        let id = invitation["id"].as_str().unwrap();
        assert_eq!(
            invitation["terminalOutput"]
                .as_str()
                .unwrap()
                .matches("coven-memory://pair")
                .count(),
            1
        );
        let status = handle_local_control(
            "POST",
            &format!("/api/v1/internal/mobile/pairings/{id}/status"),
            Some("{}"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(status.status, 200);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&status.body).unwrap()["phrase"],
            serde_json::Value::Null
        );
        assert!(!temp.path().join("mobile/pairings.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn malformed_mobile_config_does_not_disable_local_daemon_startup() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let mobile = temp.path().join("mobile");
        std::fs::create_dir(&mobile).unwrap();
        std::fs::set_permissions(&mobile, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = mobile.join(super::super::config::GATEWAY_CONFIG_FILE);
        std::fs::write(&config, b"{not-json").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert!(start_mobile_gateway_for_daemon(temp.path())
            .unwrap()
            .is_none());
    }

    fn test_listener_config() -> Option<(tempfile::TempDir, MobileGatewayConfig, Vec<u8>)> {
        let ip = [
            "100.100.100.100:9",
            "10.0.0.1:9",
            "192.168.1.1:9",
            "172.16.0.1:9",
            "1.1.1.1:9",
        ]
        .into_iter()
        .find_map(|destination| {
            let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
            socket.connect(destination).ok()?;
            let ip = socket.local_addr().ok()?.ip();
            if !matches!(ip, IpAddr::V4(_))
                || !super::super::config::is_private_mobile_address_for_test(ip)
            {
                return None;
            }
            let probe = TcpListener::bind(SocketAddr::new(ip, 0)).ok()?;
            let address = probe.local_addr().ok()?;
            TcpStream::connect_timeout(&address, Duration::from_millis(200))
                .ok()
                .map(|_| ip)
        })?;
        let reserved = TcpListener::bind(SocketAddr::new(ip, 0)).ok()?;
        let port = reserved.local_addr().ok()?.port();
        drop(reserved);
        let temp = tempfile::tempdir().ok()?;
        let config = MobileGatewayConfig {
            enabled: true,
            bind: SocketAddr::new(ip, port),
            advertised_endpoint: format!("https://{ip}:{port}"),
        };
        if super::super::config::validate_mobile_config(&config).is_err() {
            return None;
        }
        let identity = load_or_create_host_identity(temp.path(), &ip.to_string()).ok()?;
        Some((temp, config, identity.certificate_der))
    }

    fn tls_client(
        address: SocketAddr,
        server_ip: IpAddr,
        certificate: Option<Vec<u8>>,
        tls13: bool,
    ) -> Result<String> {
        let mut roots = RootCertStore::empty();
        if let Some(certificate) = certificate {
            roots.add(CertificateDer::from(certificate))?;
        }
        let versions = if tls13 {
            &[&rustls::version::TLS13][..]
        } else {
            &[&rustls::version::TLS12][..]
        };
        let config = ClientConfig::builder_with_protocol_versions(versions)
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server_name = ServerName::try_from(server_ip.to_string())?.to_owned();
        let connection = ClientConnection::new(Arc::new(config), server_name)?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(10))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let mut tls = StreamOwned::new(connection, stream);
        tls.write_all(b"GET /api/v1/mobile/capabilities HTTP/1.1\r\nHost: coven-memory\r\n\r\n")?;
        let mut response = String::new();
        if let Err(error) = tls.read_to_string(&mut response) {
            if response.is_empty()
                || !matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::UnexpectedEof
                )
            {
                return Err(error.into());
            }
        }
        Ok(response)
    }

    fn sample_pairing_request(nonce: [u8; 32]) -> super::super::contract::MobilePairingRequest {
        let signing_key = p256::SecretKey::from_slice(&[1; 32]).unwrap();
        let public_key = signing_key.public_key().to_encoded_point(false);
        super::super::contract::MobilePairingRequest {
            protocol_version: super::super::MOBILE_PROTOCOL_VERSION,
            pairing_nonce: URL_SAFE_NO_PAD.encode(nonce),
            device_name: "Synthetic phone".to_owned(),
            device_public_key: URL_SAFE_NO_PAD.encode(public_key.as_bytes()),
            app_version: "1.0.0".to_owned(),
            supported_protocol: super::super::contract::MobileProtocolRange {
                minimum: 1,
                maximum: 1,
            },
        }
    }

    fn envelope_data(body: &str) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(body).unwrap()["data"].clone()
    }

    #[test]
    fn device_confirmation_replay_returns_same_envelope_data() {
        let _guard = TEST_GATEWAY_LOCK.lock().unwrap();
        let Some((temp, config, _)) = test_listener_config() else {
            return;
        };
        let _gateway = start_mobile_gateway_with_config(temp.path(), &config).unwrap();
        let state = active_gateway().unwrap();
        let now = Utc::now();
        let invitation = state
            .pairing
            .begin_pairing([7; 32], now + PAIRING_LIFETIME)
            .unwrap();
        let enrolled = state
            .pairing
            .enroll(
                invitation.id,
                invitation.nonce,
                sample_pairing_request(invitation.nonce),
                state.host_fingerprint,
                now,
            )
            .unwrap();
        assert_eq!(
            state
                .pairing
                .confirm_host(invitation.id, &enrolled.phrase, now)
                .unwrap(),
            PairingProgress::Pending
        );

        let path = format!("/api/v1/mobile/pairings/{}/confirm", invitation.id);
        let body = serde_json::json!({ "phrase": enrolled.phrase })
            .to_string()
            .into_bytes();

        let first = handle_pairing_confirmation(
            &state,
            &path,
            MobileHttpRequest {
                method: "POST".to_owned(),
                target: path.clone(),
                headers: HashMap::new(),
                body: body.clone(),
            },
        );
        let replay = handle_pairing_confirmation(
            &state,
            &path,
            MobileHttpRequest {
                method: "POST".to_owned(),
                target: path.clone(),
                headers: HashMap::new(),
                body,
            },
        );

        assert_eq!(first.status, 201);
        assert_eq!(replay.status, 200);
        assert_eq!(envelope_data(&replay.body), envelope_data(&first.body));
    }

    #[test]
    fn local_control_replay_does_not_duplicate_pairing_completed_audit() {
        let _guard = TEST_GATEWAY_LOCK.lock().unwrap();
        let Some((temp, config, _)) = test_listener_config() else {
            return;
        };
        let _gateway = start_mobile_gateway_with_config(temp.path(), &config).unwrap();
        let state = active_gateway().unwrap();
        let now = Utc::now();
        let invitation = state
            .pairing
            .begin_pairing([9; 32], now + PAIRING_LIFETIME)
            .unwrap();
        let enrolled = state
            .pairing
            .enroll(
                invitation.id,
                invitation.nonce,
                sample_pairing_request(invitation.nonce),
                state.host_fingerprint,
                now,
            )
            .unwrap();
        assert_eq!(
            state
                .pairing
                .confirm_device(invitation.id, &enrolled.phrase, now)
                .unwrap(),
            PairingProgress::Pending
        );

        let path = format!("/api/v1/internal/mobile/pairings/{}/confirm", invitation.id);
        let body = serde_json::json!({ "phrase": enrolled.phrase }).to_string();

        let first = handle_local_control("POST", &path, Some(&body))
            .unwrap()
            .unwrap();
        let replay = handle_local_control("POST", &path, Some(&body))
            .unwrap()
            .unwrap();

        assert_eq!(first.status, 200);
        assert_eq!(replay.status, 200);
        assert_eq!(replay.body, first.body);

        let audit = std::fs::read_to_string(temp.path().join("mobile/audit.jsonl")).unwrap();
        assert_eq!(audit.matches("\"event\":\"pairing_completed\"").count(), 1);
    }
}
