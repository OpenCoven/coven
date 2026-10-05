//! Real HTTP/1.1 upgrades and RFC 6455 fixtures over ephemeral loopback TCP.
//! No client dependency is exposed by this binary crate. This deliberately small
//! test-only codec emits masked client frames and checks unmasked server frames;
//! it is never an application transport or a production protocol implementation.
use super::*;
use axum::{routing::any, Router};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::task::{Context, Poll, Waker};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

// Hang guards, not promptness assertions. Production send/idle deadlines stay
// untouched; predicates and write notifications establish ordering.
const HANG_GUARD: Duration = Duration::from_secs(30);
// Public RFC 6455 handshake vector: base64 of "the sample nonce".
const HANDSHAKE_NONCE: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const ROOM: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const CREDENTIAL: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCA";

async fn guarded<T>(future: impl Future<Output = T>) -> T {
    let started = Instant::now();
    tokio::time::timeout(HANG_GUARD, future)
        .await
        .unwrap_or_else(|_| panic!("wire test hung after {:?}", started.elapsed()))
}

#[derive(Default)]
struct WriteControl {
    mode: StdMutex<(WriteMode, Option<Waker>)>,
    attempted: Notify,
}

#[derive(Default, Clone, Copy)]
enum WriteMode {
    #[default]
    Open,
    Blocked,
    Failed,
}

impl WriteControl {
    fn set(&self, mode: WriteMode) {
        let mut state = self.mode.lock().unwrap();
        state.0 = mode;
        if let Some(waker) = state.1.take() {
            waker.wake();
        }
    }

    fn poll(&self, cx: &Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.mode.lock().unwrap();
        match state.0 {
            WriteMode::Open => Poll::Ready(Ok(())),
            WriteMode::Blocked => {
                state.1 = Some(cx.waker().clone());
                self.attempted.notify_one();
                Poll::Pending
            }
            WriteMode::Failed => {
                self.attempted.notify_one();
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "fixture write failure",
                )))
            }
        }
    }
}

struct ControlledSocket {
    stream: TcpStream,
    writes: Arc<WriteControl>,
}

impl AsyncRead for ControlledSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for ControlledSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.writes.poll(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut self.stream).poll_write(cx, buf),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

struct FixtureListener {
    listener: TcpListener,
    accepted: mpsc::UnboundedSender<Arc<WriteControl>>,
}

impl axum::serve::Listener for FixtureListener {
    type Io = ControlledSocket;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, addr) = self.listener.accept().await.unwrap();
        let writes = Arc::new(WriteControl::default());
        self.accepted.send(writes.clone()).unwrap();
        (ControlledSocket { stream, writes }, addr)
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

struct Server {
    addr: SocketAddr,
    state: RelayState,
    accepted: mpsc::UnboundedReceiver<Arc<WriteControl>>,
    task: JoinHandle<()>,
}

