//! V2 possession protocol. Persistence callbacks are trusted Rust state owners;
//! they must serialize attempts and perform durable, current-state validation.
use super::super::{Channel, Pairing, Records};
use super::{DeviceState, HostProof, Transport};
use anyhow::Result;
use p256::ecdsa::SigningKey;

pub struct DeviceAdmission<'a> {
    pub pairing: &'a Pairing,
    pub binding: &'a [u8],
    pub device: &'a SigningKey,
    pub context: &'a [u8; 32],
    pub direct: bool,
    pub state: DeviceState,
}
pub struct HostAdmission<'a> {
    pub pairing: &'a Pairing,
    pub private: &'a [u8],
    pub host: &'a SigningKey,
    pub context: &'a [u8; 32],
    pub direct: bool,
    pub proof: HostProof,
}

/// Created only after this channel verifies the device's possession signature.
/// The original absolute challenge deadline survives storage-lock waits.
pub struct VerifiedDeviceProof {
    transport: Transport,
    pairing: Pairing,
    proof: HostProof,
    deadline: std::time::Instant,
}
impl VerifiedDeviceProof {
    pub(crate) fn transport(&self) -> Transport {
        self.transport
    }
    pub(crate) fn pairing(&self) -> &Pairing {
        &self.pairing
    }
    pub(crate) fn route(&self) -> HostProof {
        self.proof
    }
    pub(crate) fn deadline(&self) -> std::time::Instant {
        self.deadline
    }
}
impl std::ops::Deref for VerifiedDeviceProof {
    type Target = HostProof;
    fn deref(&self) -> &HostProof {
        &self.proof
    }
}

pub fn connect<R: Records>(
    mut records: R,
    admission: DeviceAdmission<'_>,
    persist: impl FnOnce(DeviceState) -> Result<()>,
) -> Result<Channel<R>> {
    use super::super::*;
    let DeviceAdmission {
        pairing,
        binding,
        device,
        context,
        direct,
        state,
    } = admission;
    state.room()?;
    pairing.verify_noise_binding(binding)?;
    ensure!(
        device.verifying_key() == &pairing.device_key,
        "device identity mismatch"
    );
    let deadline = Instant::now() + HANDSHAKE_BUDGET;
    let prologue = prologue(context, direct)?;
    let mut handshake = Builder::new(PROFILE.parse()?)
        .prologue(&prologue)?
        .remote_public_key(&pairing.noise_key)?
        .build_initiator()?;
    handshake_send(&mut records, &mut handshake, deadline)?;
    handshake_receive(&mut records, &mut handshake, deadline)?;
    let hash = handshake.get_handshake_hash().to_vec();
    let mut channel = Channel {
        records,
        cipher: Some(handshake.into_transport_mode()?),
        pairing: pairing.clone(),
        routing_generation: Some(state.generation),
        direct,
    };
    let host_nonce = channel.receive(deadline)?;
    ensure!(host_nonce.len() == 32, "invalid host challenge");
    let device_nonce = nonce()?;
    channel.send(&device_nonce, deadline)?;
    let packet = channel.receive(deadline)?;
    ensure!(packet.len() == 24 + 64, "invalid v2 possession proof");
    let proof = HostProof {
        generation: u64::from_be_bytes(packet[..8].try_into()?),
        host_floor: u64::from_be_bytes(packet[8..16].try_into()?),
        counter: u64::from_be_bytes(packet[16..24].try_into()?),
    };
    verify(
        &pairing.host_key,
        &pairing.proof_v2("host", &hash, &host_nonce, &device_nonce, proof)?,
        &packet[24..],
    )?;
    // Authentication precedes arithmetic and persistence. A failed or uncertain
    // durable write aborts without exposing a device proof to the host.
    let next = state.consume(proof)?;
    ensure!(Instant::now() < deadline, "possession deadline expired");
    persist(next)?;
    channel.send(
        &sign(
            device,
            &pairing.proof_v2("device", &hash, &host_nonce, &device_nonce, proof)?,
        ),
        deadline,
    )?;
    ensure!(
        channel.receive(deadline)?.as_slice() == b"host-finished-v2",
        "invalid completion"
    );
    channel.send(b"device-finished-v2", deadline)?;
    Ok(channel)
}

/// `commit` must acquire the pairing's consumption lock, reload generation/floor
/// and registry/grant, and persist the new floor before returning success. The
/// outer policy callback alone cannot replace that locked current-state check.
pub fn accept<R: Records>(
    mut records: R,
    admission: HostAdmission<'_>,
    mut policy: impl FnMut(&Pairing, bool) -> Result<()>,
    refresh: impl FnOnce(HostProof, std::time::Instant) -> Result<HostProof>,
    commit: impl FnOnce(VerifiedDeviceProof) -> Result<()>,
) -> Result<Channel<R>> {
    use super::super::*;
    let HostAdmission {
        pairing,
        private,
        host,
        context,
        direct,
        proof,
    } = admission;
    super::HostState {
        generation: proof.generation,
        floor: proof.host_floor,
    }
    .proof(proof.counter)?;
    policy(pairing, direct)?;
    ensure!(
        host.verifying_key() == &pairing.host_key,
        "host identity mismatch"
    );
    let deadline = Instant::now() + HANDSHAKE_BUDGET;
    let prologue = prologue(context, direct)?;
    let mut handshake = Builder::new(PROFILE.parse()?)
        .prologue(&prologue)?
        .local_private_key(private)?
        .build_responder()?;
    handshake_receive(&mut records, &mut handshake, deadline)?;
    handshake_send(&mut records, &mut handshake, deadline)?;
    let hash = handshake.get_handshake_hash().to_vec();
    let mut channel = Channel {
        records,
        cipher: Some(handshake.into_transport_mode()?),
        pairing: pairing.clone(),
        routing_generation: Some(proof.generation),
        direct,
    };
    let host_nonce = nonce()?;
    channel.send(&host_nonce, deadline)?;
    let device_nonce = channel.receive(deadline)?;
    ensure!(device_nonce.len() == 32, "invalid device challenge");
    let current = refresh(proof, deadline)?;
    ensure!(
        current.generation == proof.generation && current.counter == proof.counter,
        "routing challenge changed"
    );
    super::HostState {
        generation: current.generation,
        floor: current.host_floor,
    }
    .proof(current.counter)?;
    let proof = current;
    let signature = sign(
        host,
        &pairing.proof_v2("host", &hash, &host_nonce, &device_nonce, proof)?,
    );
    let mut packet = Vec::with_capacity(24 + signature.len());
    packet.extend_from_slice(&proof.generation.to_be_bytes());
    packet.extend_from_slice(&proof.host_floor.to_be_bytes());
    packet.extend_from_slice(&proof.counter.to_be_bytes());
    packet.extend_from_slice(&signature);
    channel.send(&packet, deadline)?;
    let signature = channel.receive(deadline)?;
    verify(
        &pairing.device_key,
        &pairing.proof_v2("device", &hash, &host_nonce, &device_nonce, proof)?,
        &signature,
    )?;
    policy(pairing, direct)?;
    commit(VerifiedDeviceProof {
        transport: if direct {
            Transport::Direct
        } else {
            Transport::Relay
        },
        pairing: pairing.clone(),
        proof,
        deadline,
    })?;
    channel.send(b"host-finished-v2", deadline)?;
    ensure!(
        channel.receive(deadline)?.as_slice() == b"device-finished-v2",
        "invalid completion"
    );
    policy(pairing, direct)?;
    Ok(channel)
}
