//! Bounded opaque WebSocket rendezvous.
//!
//! A room is addressed by a canonical 32-byte base64url identifier and guarded
//! by a separate canonical 32-byte bearer credential. The first peer creates
//! the ephemeral room; one `host` and one `client` may be connected at a time.
//! Only binary application frames are forwarded. The relay does not persist or
//! interpret them and is not an authentication or authorization authority.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, Mutex};
use tokio::time::Instant;

const RELAY_PROTOCOL_VERSION: &str = "1";
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_MAX_ROOMS: usize = 1_024;
const DEFAULT_CHANNEL_CAPACITY: usize = 32;

#[derive(Clone)]
pub struct RelayState {
    inner: Arc<Mutex<RelayRegistry>>,
    next_peer_id: Arc<AtomicU64>,
    limits: RelayLimits,
}

impl Default for RelayState {
    fn default() -> Self {
        Self::with_limits(RelayLimits {
            max_rooms: DEFAULT_MAX_ROOMS,
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
        })
    }
}

impl RelayState {
    fn with_limits(limits: RelayLimits) -> Self {
        assert!(limits.max_rooms > 0);
        assert!(limits.channel_capacity > 0);
        Self {
            inner: Arc::new(Mutex::new(RelayRegistry::default())),
            next_peer_id: Arc::new(AtomicU64::new(1)),
            limits,
        }
    }

    async fn register(
        &self,
        room_id: &str,
        credential: &str,
        role: PeerRole,
    ) -> Result<Registration, RegistrationError> {
        let mut registry = self.inner.lock().await;
        if !registry.rooms.contains_key(room_id) {
            if registry.rooms.len() >= self.limits.max_rooms {
                return Err(RegistrationError::RelayFull);
            }
            registry.rooms.insert(
                room_id.to_owned(),
                RelayRoom {
                    credential: credential.to_owned(),
                    host: None,
                    client: None,
                },
            );
        }

        let room = registry.rooms.get_mut(room_id).expect("room exists");
        if !secret_eq(room.credential.as_bytes(), credential.as_bytes()) {
            return Err(RegistrationError::AuthorizationFailed);
        }
        let slot = room.slot_mut(role);
        if slot.is_some() {
            return Err(RegistrationError::RoleOccupied);
        }

        let peer_id = self.next_peer_id.fetch_add(1, Ordering::Relaxed);
        let (sender, inbox) = mpsc::channel(self.limits.channel_capacity);
        *slot = Some(ConnectedPeer {
            id: peer_id,
            sender,
        });
        Ok(Registration {
            room_id: room_id.to_owned(),
            role,
            peer_id,
            inbox,
        })
    }

    async fn peer_sender(&self, room_id: &str, role: PeerRole) -> Option<mpsc::Sender<Message>> {
        let registry = self.inner.lock().await;
        registry
            .rooms
            .get(room_id)
            .and_then(|room| room.peer(role))
            .map(|peer| peer.sender.clone())
    }

    async fn unregister(&self, room_id: &str, role: PeerRole, peer_id: u64) {
        let peer_to_notify = {
            let mut registry = self.inner.lock().await;
            let (peer_to_notify, remove_room) = {
                let Some(room) = registry.rooms.get_mut(room_id) else {
                    return;
                };
                let slot = room.slot_mut(role);
                if !slot.as_ref().is_some_and(|peer| peer.id == peer_id) {
                    return;
                }
                *slot = None;
                (
                    room.peer(role).map(|peer| peer.sender.clone()),
                    room.host.is_none() && room.client.is_none(),
                )
            };
            if remove_room {
                registry.rooms.remove(room_id);
            }
            peer_to_notify
        };
        if let Some(peer) = peer_to_notify {
            let _ = peer.try_send(close_message(1001, "relay peer disconnected"));
        }
    }

    #[cfg(test)]
    async fn room_count(&self) -> usize {
        self.inner.lock().await.rooms.len()
    }
}

#[derive(Clone, Copy)]
struct RelayLimits {
    max_rooms: usize,
    channel_capacity: usize,
}

#[derive(Default)]
struct RelayRegistry {
    rooms: HashMap<String, RelayRoom>,
}

struct RelayRoom {
    credential: String,
    host: Option<ConnectedPeer>,
    client: Option<ConnectedPeer>,
}

impl RelayRoom {
    fn slot_mut(&mut self, role: PeerRole) -> &mut Option<ConnectedPeer> {
        match role {
            PeerRole::Host => &mut self.host,
            PeerRole::Client => &mut self.client,
        }
    }

    fn peer(&self, role: PeerRole) -> Option<&ConnectedPeer> {
        match role {
            PeerRole::Host => self.client.as_ref(),
            PeerRole::Client => self.host.as_ref(),
        }
    }
}

struct ConnectedPeer {
    id: u64,
    sender: mpsc::Sender<Message>,
}