impl Server {
    async fn start(limits: RelayLimits) -> Self {
        let state = RelayState::with_limits(limits);
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, accepted) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/ws", any(handler))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(
                FixtureListener {
                    listener,
                    accepted: tx,
                },
                app,
            )
            .await
            .unwrap();
        });
        Self {
            addr,
            state,
            accepted,
            task,
        }
    }

    async fn connect(&mut self, query: &str, auth: &str) -> (Peer, String) {
        let mut stream = guarded(TcpStream::connect(self.addr)).await.unwrap();
        let writes = guarded(self.accepted.recv()).await.unwrap();
        let request = format!("GET /ws?{query} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_NONCE}\r\n{auth}\r\n", self.addr);
        guarded(stream.write_all(request.as_bytes())).await.unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            response.push(guarded(stream.read_u8()).await.unwrap());
            assert!(response.len() < 8192, "oversized fixture HTTP response");
        }
        if !response.starts_with(b"HTTP/1.1 101 ") {
            let headers = String::from_utf8(response.clone()).unwrap();
            let len: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .expect("fixture expects a fixed HTTP error body");
            assert!(len < 8192);
            let mut body = vec![0; len];
            guarded(stream.read_exact(&mut body)).await.unwrap();
            response.extend_from_slice(&body);
        }
        (
            Peer { stream, writes },
            String::from_utf8(response).unwrap(),
        )
    }

    async fn peer(&mut self, role: PeerRole) -> Peer {
        let role_name = match role {
            PeerRole::Host => "host",
            PeerRole::Client => "client",
        };
        let (peer, response) = self
            .connect(
                &format!("v=1&room={ROOM}&role={role_name}"),
                &format!("Authorization: Bearer {CREDENTIAL}\r\n"),
            )
            .await;
        assert!(response.starts_with("HTTP/1.1 101 "), "{response}");
        assert!(
            response
                .to_ascii_lowercase()
                .contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
            "{response}"
        );
        // A 101 response precedes registration. Observe the actual slot rather
        // than sleeping or assuming the upgrade callback has already run.
        self.until(|| async {
            let registry = self.state.inner.lock().await;
            registry.rooms.get(ROOM).is_some_and(|room| match role {
                PeerRole::Host => room.host.is_some(),
                PeerRole::Client => room.client.is_some(),
            })
        })
        .await;
        peer
    }

    async fn until<F, Fut>(&self, predicate: F)
    where
        F: Fn() -> Fut,
        Fut: Future<Output = bool>,
    {
        guarded(async {
            while !predicate().await {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }

    async fn released(&self) {
        self.until(|| async { self.state.room_count().await == 0 })
            .await;
        assert_eq!(
            self.state.queued_bytes.available_permits(),
            self.state.limits.max_queued_bytes
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Peer {
    stream: TcpStream,
    writes: Arc<WriteControl>,
}

impl Peer {
    async fn frame(&mut self, fin: bool, opcode: u8, payload: &[u8]) {
        self.raw_frame(fin, opcode, payload, true).await;
    }

    async fn raw_frame(&mut self, fin: bool, opcode: u8, payload: &[u8], masked: bool) {
        let mask_bit = if masked { 0x80 } else { 0 };
        let mut bytes = vec![(if fin { 0x80 } else { 0 }) | opcode];
        match payload.len() {
            0..=125 => bytes.push(mask_bit | payload.len() as u8),
            126..=65535 => {
                bytes.push(mask_bit | 126);
                bytes.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            }
            _ => {
                bytes.push(mask_bit | 127);
                bytes.extend_from_slice(&(payload.len() as u64).to_be_bytes());
            }
        }
        let mask = [0x12, 0x34, 0x56, 0x78];
        if masked {
            bytes.extend_from_slice(&mask);
        }
        bytes.extend(
            payload
                .iter()
                .enumerate()
                .map(|(i, b)| if masked { b ^ mask[i % 4] } else { *b }),
        );
        guarded(self.stream.write_all(&bytes)).await.unwrap();
    }

    async fn recv(&mut self) -> (u8, Vec<u8>) {
        guarded(async {
            let first = self.stream.read_u8().await.unwrap();
            let second = self.stream.read_u8().await.unwrap();
            assert_eq!(first & 0x70, 0, "unexpected server RSV bits");
            assert_ne!(
                first & 0x80,
                0,
                "fixture expects unfragmented server messages"
            );
            assert_eq!(second & 0x80, 0, "server must not mask frames");
            let len = match second & 0x7f {
                126 => self.stream.read_u16().await.unwrap() as usize,
                127 => usize::try_from(self.stream.read_u64().await.unwrap()).unwrap(),
                len => len as usize,
            };
            assert!(len <= MAX_MESSAGE_BYTES, "oversized server frame");
            let mut payload = vec![0; len];
            self.stream.read_exact(&mut payload).await.unwrap();
            (first & 0xf, payload)
        })
        .await
    }

    async fn close(&mut self, code: u16) {
        let (opcode, payload) = self.recv().await;
        assert_eq!(opcode, 8);
        assert!(payload.len() >= 2);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), code);
    }
}

fn limits() -> RelayLimits {
    RelayLimits {
        max_rooms: 2,
        channel_capacity: 2,
        max_queued_bytes: DEFAULT_MAX_QUEUED_BYTES,
    }
}

#[tokio::test]
async fn rendezvous_exchanges_opaque_binary_in_both_arrival_orders() {
    for first_role in [PeerRole::Host, PeerRole::Client] {
        let mut server = Server::start(limits()).await;
        let mut first = server.peer(first_role).await;
        let mut second = server
            .peer(match first_role {
                PeerRole::Host => PeerRole::Client,
                PeerRole::Client => PeerRole::Host,
            })
            .await;
        for payload in [
            vec![],
            vec![0, 255, 1, 128, 13, 10],
            (0..MAX_FRAME_BYTES).map(|i| i as u8).collect(),
        ] {
            first.frame(true, 2, &payload).await;
            assert_eq!(second.recv().await, (2, payload.clone()));
            second.frame(true, 2, &payload).await;
            assert_eq!(first.recv().await, (2, payload));
        }
        first.frame(true, 8, &1000u16.to_be_bytes()).await;
        second.close(1000).await;
        server.released().await;
    }
}

#[tokio::test]
async fn malformed_http_rendezvous_fails_before_registration() {
    let mut server = Server::start(limits()).await;
    let valid = format!("v=1&room={ROOM}&role=host");
    let auth = format!("Authorization: Bearer {CREDENTIAL}\r\n");
    for query in [
        "".to_owned(),
        format!("room={ROOM}&role=host"),
        format!("v=2&room={ROOM}&role=host"),
        format!("v=1&room={ROOM}&role=host&role=client"),
        format!("v=1&room={ROOM}%3d&role=host"),
        "v=1&room=short&role=host".into(),
        format!("v=1&room={ROOM}&role=admin"),
        format!("v=1&room={ROOM}&role=host&credential={CREDENTIAL}"),
    ] {
        let (_, response) = server.connect(&query, &auth).await;
        assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
        assert!(!response.contains(ROOM));
        assert!(!response.contains(CREDENTIAL));
    }
    for auth in [
        "".into(),
        "Authorization: Basic invalid\r\n".into(),
        format!("Authorization: Bearer {CREDENTIAL}\r\nAuthorization: Bearer {CREDENTIAL}\r\n"),
        format!("Authorization: Bearer {CREDENTIAL}, Bearer {CREDENTIAL}\r\n"),
    ] {
        let (_, response) = server.connect(&valid, &auth).await;
        assert!(response.starts_with("HTTP/1.1 401 "), "{response}");
        assert!(response
            .to_ascii_lowercase()
            .contains("www-authenticate: bearer"));
        assert!(!response.contains(CREDENTIAL));
    }
    server.released().await;
}

#[tokio::test]
async fn wrong_credentials_duplicate_roles_and_room_capacity_fail_closed() {
    let mut bounded = limits();
    bounded.max_rooms = 1;
    let mut server = Server::start(bounded).await;
    let mut host = server.peer(PeerRole::Host).await;
    for (query, auth, code) in [
        (
            format!("v=1&room={ROOM}&role=client"),
            format!("Authorization: Bearer {}\r\n", "D".repeat(42) + "A"),
            1008,
        ),
        (
            format!("v=1&room={ROOM}&role=host"),
            format!("Authorization: Bearer {CREDENTIAL}\r\n"),
            1008,
        ),
        (
            format!("v=1&room={}&role=host", "B".repeat(42) + "A"),
            format!("Authorization: Bearer {CREDENTIAL}\r\n"),
            1013,
        ),
    ] {
        let (mut rejected, response) = server.connect(&query, &auth).await;
        assert!(response.starts_with("HTTP/1.1 101 "));
        rejected.close(code).await;
        assert_eq!(server.state.room_count().await, 1);
    }
    let mut client = server.peer(PeerRole::Client).await;
    host.frame(true, 2, b"still connected").await;
    assert_eq!(client.recv().await, (2, b"still connected".to_vec()));
    drop(client);
    host.close(1001).await;
    server.released().await;
}

#[tokio::test]
async fn text_unpaired_binary_and_invalid_wire_traffic_fail_closed() {
    for (opcode, masked, payload) in [
        (1, true, b"private text".as_slice()),
        (2, false, b"unmasked".as_slice()),
        (3, true, b"reserved opcode".as_slice()),
        (9, true, &[0; 126]),
    ] {
        let mut server = Server::start(limits()).await;
        let mut host = server.peer(PeerRole::Host).await;
        let mut client = server.peer(PeerRole::Client).await;
        host.raw_frame(true, opcode, payload, masked).await;
        if opcode == 1 {
            host.close(1003).await;
        }
        client.close(1001).await;
        server.released().await;
    }
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    host.frame(true, 2, b"no offline buffer").await;
    host.close(1013).await;
    server.released().await;
}

#[tokio::test]
async fn fragmented_message_at_limit_passes_and_oversized_frame_fails() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut client = server.peer(PeerRole::Client).await;
    let part = vec![0xfe; MAX_FRAME_BYTES];
    for i in 0..MAX_MESSAGE_BYTES / MAX_FRAME_BYTES {
        host.frame(
            i == MAX_MESSAGE_BYTES / MAX_FRAME_BYTES - 1,
            if i == 0 { 2 } else { 0 },
            &part,
        )
        .await;
    }
    assert_eq!(client.recv().await, (2, vec![0xfe; MAX_MESSAGE_BYTES]));
    host.frame(true, 2, &vec![0xff; MAX_FRAME_BYTES + 1]).await;
    client.close(1001).await;
    server.released().await;
}

#[tokio::test]
async fn fragmented_message_cannot_bypass_message_limit() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut client = server.peer(PeerRole::Client).await;
    for i in 0..MAX_MESSAGE_BYTES / MAX_FRAME_BYTES {
        host.frame(
            false,
            if i == 0 { 2 } else { 0 },
            &vec![0xab; MAX_FRAME_BYTES],
        )
        .await;
    }
    host.frame(true, 0, &[1]).await;
    client.close(1001).await;
    server.released().await;
}

#[tokio::test]
async fn failed_socket_send_releases_room_and_byte_permits() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let client = server.peer(PeerRole::Client).await;
    client.writes.set(WriteMode::Failed);
    host.frame(true, 2, b"send failure payload").await;
    guarded(client.writes.attempted.notified()).await;
    host.close(1001).await;
    server.released().await;
}

