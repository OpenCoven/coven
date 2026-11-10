//! Direct WSS candidates on the existing operator-enabled private TLS listener.
//! Routing chooses a pairing; only fresh endpoint proofs admit its channel.
use super::super::transport::{
    self, cancellation::Attempt, connection::Transport, storage, websocket,
};
use super::*;
use p256::{elliptic_curve::subtle::ConstantTimeEq, pkcs8::DecodePrivateKey};
use std::time::Instant;
use transport::{routing, storage::v2};

enum Selected {
    Legacy(storage::Material),
    Counter(v2::Snapshot, u64),
}

pub(super) fn claims(request: &MobileHttpRequest) -> bool {
    request.target.split('?').next() == Some("/ws")
}

pub(super) fn serve(
    tls: StreamOwned<ServerConnection, TcpStream>,
    state: &MobileGatewayState,
    request: MobileHttpRequest,
    attempt: Attempt,
) -> Result<()> {
    anyhow::ensure!(
        request.method == "GET" && request.body.is_empty(),
        "invalid direct upgrade"
    );
    let mut observation = state.connections.begin(Transport::Direct)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let target = url::Url::parse(&format!("https://direct.invalid{}", request.target))?;
    anyhow::ensure!(
        target.path() == "/ws" && target.fragment().is_none(),
        "invalid direct routing"
    );
    let parameters: Vec<_> = target.query_pairs().collect();
    anyhow::ensure!(parameters.len() == 3, "invalid direct routing");
    let mut version = None;
    let mut role = None;
    let mut room = None;
    for (name, value) in parameters {
        match name.as_ref() {
            "v" if version.is_none() => version = Some(value.into_owned()),
            "role" if role.is_none() => role = Some(value.into_owned()),
            "room" if room.is_none() => room = Some(value.into_owned()),
            _ => bail!("invalid direct routing"),
        }
    }
    anyhow::ensure!(
        version.as_deref() == Some("1") && role.as_deref() == Some("client"),
        "invalid direct routing"
    );
    let decode = |value: &str| -> Result<[u8; 32]> {
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| anyhow::anyhow!("invalid direct routing"))?;
        anyhow::ensure!(
            URL_SAFE_NO_PAD.encode(&bytes) == value,
            "invalid direct routing"
        );
        bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid direct routing"))
    };
    let room = decode(room.as_deref().context("invalid direct routing")?)?;
    let credential = request
        .headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .context("invalid direct routing")?;
    let credential = zeroize::Zeroizing::new(decode(credential)?);
    let identity = super::super::identity::load_existing_host_identity(&state.coven_home)?;
    anyhow::ensure!(
        identity.public_key_fingerprint == state.host_fingerprint,
        "host identity changed"
    );
    let signer = p256::ecdsa::SigningKey::from_pkcs8_der(&identity.private_key_der)?;
    state.registry.reload()?;
    let now = Utc::now();
    let mut selected = None;
    // The registry enforces its 128-device bound. Inspect every admitted
    // record so an earlier unprovisioned device cannot hide a later pairing.
    for device in state.registry.list_status()?.into_iter().filter(|device| {
        device.revoked_at.is_none()
            && device.suspended_at.is_none()
            && device.expires_at.is_none_or(|expiry| now < expiry)
    }) {
        anyhow::ensure!(Instant::now() < deadline, "direct routing deadline expired");
        match v2::load(
            &state.coven_home,
            &state.registry,
            signer.verifying_key(),
            device.id,
            routing::Transport::Direct,
        ) {
            Ok(Some(snapshot)) => {
                for offset in 0..routing::WINDOW {
                    let Some(counter) = snapshot
                        .state(routing::Transport::Direct)
                        .floor
                        .checked_add(offset)
                    else {
                        continue;
                    };
                    if snapshot
                        .state(routing::Transport::Direct)
                        .proof(counter)
                        .is_err()
                    {
                        continue;
                    }
                    let (expected_room, expected_credential) =
                        snapshot.rendezvous(routing::Transport::Direct, counter)?;
                    if expected_room == room
                        && bool::from(expected_credential.as_slice().ct_eq(credential.as_slice()))
                    {
                        anyhow::ensure!(selected.is_none(), "ambiguous direct routing");
                        selected = Some(Selected::Counter(snapshot.clone(), counter));
                    }
                }
            }
            Ok(None) => {
                let material = match storage::load_existing(
                    &state.coven_home,
                    &state.registry,
                    signer.verifying_key(),
                    device.id,
                ) {
                    Ok(Some(material)) => material,
                    _ => continue,
                };
                let slot = u64::try_from(now.timestamp())? / storage::SLOT_SECONDS;
                let (expected_room, expected_credential) = material.rendezvous(slot, true)?;
                if expected_room == room
                    && bool::from(expected_credential.as_slice().ct_eq(credential.as_slice()))
                {
                    anyhow::ensure!(selected.is_none(), "ambiguous direct routing");
                    v2::check_channel(
                        &state.coven_home,
                        &state.registry,
                        &material.pairing,
                        routing::Transport::Direct,
                        None,
                    )?;
                    selected = Some(Selected::Legacy(material));
                }
            }
            // Missing, malformed or fenced v2 state never falls back to v1.
            Err(_) => continue,
        }
    }
    let selected = selected.context("direct pairing unavailable")?;
    let mut upgrade = tungstenite::http::Request::builder()
        .method("GET")
        .uri(&request.target);
    for (name, value) in &request.headers {
        upgrade = upgrade.header(name, value);
    }
    let upgrade = upgrade
        .body(())
        .map_err(|_| anyhow::anyhow!("invalid direct upgrade"))?;
    let records = websocket::accept_tls(tls, upgrade, deadline, attempt)?;
    observation.authenticating()?;
    let mut channel = match selected {
        Selected::Legacy(material) => transport::accept(
            records,
            &material.pairing,
            &material.noise_private,
            &signer,
            &room,
            true,
            |pairing, _| {
                v2::check_channel(
                    &state.coven_home,
                    &state.registry,
                    pairing,
                    routing::Transport::Direct,
                    None,
                )
            },
        )?,
        Selected::Counter(snapshot, counter) => routing::channel::accept(
            records,
            routing::channel::HostAdmission {
                pairing: &snapshot.material.pairing,
                private: &snapshot.material.noise_private,
                host: &signer,
                context: &room,
                direct: true,
                proof: snapshot.state(routing::Transport::Direct).proof(counter)?,
            },
            |pairing, _| {
                v2::check_channel(
                    &state.coven_home,
                    &state.registry,
                    pairing,
                    routing::Transport::Direct,
                    Some(snapshot.generation),
                )
            },
            |proof, deadline| {
                v2::refresh_proof(
                    &state.coven_home,
                    &state.registry,
                    &snapshot.material.pairing,
                    routing::Transport::Direct,
                    proof,
                    deadline,
                )
            },
            |proof| v2::consume_verified(&state.coven_home, &state.registry, proof),
        )?,
    };
    observation.online()?;
    while channel.is_open() {
        serve_transport_with_state(state, &mut channel)?;
    }
    Ok(())
}
