//! Bounded outbound WSS. TLS authenticates only the broker; Noise authenticates peers.
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use tungstenite::{
    client::IntoClientRequest, protocol::WebSocketConfig, stream::MaybeTlsStream, Message,
    WebSocket,
};
use url::Url;
use zeroize::Zeroize;

use super::cancellation::{Attempt, Cancellation};
use super::network::{self, remaining};
use super::{Records, MAX_RECORD_BYTES};

/// No Debug/Display implementation: URLs may carry private deployment metadata.
pub struct RelayEndpoint {
    url: Url,
    trust: Option<Arc<rustls::ClientConfig>>,
}

/// Operator-selected trust is scoped to this endpoint, never learned from a relay.
/// Empty roots select the normal WebPKI roots; nonempty roots replace that set.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EndpointConfig {
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ca_certificates: Vec<String>,
}

impl EndpointConfig {
    pub fn parse(&self) -> Result<RelayEndpoint> {
        let mut endpoint = RelayEndpoint::parse(&self.url)?;
        ensure!(
            self.ca_certificates.len() <= 4,
            "too many relay certificate authorities"
        );
        if !self.ca_certificates.is_empty() {
            let mut roots = rustls::RootCertStore::empty();
            for encoded in &self.ca_certificates {
                ensure!(
                    encoded.len() <= 10923,
                    "relay certificate authority too large"
                );
                let der = URL_SAFE_NO_PAD
                    .decode(encoded)
                    .map_err(|_| anyhow::anyhow!("invalid relay certificate authority"))?;
                ensure!(
                    !der.is_empty() && der.len() <= 8192,
                    "invalid relay certificate authority size"
                );
                ensure!(
                    URL_SAFE_NO_PAD.encode(&der) == *encoded,
                    "noncanonical relay certificate authority"
                );
                roots
                    .add(rustls::pki_types::CertificateDer::from(der))
                    .map_err(|_| anyhow::anyhow!("invalid relay certificate authority"))?;
            }
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
            endpoint.trust = Some(Arc::new(config));
        }
        Ok(endpoint)
    }
}

impl RelayEndpoint {
    /// Resolve only the operator-configured host, with bounded process-wide
    /// resolver admission. Literal addresses bypass DNS entirely.
    pub fn resolve(
        &self,
        cancellation: &Cancellation,
        deadline: Instant,
    ) -> Result<Vec<SocketAddr>> {
        cancellation.ensure_pending()?;
        remaining(deadline)?;
        let port = self
            .url
            .port_or_known_default()
            .ok_or_else(|| anyhow::anyhow!("invalid relay port"))?;
        match self
            .url
            .host()
            .ok_or_else(|| anyhow::anyhow!("invalid relay host"))?
        {
            url::Host::Domain(host) => {
                network::resolve(host.to_owned(), port, cancellation, deadline)
            }
            url::Host::Ipv4(host) => Ok(vec![SocketAddr::new(host.into(), port)]),
            url::Host::Ipv6(host) => Ok(vec![SocketAddr::new(host.into(), port)]),
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        ensure!(value.len() <= 2048, "relay endpoint too long");
        let url = Url::parse(value).map_err(|_| anyhow::anyhow!("invalid relay endpoint"))?;
        ensure!(
            url.scheme() == "wss" && url.host().is_some(),
            "relay endpoint must use WSS"
        );
        ensure!(
            url.username().is_empty() && url.password().is_none(),
            "relay endpoint contains credentials"
        );
        ensure!(
            url.query().is_none() && url.fragment().is_none() && url.path() == "/ws",
            "relay endpoint must use the /ws path without query or fragment"
        );
        ensure!(
            url.port_or_known_default().is_some_and(|port| port != 0),
            "invalid relay port"
        );
        Ok(Self { url, trust: None })
    }
}

#[derive(Clone, Copy)]
pub enum Role {
    Host,
    Client,
}

/// The address must come from resolution of the operator-configured endpoint.
/// DNS resolution belongs to the bounded connection scheduler, not this worker.
/// Redirects are rejected by the single-upgrade client API, never followed.
pub fn connect(
    endpoint: &RelayEndpoint,
    address: SocketAddr,
    role: Role,
    room: &[u8; 32],
    credential: &[u8; 32],
    deadline: Instant,
) -> Result<WebSocketRecords> {
    connect_cancellable(
        endpoint,
        address,
        role,
        room,
        credential,
        deadline,
        &Cancellation::default(),
    )
}

/// A token is single-use, including failed attempts. Cancellation interrupts an
/// pending TCP connection and established socket. The caller supplies one
/// absolute deadline for the complete connection attempt.
pub fn connect_cancellable(
    endpoint: &RelayEndpoint,
    address: SocketAddr,
    role: Role,
    room: &[u8; 32],
    credential: &[u8; 32],
    deadline: Instant,
    cancellation: &Cancellation,
) -> Result<WebSocketRecords> {
    let attempt = cancellation.begin()?;
    super::diagnostics::initialize()?;
    ensure!(
        endpoint.url.port_or_known_default() == Some(address.port()),
        "relay port mismatch"
    );
    let socket = network::connect(address, &attempt, deadline)?;
    let stream = DeadlineStream { socket, deadline };
    let mut url = endpoint.url.clone();
    url.query_pairs_mut()
        .append_pair("v", "1")
        .append_pair("room", &URL_SAFE_NO_PAD.encode(room))
        .append_pair(
            "role",
            match role {
                Role::Host => "host",
                Role::Client => "client",
            },
        );
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| anyhow::anyhow!("invalid relay request"))?;
    let mut header = format!("Bearer {}", URL_SAFE_NO_PAD.encode(credential))
        .parse::<tungstenite::http::HeaderValue>()
        .map_err(|_| anyhow::anyhow!("invalid relay credential"))?;
    header.set_sensitive(true);
    request
        .headers_mut()
        .insert(tungstenite::http::header::AUTHORIZATION, header);
    let connector = endpoint
        .trust
        .as_ref()
        .map(|config| tungstenite::Connector::Rustls(Arc::clone(config)));
    let (socket, _) =
        tungstenite::client_tls_with_config(request, stream, Some(limits()), connector)
            .map_err(|_| anyhow::anyhow!("relay TLS or upgrade failed"))?;
    Ok(WebSocketRecords { socket, attempt })
}