#[tokio::test]
async fn full_queue_does_not_lose_peer_disconnect() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut client = server.peer(PeerRole::Client).await;
    client.writes.set(WriteMode::Blocked);
    host.frame(true, 2, b"in flight").await;
    guarded(client.writes.attempted.notified()).await;
    host.frame(true, 2, b"queued one").await;
    host.frame(true, 2, b"queued two").await;
    server
        .until(|| async {
            server.state.queued_bytes.available_permits() == DEFAULT_MAX_QUEUED_BYTES - 29
        })
        .await;
    drop(host);
    server
        .until(|| async {
            server
                .state
                .inner
                .lock()
                .await
                .rooms
                .get(ROOM)
                .unwrap()
                .host
                .is_none()
        })
        .await;
    client.writes.set(WriteMode::Open);
    // In-flight delivery may finish, but disconnect must survive a full inbox.
    loop {
        let (opcode, _) = client.recv().await;
        if opcode == 8 {
            break;
        }
        assert_eq!(opcode, 2);
    }
    server.released().await;
}

#[tokio::test]
async fn blocked_send_expires_without_retaining_room_or_permits() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let client = server.peer(PeerRole::Client).await;
    client.writes.set(WriteMode::Blocked);
    host.frame(true, 2, b"held until send deadline").await;
    guarded(client.writes.attempted.notified()).await;
    assert_eq!(
        server.state.queued_bytes.available_permits(),
        DEFAULT_MAX_QUEUED_BYTES - 24
    );
    host.close(1001).await;
    server.released().await;
}

