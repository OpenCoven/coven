//! In-process production relay with a fixture TLS listener, not a simulated broker.
use super::{cancellation, websocket, Pairing};
use anyhow::{Context, Result};
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::oneshot;

/// Bytes exactly as the relay process sees them after TLS termination, in both
/// directions and including the HTTP upgrade. This is the broker's vantage
/// point for the "no plaintext or stable identifier" evidence.
pub(crate) type Capture = Arc<Mutex<Vec<u8>>>;

pub(crate) struct CapturingIo<S> {
    inner: S,
    capture: Capture,
}

impl<S: AsyncRead + Unpin> AsyncRead for CapturingIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            this.capture
                .lock()
                .unwrap()
                .extend_from_slice(&buf.filled()[before..]);
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CapturingIo<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(cx, bytes);
        if let Poll::Ready(Ok(written)) = &result {
            this.capture
                .lock()
                .unwrap()
                .extend_from_slice(&bytes[..*written]);
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

struct TlsListener {
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    capture: Capture,
}

impl axum::serve::Listener for TlsListener {
    type Io = CapturingIo<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (socket, address) = self.listener.accept().await.unwrap();
            if let Ok(Ok(stream)) =
                tokio::time::timeout(Duration::from_secs(30), self.acceptor.accept(socket)).await
            {
                return (
                    CapturingIo {
                        inner: stream,
                        capture: Arc::clone(&self.capture),
                    },
                    address,
                );
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

pub(crate) struct Relay {
    address: SocketAddr,
    endpoint: websocket::RelayEndpoint,
    configuration: websocket::EndpointConfig,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
    capture: Capture,
}

impl Relay {
    pub(crate) fn start() -> Self {
        use base64::Engine;
        let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let root = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(certificate.cert.der());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der())
                .into(),
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let configuration = websocket::EndpointConfig {
            url: format!("wss://localhost:{}/ws", address.port()),
            ca_certificates: vec![root],
        };
        let endpoint = configuration.parse().unwrap();
        let (stop, stopped) = oneshot::channel();
        let capture: Capture = Arc::default();
        let listener_capture = Arc::clone(&capture);
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = TlsListener {
                    listener: tokio::net::TcpListener::from_std(listener).unwrap(),
                    acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)),
                    capture: listener_capture,
                };
                tokio::select! {
                    result = axum::serve(listener, coven_relay::router()).into_future() => result.unwrap(),
                    _ = stopped => {},
                }
            });
            runtime.shutdown_timeout(Duration::from_secs(30));
        });
        Self {
            address,
            endpoint,
            configuration,
            stop: Some(stop),
            thread: Some(thread),
            capture,
        }
    }

    /// Every byte the relay has read or written inside TLS so far.
    pub(crate) fn captured(&self) -> Vec<u8> {
        self.capture.lock().unwrap().clone()
    }

    pub(crate) fn configuration(&self) -> websocket::EndpointConfig {
        self.configuration.clone()
    }

    pub(crate) fn join(
        &self,
        role: websocket::Role,
        room: &[u8; 32],
    ) -> websocket::WebSocketRecords {
        self.connect(role, room, &[42; 32]).unwrap()
    }

    pub(crate) fn connect(
        &self,
        role: websocket::Role,
        room: &[u8; 32],
        credential: &[u8; 32],
    ) -> Result<websocket::WebSocketRecords> {
        let cancellation = cancellation::Cancellation::default();
        let deadline = Instant::now() + Duration::from_secs(30);
        let addresses = self.endpoint.resolve(&cancellation, deadline)?;
        let address = addresses
            .into_iter()
            .find(|candidate| *candidate == self.address)
            .context("localhost did not resolve to the fixture listener")?;
        let mut records = websocket::connect_cancellable(
            &self.endpoint,
            address,
            role,
            room,
            credential,
            deadline,
            &cancellation,
        )?;
        records.fixture_registration_barrier()?;
        Ok(records)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.thread.take().unwrap().join().unwrap();
    }
}

/// Stable pairing identifiers in every encoding a broker could search for:
/// raw and textual UUIDs, uncompressed/compressed P-256 points and their x
/// coordinates, and the pairing-scoped Noise static key. Absence of these
/// bytes is necessary evidence, not a confidentiality proof.
pub(crate) fn stable_identifier_markers(pairing: &Pairing) -> Vec<(&'static str, Vec<u8>)> {
    let mut markers = vec![
        ("device id bytes", pairing.device_id.as_bytes().to_vec()),
        ("device id text", pairing.device_id.to_string().into_bytes()),
        ("grant id bytes", pairing.grant_id.as_bytes().to_vec()),
        ("grant id text", pairing.grant_id.to_string().into_bytes()),
        ("noise static key", pairing.noise_key.to_vec()),
    ];
    for (name, key) in [
        ("host key", pairing.host_key),
        ("device key", pairing.device_key),
    ] {
        let uncompressed = key.to_sec1_point(false).as_bytes().to_vec();
        markers.push((name, uncompressed[1..33].to_vec()));
        markers.push((name, uncompressed));
        markers.push((name, key.to_sec1_point(true).as_bytes().to_vec()));
    }
    markers
}

pub(crate) fn assert_bytes_exclude(captured: &[u8], markers: &[(&str, Vec<u8>)]) {
    for (name, marker) in markers {
        assert!(
            !captured.windows(marker.len()).any(|part| part == marker),
            "captured relay bytes contain {name}"
        );
    }
}
