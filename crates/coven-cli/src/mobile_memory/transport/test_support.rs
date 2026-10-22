//! In-process production relay with a fixture TLS listener, not a simulated broker.
use super::{cancellation, websocket};
use anyhow::{Context, Result};
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

struct TlsListener {
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (socket, address) = self.listener.accept().await.unwrap();
            if let Ok(Ok(stream)) =
                tokio::time::timeout(Duration::from_secs(30), self.acceptor.accept(socket)).await
            {
                return (stream, address);
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
        }
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