#[derive(Clone, Default)]
struct DiagnosticBuffer(Arc<StdMutex<Vec<u8>>>);

impl io::Write for DiagnosticBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn verbose_diagnostics_do_not_expose_credentials_or_payloads() {
    let captured = DiagnosticBuffer::default();
    let writer = captured.clone();
    use tracing_subscriber::util::SubscriberInitExt;
    crate::diagnostic_subscriber(tracing_subscriber::EnvFilter::new("trace"), move || {
        writer.clone()
    })
    .init();
    tracing::info!(target: "coven_relay::checkpoint", "diagnostic capture active");
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut client = server.peer(PeerRole::Client).await;
    host.frame(true, 2, b"private-payload-marker").await;
    assert_eq!(client.recv().await, (2, b"private-payload-marker".to_vec()));
    let mut close = 1000u16.to_be_bytes().to_vec();
    close.extend_from_slice(b"private-close-marker");
    host.frame(true, 8, &close).await;
    client.close(1000).await;
    server.released().await;
    let diagnostics = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        diagnostics.contains("diagnostic capture active"),
        "capture was inactive"
    );
    for secret in [
        ROOM,
        CREDENTIAL,
        "private-payload-marker",
        "private-close-marker",
        "707269766174652d7061796c6f61642d6d61726b6572",
    ] {
        assert!(
            !diagnostics.contains(secret),
            "sensitive data in verbose diagnostics"
        );
    }
}