struct Registration {
    room_id: String,
    role: PeerRole,
    peer_id: u64,
    inbox: mpsc::Receiver<Message>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerRole {
    Host,
    Client,
}

impl PeerRole {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "host" => Some(Self::Host),
            "client" => Some(Self::Client),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistrationError {
    RelayFull,
    AuthorizationFailed,
    RoleOccupied,
}

impl RegistrationError {
    fn close(self) -> (u16, &'static str) {
        match self {
            Self::RelayFull => (1013, "relay capacity reached"),
            Self::AuthorizationFailed => (1008, "relay authorization failed"),
            Self::RoleOccupied => (1008, "relay role already connected"),
        }
    }
}

struct RelayRequest {
    room_id: String,
    credential: String,
    role: PeerRole,
}

impl RelayRequest {
    fn parse(query: Option<&str>, headers: &HeaderMap) -> Result<Self, RequestError> {
        let query = query.ok_or(RequestError::InvalidQuery)?;
        let mut version = None;
        let mut room_id = None;
        let mut role = None;
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or(RequestError::InvalidQuery)?;
            let target = match name {
                "v" => &mut version,
                "room" => &mut room_id,
                "role" => &mut role,
                _ => return Err(RequestError::InvalidQuery),
            };
            if value.is_empty() || target.replace(value).is_some() {
                return Err(RequestError::InvalidQuery);
            }
        }
        if version != Some(RELAY_PROTOCOL_VERSION) {
            return Err(RequestError::ProtocolUnsupported);
        }
        let room_id = room_id.ok_or(RequestError::InvalidQuery)?;
        if !is_canonical_32_byte_base64url(room_id) {
            return Err(RequestError::InvalidQuery);
        }
        let role = role
            .and_then(PeerRole::parse)
            .ok_or(RequestError::InvalidQuery)?;
        Ok(Self {
            room_id: room_id.to_owned(),
            credential: bearer_credential(headers)?.to_owned(),
            role,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestError {
    InvalidQuery,
    ProtocolUnsupported,
    AuthorizationRequired,
}

impl RequestError {
    fn response(self) -> Response {
        match self {
            Self::InvalidQuery => {
                (StatusCode::BAD_REQUEST, "invalid relay rendezvous request").into_response()
            }
            Self::ProtocolUnsupported => (
                StatusCode::BAD_REQUEST,
                "unsupported relay protocol version",
            )
                .into_response(),
            Self::AuthorizationRequired => (
                StatusCode::UNAUTHORIZED,
                [("www-authenticate", "Bearer")],
                "relay bearer authorization required",
            )
                .into_response(),
        }
    }
}

pub async fn handler(
    State(state): State<RelayState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let request = match RelayRequest::parse(query.as_deref(), &headers) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| serve(socket, state, request))
}

async fn serve(mut socket: WebSocket, state: RelayState, request: RelayRequest) {
    let registration = match state
        .register(&request.room_id, &request.credential, request.role)
        .await
    {
        Ok(registration) => registration,
        Err(error) => {
            let (code, reason) = error.close();
            let _ = socket.send(close_message(code, reason)).await;
            return;
        }
    };
    let Registration {
        room_id,
        role,
        peer_id,
        mut inbox,
    } = registration;
    relay_loop(&mut socket, &state, &room_id, role, &mut inbox).await;
    state.unregister(&room_id, role, peer_id).await;
}

async fn relay_loop(
    socket: &mut WebSocket,
    state: &RelayState,
    room_id: &str,
    role: PeerRole,
    inbox: &mut mpsc::Receiver<Message>,
) {
    let idle = tokio::time::sleep(IDLE_TIMEOUT);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                idle.as_mut().reset(Instant::now() + IDLE_TIMEOUT);
                let Some(Ok(message)) = incoming else { break; };
                match message {
                    Message::Binary(payload) => {
                        if let Err(close) = forward_to_peer(
                            state, room_id, role, Message::Binary(payload),
                        ).await {
                            let _ = socket.send(close).await;
                            break;
                        }
                    }
                    Message::Text(_) => {
                        let _ = socket.send(close_message(1003, "binary frames required")).await;
                        break;
                    }
                    Message::Ping(_) | Message::Pong(_) => {}
                    Message::Close(frame) => {
                        if let Some(peer) = state.peer_sender(room_id, role).await {
                            let _ = peer.try_send(Message::Close(frame));
                        }
                        break;
                    }
                }
            }
            outgoing = inbox.recv() => {
                idle.as_mut().reset(Instant::now() + IDLE_TIMEOUT);
                let Some(outgoing) = outgoing else { break; };
                let is_close = matches!(outgoing, Message::Close(_));
                if socket.send(outgoing).await.is_err() || is_close {
                    break;
                }
            }
            () = &mut idle => {
                let _ = socket.send(close_message(1001, "relay idle timeout")).await;
                break;
            }
        }
    }
}

async fn forward_to_peer(
    state: &RelayState,
    room_id: &str,
    role: PeerRole,
    message: Message,
) -> Result<(), Message> {
    let peer = state
        .peer_sender(room_id, role)
        .await
        .ok_or_else(|| close_message(1013, "relay peer unavailable"))?;
    match peer.try_send(message) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(close_message(1013, "relay backpressure")),
        Err(TrySendError::Closed(_)) => Err(close_message(1001, "relay peer disconnected")),
    }
}

fn bearer_credential(headers: &HeaderMap) -> Result<&str, RequestError> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    values
        .next()
        .filter(|_| values.next().is_none())
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| is_canonical_32_byte_base64url(value))
        .ok_or(RequestError::AuthorizationRequired)
}

fn is_canonical_32_byte_base64url(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && value
            .as_bytes()
            .last()
            .is_some_and(|byte| b"AEIMQUYcgkosw048".contains(byte))
}

fn secret_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn close_message(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

#[cfg(test)]
mod tests;