fn limits() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_RECORD_BYTES))
        .max_frame_size(Some(MAX_RECORD_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_RECORD_BYTES * 2)
}

struct DeadlineStream {
    socket: TcpStream,
    deadline: Instant,
}

impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.socket
            .set_read_timeout(Some(remaining(self.deadline)?))?;
        self.socket.read(bytes)
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket
            .set_write_timeout(Some(remaining(self.deadline)?))?;
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        remaining(self.deadline)?;
        self.socket.flush()
    }
}

pub struct WebSocketRecords {
    socket: WebSocket<MaybeTlsStream<DeadlineStream>>,
    attempt: Attempt,
}

impl WebSocketRecords {
    /// Fixture barrier only: a pong proves the real router has registered this
    /// role. It is not endpoint authentication or proof of opposite-peer presence.
    #[cfg(test)]
    pub(super) fn fixture_registration_barrier(&mut self) -> Result<()> {
        self.set_deadline(Instant::now() + std::time::Duration::from_secs(30))?;
        self.socket
            .send(Message::Ping(b"registration-fixture".to_vec().into()))?;
        ensure!(
            matches!(self.socket.read()?, Message::Pong(bytes) if bytes.as_ref() == b"registration-fixture"),
            "relay fixture registration failed"
        );
        Ok(())
    }

    fn set_deadline(&mut self, deadline: Instant) -> Result<()> {
        self.attempt.ensure_active()?;
        remaining(deadline)?;
        match self.socket.get_mut() {
            MaybeTlsStream::Rustls(stream) => stream.sock.deadline = deadline,
            #[cfg(test)]
            MaybeTlsStream::Plain(stream) => stream.deadline = deadline,
            _ => bail!("unencrypted relay transport forbidden"),
        }
        Ok(())
    }
}

impl Records for WebSocketRecords {
    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
        ensure!(bytes.len() <= MAX_RECORD_BYTES, "relay record too large");
        self.set_deadline(deadline)?;
        self.socket
            .send(Message::Binary(bytes.to_vec().into()))
            .map_err(|_| anyhow::anyhow!("relay send failed"))?;
        self.attempt.ensure_active()
    }
    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        loop {
            self.set_deadline(deadline)?;
            let message = self
                .socket
                .read()
                .map_err(|_| anyhow::anyhow!("relay receive failed"))?;
            self.attempt.ensure_active()?;
            match message {
                Message::Binary(bytes) => return Ok(bytes.to_vec()),
                Message::Ping(_) | Message::Pong(_) => self
                    .socket
                    .flush()
                    .map_err(|_| anyhow::anyhow!("relay control write failed"))?,
                _ => bail!("unexpected relay record"),
            }
        }
    }
}