#[tokio::test]
async fn ping_pong_is_local_and_fragmentation_remains_opaque() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut client = server.peer(PeerRole::Client).await;
    host.frame(false, 2, &[0, 255]).await;
    host.frame(true, 9, b"transport-local").await;
    assert_eq!(host.recv().await, (10, b"transport-local".to_vec()));
    host.frame(true, 10, b"unsolicited pong").await;
    host.frame(true, 0, &[128, 1]).await;
    assert_eq!(client.recv().await, (2, vec![0, 255, 128, 1]));
    drop(host);
    client.close(1001).await;
    server.released().await;
}

#[tokio::test]
async fn queue_overflow_closes_sender_and_releases_buffered_permits() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut client = server.peer(PeerRole::Client).await;
    client.writes.set(WriteMode::Blocked);
    host.frame(true, 2, b"12345").await;
    guarded(client.writes.attempted.notified()).await;
    host.frame(true, 2, b"12345").await;
    host.frame(true, 2, b"12345").await;
    server
        .until(|| async {
            server.state.queued_bytes.available_permits() == DEFAULT_MAX_QUEUED_BYTES - 15
        })
        .await;
    host.frame(true, 2, b"overflow").await;
    host.close(1013).await;
    client.writes.set(WriteMode::Open);
    assert_eq!(client.recv().await, (2, b"12345".to_vec()));
    client.close(1001).await;
    server.released().await;
}

#[tokio::test]
async fn byte_budget_overflow_closes_sender_without_leaking_permits() {
    let mut bounded = limits();
    bounded.max_queued_bytes = 5;
    let mut server = Server::start(bounded).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut client = server.peer(PeerRole::Client).await;
    client.writes.set(WriteMode::Blocked);
    host.frame(true, 2, b"12345").await;
    guarded(client.writes.attempted.notified()).await;
    assert_eq!(server.state.queued_bytes.available_permits(), 0);
    host.frame(true, 2, b"6").await;
    host.close(1013).await;
    assert_eq!(server.state.queued_bytes.available_permits(), 0);
    client.writes.set(WriteMode::Open);
    assert_eq!(client.recv().await, (2, b"12345".to_vec()));
    client.close(1001).await;
    server.released().await;
}

#[tokio::test]
async fn failed_http_upgrade_does_not_allocate_or_notify_existing_peer() {
    let mut bounded = limits();
    bounded.max_rooms = 1;
    let mut server = Server::start(bounded).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut stream = guarded(TcpStream::connect(server.addr)).await.unwrap();
    let writes = guarded(server.accepted.recv()).await.unwrap();
    writes.set(WriteMode::Failed);
    let request = format!("GET /ws?v=1&room={ROOM}&role=client HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_NONCE}\r\nAuthorization: Bearer {CREDENTIAL}\r\n\r\n");
    guarded(stream.write_all(request.as_bytes())).await.unwrap();
    guarded(writes.attempted.notified()).await;
    let mut response = Vec::new();
    guarded(stream.read_to_end(&mut response)).await.unwrap();
    assert!(
        response.is_empty(),
        "failed upgrade unexpectedly returned data"
    );
    assert_eq!(server.state.room_count().await, 1);
    assert!(server
        .state
        .peer_sender(ROOM, PeerRole::Host)
        .await
        .is_none());
    // A subsequent genuine upgrade can claim the same role; the existing host
    // receives only this binary frame, never a phantom disconnect notification.
    let mut client = server.peer(PeerRole::Client).await;
    client.frame(true, 2, b"after failed upgrade").await;
    assert_eq!(host.recv().await, (2, b"after failed upgrade".to_vec()));
    drop(client);
    host.close(1001).await;
    server.released().await;
}

#[tokio::test]
async fn incomplete_and_non_websocket_requests_leave_no_owned_resources() {
    let mut server = Server::start(limits()).await;
    let mut stream = guarded(TcpStream::connect(server.addr)).await.unwrap();
    let _writes = guarded(server.accepted.recv()).await.unwrap();
    guarded(stream.write_all(b"GET /ws?v=1 HTTP/1.1\r\nHost: localhost\r\n"))
        .await
        .unwrap();
    stream.shutdown().await.unwrap();
    let mut response = Vec::new();
    guarded(stream.read_to_end(&mut response)).await.unwrap();
    server.released().await;
    let mut stream = guarded(TcpStream::connect(server.addr)).await.unwrap();
    let _writes = guarded(server.accepted.recv()).await.unwrap();
    let request = format!("GET /ws?v=1&room={ROOM}&role=host HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {CREDENTIAL}\r\n\r\n");
    guarded(stream.write_all(request.as_bytes())).await.unwrap();
    response.clear();
    guarded(stream.read_to_end(&mut response)).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 400 "));
    server.released().await;
}

#[tokio::test]
async fn idle_peer_expires_at_the_unchanged_production_deadline() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    // This case deliberately waits for the real 120-second server timer. Its
    // separate hang guard includes that deadline; no test seam shortens it.
    let started = Instant::now();
    let first = tokio::time::timeout(IDLE_TIMEOUT + HANG_GUARD, host.stream.read_u8())
        .await
        .unwrap_or_else(|_| panic!("idle expiry hung after {:?}", started.elapsed()))
        .unwrap();
    assert_eq!(first, 0x88);
    let len = guarded(host.stream.read_u8()).await.unwrap();
    assert!(len <= 125, "invalid close frame length");
    let mut payload = vec![0; usize::from(len)];
    guarded(host.stream.read_exact(&mut payload)).await.unwrap();
    assert_eq!(&payload[..2], &1001u16.to_be_bytes());
    assert_eq!(&payload[2..], b"relay idle timeout");
    server.released().await;
}

#[tokio::test]
async fn graceful_close_acknowledges_initiator_and_notifies_peer() {
    let mut server = Server::start(limits()).await;
    let mut host = server.peer(PeerRole::Host).await;
    let mut client = server.peer(PeerRole::Client).await;
    host.frame(true, 8, &1000u16.to_be_bytes()).await;
    host.close(1000).await;
    client.close(1000).await;
    server.released().await;
}