#[cfg(test)]
pub(super) fn fixture_connect(address: SocketAddr) -> Result<WebSocketRecords> {
    fixture_connect_cancellable(address, &Cancellation::default())
}

#[cfg(test)]
fn fixture_connect_cancellable(
    address: SocketAddr,
    cancellation: &Cancellation,
) -> Result<WebSocketRecords> {
    let attempt = cancellation.begin()?;
    super::diagnostics::initialize()?;
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    let stream = DeadlineStream {
        socket: TcpStream::connect_timeout(&address, remaining(deadline)?)?,
        deadline,
    };
    attempt.register(&stream.socket)?;
    let (socket, _) = tungstenite::client::client_with_config(
        format!("ws://{address}/ws"),
        MaybeTlsStream::Plain(stream),
        Some(limits()),
    )
    .map_err(|_| anyhow::anyhow!("fixture upgrade failed"))?;
    Ok(WebSocketRecords { socket, attempt })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_endpoints_resolve_without_dns_and_respect_cancellation() {
        for (url, expected) in [
            ("wss://127.0.0.1:8443/ws", "127.0.0.1:8443"),
            ("wss://[::1]:8443/ws", "[::1]:8443"),
        ] {
            let endpoint = RelayEndpoint::parse(url).unwrap();
            let cancellation = Cancellation::default();
            let deadline = Instant::now() + std::time::Duration::from_secs(30);
            assert_eq!(
                endpoint.resolve(&cancellation, deadline).unwrap(),
                vec![expected.parse::<SocketAddr>().unwrap()]
            );
            cancellation.cancel();
            assert!(endpoint.resolve(&cancellation, deadline).is_err());
        }
    }

    struct CheckRoutingRequest;
    impl tungstenite::handshake::server::Callback for CheckRoutingRequest {
        fn on_request(
            self,
            request: &tungstenite::handshake::server::Request,
            response: tungstenite::handshake::server::Response,
        ) -> std::result::Result<
            tungstenite::handshake::server::Response,
            tungstenite::handshake::server::ErrorResponse,
        > {
            assert!(request.uri().path() == "/ws");
            assert!(
                request.uri().query()
                    == Some(
                        format!("v=1&room={}&role=client", URL_SAFE_NO_PAD.encode([7; 32]))
                            .as_str()
                    )
            );
            assert!(
                request.headers().get("authorization").unwrap().as_bytes()
                    == format!("Bearer {}", URL_SAFE_NO_PAD.encode([9; 32])).as_bytes()
            );
            Ok(response)
        }
    }
    #[test]
    fn explicit_broker_trust_preserves_hostname_verification() {
        use std::net::TcpListener;
        use std::time::Duration;

        for hostname in ["localhost", "wrong-host.test"] {
            let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            let root = URL_SAFE_NO_PAD.encode(certificate.cert.der());
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(
                    certificate.signing_key.serialize_der(),
                )
                .into(),
            )
            .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let accepted = hostname == "localhost";
            let broker = std::thread::spawn(move || {
                let (socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                let mut stream = rustls::StreamOwned::new(
                    rustls::ServerConnection::new(Arc::new(config)).unwrap(),
                    socket,
                );
                if !accepted {
                    match stream.read(&mut [0; 4096]) {
                        Err(_) | Ok(0) => {}
                        Ok(count) => {
                            panic!("hostname-mismatched broker received {count} application bytes")
                        }
                    }
                    return;
                }
                let mut websocket = tungstenite::accept_hdr(stream, CheckRoutingRequest).unwrap();
                let message = websocket.read().unwrap();
                assert!(matches!(message, Message::Binary(_)));
                websocket.send(message).unwrap();
            });
            let endpoint = EndpointConfig {
                url: format!("wss://{hostname}:{}/ws", address.port()),
                ca_certificates: vec![root],
            }
            .parse()
            .unwrap();
            let result = connect(
                &endpoint,
                address,
                Role::Client,
                &[7; 32],
                &[9; 32],
                Instant::now() + Duration::from_secs(30),
            );
            if accepted {
                let mut records = result.unwrap();
                let deadline = Instant::now() + Duration::from_secs(30);
                records.send(b"opaque-fixture-record", deadline).unwrap();
                assert_eq!(records.receive(deadline).unwrap(), b"opaque-fixture-record");
            } else {
                assert!(result.is_err(), "accepted a broker with the wrong hostname");
            }
            broker.join().unwrap();
        }
    }

    #[test]
    fn invalid_explicit_trust_is_rejected_before_network_access() {
        for certificates in [
            vec!["not-base64!".into()],
            vec![URL_SAFE_NO_PAD.encode(b"not a certificate")],
            vec!["A".repeat(10924)],
            vec![String::new(); 5],
        ] {
            assert!(EndpointConfig {
                url: "wss://localhost/ws".into(),
                ca_certificates: certificates
            }
            .parse()
            .is_err());
        }
        assert!(serde_json::from_str::<EndpointConfig>(
            r#"{"url":"wss://localhost/ws","insecure":true}"#
        )
        .is_err());
    }

    #[test]
    fn cancellation_interrupts_established_record_receive() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, released) = mpsc::channel();
        let broker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            let socket = tungstenite::accept(stream).unwrap();
            // Keep the peer open: only local cancellation can unblock the read.
            released.recv_timeout(Duration::from_secs(30)).unwrap();
            drop(socket);
        });
        let cancellation = Cancellation::default();
        let worker_token = cancellation.clone();
        let (ready, readiness) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut records = fixture_connect_cancellable(address, &worker_token).unwrap();
            ready.send(()).unwrap();
            let result = records.receive(Instant::now() + Duration::from_secs(120));
            done.send(result.is_err()).unwrap();
        });
        readiness.recv_timeout(Duration::from_secs(30)).unwrap();
        cancellation.cancel();
        assert!(completed
            .recv_timeout(Duration::from_secs(30))
            .expect("cancelled record read did not finish"));
        worker.join().unwrap();
        release.send(()).unwrap();
        broker.join().unwrap();
    }

    #[test]
    fn cancellation_interrupts_stalled_production_tls_handshake() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let cancellation = Cancellation::default();
        let worker_token = cancellation.clone();
        let (done, completed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let endpoint =
                RelayEndpoint::parse(&format!("wss://localhost:{}/ws", address.port())).unwrap();
            let result = connect_cancellable(
                &endpoint,
                address,
                Role::Host,
                &[1; 32],
                &[2; 32],
                Instant::now() + Duration::from_secs(120),
                &worker_token,
            );
            done.send(result.is_err()).unwrap();
        });
        let (mut broker, _) = listener.accept().unwrap();
        broker
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        // Reading a TLS byte proves registration and handshake I/O have begun.
        // The broker deliberately never responds and stays open until completion.
        broker.read_exact(&mut [0; 1]).unwrap();
        cancellation.cancel();
        // A generous hang guard, well below the explicit 120-second I/O deadline.
        assert!(completed
            .recv_timeout(Duration::from_secs(30))
            .expect("cancelled TLS worker did not finish"));
        worker.join().unwrap();
        drop(broker);
        assert!(cancellation.begin().is_err());
    }

    #[test]
    fn pre_cancelled_attempt_does_not_open_a_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint =
            RelayEndpoint::parse(&format!("wss://localhost:{}/ws", address.port())).unwrap();
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert!(connect_cancellable(
            &endpoint,
            address,
            Role::Host,
            &[1; 32],
            &[2; 32],
            Instant::now() + std::time::Duration::from_secs(30),
            &cancellation
        )
        .is_err());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn production_client_rejects_untrusted_broker_before_sending_credentials() {
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::time::Duration;

        let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let broker = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            // Hang guards only; certificate rejection, not timing, proves the boundary.
            socket
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
            let mut stream = rustls::StreamOwned::new(connection, socket);
            let mut request = [0_u8; 4096];
            match stream.read(&mut request) {
                Err(_) | Ok(0) => {}
                Ok(count) => panic!("untrusted broker received {count} application bytes"),
            }
        });
        let endpoint =
            RelayEndpoint::parse(&format!("wss://localhost:{}/ws", address.port())).unwrap();
        let result = connect(
            &endpoint,
            address,
            Role::Client,
            &[7; 32],
            &[9; 32],
            Instant::now() + Duration::from_secs(30),
        );
        broker.join().unwrap();
        assert!(result.is_err(), "accepted an untrusted broker certificate");
    }

    #[test]
    fn only_explicit_secure_endpoints_are_accepted() {
        for value in [
            "ws://example.test/ws",
            "https://example.test/ws",
            "wss://user:password@example.test/ws",
            "wss://example.test/ws?room=foo",
            "wss://example.test/ws#fragment",
            "wss://example.test/",
            "wss://example.test:0/ws",
        ] {
            assert!(RelayEndpoint::parse(value).is_err(), "accepted {value}");
        }
        assert!(RelayEndpoint::parse("wss://example.test/ws").is_ok());
        assert!(RelayEndpoint::parse("wss://192.168.1.20:8443/ws").is_ok());
    }
}
