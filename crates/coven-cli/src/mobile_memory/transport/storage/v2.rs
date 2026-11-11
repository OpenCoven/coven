//! Durable v2 routing material. The v1 file path is reused so an upgraded
//! pairing cannot silently fall back to a separately retained v1 credential.
use super::*;

pub mod device;
pub(crate) mod restore;
use crate::mobile_memory::transport::routing::{self, HostProof, HostState, Transport};

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Handover {
    pub version: u16,
    pub generation: u64,
    pub device_id: String,
    pub grant_id: String,
    pub revocation_epoch: u64,
    pub host_key: Vec<u8>,
    pub device_key: Vec<u8>,
    pub noise_public: [u8; 32],
    pub binding_signature: Vec<u8>,
    pub routing_seed: [u8; 32],
    pub relay_endpoints: Vec<EndpointConfig>,
}
impl Handover {
    pub fn acknowledgment(&self) -> Acknowledgment {
        Acknowledgment {
            version: 2,
            generation: self.generation,
            seed_digest: ring::digest::digest(&ring::digest::SHA256, &self.routing_seed)
                .as_ref()
                .try_into()
                .expect("SHA256 size"),
        }
    }
    pub fn verify(&self, host: &VerifyingKey, id: Uuid, device: &VerifyingKey) -> Result<Pairing> {
        ensure!(self.version == 2, "unsupported routing handover");
        Provisioning {
            version: 1,
            device_id: self.device_id.clone(),
            grant_id: self.grant_id.clone(),
            revocation_epoch: self.revocation_epoch,
            host_key: self.host_key.clone(),
            device_key: self.device_key.clone(),
            noise_public: self.noise_public,
            binding_signature: self.binding_signature.clone(),
            routing_seed: self.routing_seed,
            slot_seconds: SLOT_SECONDS,
            relay_endpoints: self.relay_endpoints.clone(),
        }
        .verify(host, id, device)
    }
}
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Acknowledgment {
    pub version: u16,
    pub generation: u64,
    pub seed_digest: [u8; 32],
}
#[derive(Clone)]
pub struct Snapshot {
    pub material: Material,
    pub generation: u64,
    pub floors: [u64; 2],
}
impl Snapshot {
    pub fn state(&self, transport: Transport) -> HostState {
        HostState {
            generation: self.generation,
            floor: self.floors[match transport {
                Transport::Direct => 0,
                Transport::Relay => 1,
            }],
        }
    }
    pub fn rendezvous(
        &self,
        transport: Transport,
        counter: u64,
    ) -> Result<([u8; 32], Zeroizing<[u8; 32]>)> {
        routing::derive(
            &self.material.routing_seed,
            transport,
            self.generation,
            counter,
        )
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Generation {
    generation: u64,
    floors: [u64; 2],
    material: StoredMaterial,
}
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Stored {
    version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    minimum_generation: Option<u64>,
    committed: Option<Generation>,
    pending: Option<Generation>,
}
#[derive(Deserialize)]
struct Version {
    version: u16,
}
fn next_generation(stored: &Stored) -> Result<u64> {
    stored
        .committed
        .as_ref()
        .map(|g| g.generation)
        .or(stored.minimum_generation)
        .map(|g| g.checked_add(1).context("routing re-pair required"))
        .transpose()
        .map(|g| g.unwrap_or(0))
}
fn index(transport: Transport) -> usize {
    match transport {
        Transport::Direct => 0,
        Transport::Relay => 1,
    }
}
fn is_direct(transport: Transport) -> bool {
    transport == Transport::Direct
}

fn check_installed_host(home: &Path, expected: &VerifyingKey) -> Result<()> {
    use p256::pkcs8::DecodePrivateKey;
    let identity = crate::mobile_memory::identity::load_existing_host_identity(home)?;
    let key = SigningKey::from_pkcs8_der(&identity.private_key_der)?;
    ensure!(
        key.verifying_key() == expected,
        "transport host identity changed"
    );
    Ok(())
}
fn snapshot(generation: &Generation, host: &VerifyingKey) -> Result<Snapshot> {
    let stored = &generation.material;
    ensure!(
        stored.version == 1 && stored.host_key == host.to_sec1_point(false).as_bytes(),
        "transport host identity changed"
    );
    validate_keypair(stored)?;
    Ok(Snapshot {
        material: Material {
            pairing: Pairing {
                device_id: Uuid::parse_str(&stored.device_id)?,
                grant_id: Uuid::parse_str(&stored.grant_id)?,
                revocation_epoch: stored.revocation_epoch,
                host_key: *host,
                device_key: VerifyingKey::from_sec1_bytes(&stored.device_key)?,
                noise_key: stored.noise_public,
            },
            noise_private: Zeroizing::new(stored.noise_private.clone()),
            routing_seed: Zeroizing::new(stored.routing_seed),
        },
        generation: generation.generation,
        floors: generation.floors,
    })
}
fn handover(generation: &Generation, host: &SigningKey) -> Result<Handover> {
    let provision = snapshot(generation, host.verifying_key())?
        .material
        .provisioning(host)?;
    Ok(Handover {
        version: 2,
        generation: generation.generation,
        device_id: provision.device_id.clone(),
        grant_id: provision.grant_id.clone(),
        revocation_epoch: provision.revocation_epoch,
        host_key: provision.host_key.clone(),
        device_key: provision.device_key.clone(),
        noise_public: provision.noise_public,
        binding_signature: provision.binding_signature.clone(),
        routing_seed: provision.routing_seed,
        relay_endpoints: Vec::new(),
    })
}
fn read(path: &Path, id: Uuid) -> Result<Option<Stored>> {
    if !exists(path)? {
        return Ok(None);
    }
    let encoded = read_private(path)?;
    let version: Version = serde_json::from_slice(&encoded)?;
    if version.version == 1 {
        let legacy: StoredMaterial = serde_json::from_slice(&encoded)?;
        ensure!(
            legacy.device_id == id.to_string(),
            "transport device mismatch"
        );
        validate_keypair(&legacy)?;
        return Ok(None);
    }
    let stored: Stored = serde_json::from_slice(&encoded)?;
    ensure!(
        stored.version == 2
            && (stored.committed.is_some()
                || stored.pending.is_some()
                || stored.minimum_generation.is_some()),
        "invalid v2 transport state"
    );
    if let (Some(committed), Some(minimum)) = (&stored.committed, stored.minimum_generation) {
        ensure!(
            committed.generation > minimum,
            "committed routing generation below retained minimum"
        );
    }
    for generation in [stored.committed.as_ref(), stored.pending.as_ref()]
        .into_iter()
        .flatten()
    {
        ensure!(
            generation.material.version == 1 && generation.material.device_id == id.to_string(),
            "invalid v2 transport identity"
        );
        validate_keypair(&generation.material)?;
    }
    if let Some(pending) = stored.pending.as_ref() {
        ensure!(
            pending.floors == [0, 0],
            "pending routing counters are not zero"
        );
        let expected = next_generation(&stored)?;
        ensure!(
            pending.generation == expected,
            "invalid pending routing generation"
        );
    }
    Ok(Some(stored))
}
const PROTOCOL_FLOOR_BYTES: &[u8] = b"coven.mobile.routing.v2\n";

fn require_v2(home: &Path, id: Uuid) -> Result<()> {
    let path = home
        .join("mobile")
        .join(format!(".transport-v2-{id}.required"));
    if exists(&path)? {
        ensure!(
            read_private(&path)?.as_slice() == PROTOCOL_FLOOR_BYTES,
            "invalid v2 protocol floor"
        );
    } else {
        crate::mobile_memory::config::atomic_create_private(&path, PROTOCOL_FLOOR_BYTES)?;
    }
    // Repeat both file and namespace barriers before publishing routing state,
    // even when retrying a marker whose first persistence result was uncertain.
    crate::durable_fs::sync_file(&fs::File::open(&path)?)?;
    #[cfg(test)]
    crate::mobile_memory::config::write_checkpoint(
        &path,
        crate::mobile_memory::config::MobileWritePhase::FileSynced,
    )?;
    crate::durable_fs::sync_parent(&path)?;
    #[cfg(test)]
    crate::mobile_memory::config::write_checkpoint(
        &path,
        crate::mobile_memory::config::MobileWritePhase::RetainedParentSynced,
    )?;
    crate::durable_fs::sync_parent(path.parent().context("missing mobile directory")?)?;
    #[cfg(test)]
    crate::mobile_memory::config::write_checkpoint(
        &path,
        crate::mobile_memory::config::MobileWritePhase::RetainedHomeSynced,
    )?;
    Ok(())
}
fn save(path: &Path, stored: &impl Serialize) -> Result<()> {
    let encoded = Zeroizing::new(serde_json::to_vec(stored)?);
    ensure!(
        encoded.len() <= MAX_STATE_BYTES as usize,
        "transport state exceeds limit"
    );
    atomic_replace_private(path, &encoded)?;
    // The legacy helper has no checked Windows directory barrier. Repeat with
    // the shared retained-directory implementation (NTFS required on Windows).
    crate::durable_fs::sync_parent(path)?;
    #[cfg(test)]
    crate::mobile_memory::config::write_checkpoint(
        path,
        crate::mobile_memory::config::MobileWritePhase::RetainedParentSynced,
    )?;
    crate::durable_fs::sync_parent(path.parent().context("missing transport directory")?)?;
    #[cfg(test)]
    crate::mobile_memory::config::write_checkpoint(
        path,
        crate::mobile_memory::config::MobileWritePhase::RetainedHomeSynced,
    )?;
    Ok(())
}
fn locked<T>(
    registry: &DeviceRegistry,
    home: &Path,
    id: Uuid,
    deadline: Option<std::time::Instant>,
    operation: impl FnOnce(&Path) -> Result<T>,
) -> Result<T> {
    locked_checked(home, id, deadline, || registry.ensure_running(), operation)
}
fn locked_checked<T>(
    home: &Path,
    id: Uuid,
    deadline: Option<std::time::Instant>,
    running: impl Fn() -> Result<()>,
    operation: impl FnOnce(&Path) -> Result<T>,
) -> Result<T> {
    let check = || -> Result<()> {
        crate::mobile_memory::store_lock::check()?;
        running()?;
        ensure!(
            deadline.is_none_or(|end| std::time::Instant::now() < end),
            "possession deadline expired"
        );
        Ok(())
    };
    let _process = loop {
        check()?;
        match MATERIAL_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                anyhow::bail!("transport state lock poisoned")
            }
        }
    };
    let directory = ensure_private_mobile_dir(home)?;
    let lock = crate::state_lock::open_lock_file(&directory.join(".transport.lock"))?;
    loop {
        check()?;
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(error) if crate::state_lock::is_lock_contended(&error) => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(error) => return Err(error.into()),
        }
    }
    check()?;
    let result = operation(&directory.join(format!("transport-{id}.json")));
    let released = FileExt::unlock(&lock);
    match result {
        Err(error) => Err(error),
        Ok(value) => {
            released?;
            Ok(value)
        }
    }
}

fn new_generation(
    registry: &DeviceRegistry,
    host: &SigningKey,
    id: Uuid,
    generation: u64,
) -> Result<Generation> {
    registry.reload()?;
    let record = registry
        .authorization_record(id)?
        .context("unknown transport device")?;
    let (private, public) = generate_noise_key()?;
    let value = Generation {
        generation,
        floors: [0, 0],
        material: StoredMaterial {
            version: 1,
            device_id: id.to_string(),
            grant_id: record.grant.id.to_string(),
            revocation_epoch: record.grant.revocation_epoch,
            host_key: host
                .verifying_key()
                .to_sec1_point(false)
                .as_bytes()
                .to_vec(),
            device_key: URL_SAFE_NO_PAD.decode(&record.device.public_key_x963)?,
            noise_private: private.to_vec(),
            noise_public: public,
            routing_seed: nonce()?,
        },
    };
    authority::check_current(
        registry,
        &snapshot(&value, host.verifying_key())?.material.pairing,
        true,
        Utc::now(),
    )?;
    Ok(value)
}
fn active(stored: &Stored) -> Result<&Generation> {
    ensure!(stored.pending.is_none(), "routing handover pending");
    stored
        .committed
        .as_ref()
        .context("routing handover not committed")
}
fn matches_ack(generation: &Generation, ack: &Acknowledgment) -> bool {
    ack.version == 2
        && ack.generation == generation.generation
        && ack.seed_digest.as_slice()
            == ring::digest::digest(&ring::digest::SHA256, &generation.material.routing_seed)
                .as_ref()
}

fn prepare_current_pending(
    stored: &mut Stored,
    registry: &DeviceRegistry,
    host: &SigningKey,
    id: Uuid,
) -> Result<()> {
    if let Some(pending) = stored.pending.as_ref() {
        // Distinguish a genuine authority change from I/O failure, expiry or
        // suspension. Transient errors must not replace a previously offered
        // generation/seed. A current re-grant or key/epoch change burns it.
        registry.reload()?;
        let current = registry
            .authorization_record(id)?
            .context("unknown transport device")?;
        let device_key = URL_SAFE_NO_PAD.decode(&current.device.public_key_x963)?;
        let unchanged = pending.material.grant_id == current.grant.id.to_string()
            && pending.material.revocation_epoch == current.grant.revocation_epoch
            && pending.material.host_key == host.verifying_key().to_sec1_point(false).as_bytes()
            && pending.material.device_key == device_key;
        if unchanged {
            return authority::check_current(
                registry,
                &snapshot(pending, host.verifying_key())?.material.pairing,
                true,
                Utc::now(),
            );
        }
        let burned = pending.generation;
        let generation = burned.checked_add(1).context("routing re-pair required")?;
        let next = new_generation(registry, host, id, generation)?;
        stored.minimum_generation = Some(burned);
        stored.committed = None;
        stored.pending = Some(next);
    } else {
        let generation = next_generation(stored)?;
        stored.pending = Some(new_generation(registry, host, id, generation)?);
    }
    Ok(())
}

/// Caller must authenticate the signed request over a pinned direct ingress.
/// A pending value is persisted again before retry response so an uncertain
/// prior namespace barrier cannot be mistaken for a successful durable write.
pub fn prepare_direct(
    home: &Path,
    registry: &DeviceRegistry,
    host: &SigningKey,
    id: Uuid,
) -> Result<Handover> {
    locked(registry, home, id, None, |path| {
        restore::check_installation(home)?;
        check_installed_host(home, host.verifying_key())?;
        let existing = read(path, id)?;
        ensure!(
            existing.is_some() || !requires_v2(home, id)?,
            "missing v2 state requires authenticated recovery"
        );
        let mut stored = existing.unwrap_or(Stored {
            version: 2,
            minimum_generation: None,
            committed: None,
            pending: None,
        });
        prepare_current_pending(&mut stored, registry, host, id)?;
        let pending = stored
            .pending
            .as_ref()
            .context("missing pending handover")?;
        let material = snapshot(pending, host.verifying_key())?;
        authority::check_current(registry, &material.material.pairing, true, Utc::now())?;
        restore::check_handover(
            home,
            id,
            pending.generation,
            pending.material.revocation_epoch,
        )?;
        require_v2(home, id)?;
        save(path, &stored)?;
        handover(pending, host)
    })
}
/// Acknowledgment is accepted only after the device durably adopts the exact
/// pending generation/seed. Replay after successful commit is idempotent.
pub fn acknowledge_direct(
    home: &Path,
    registry: &DeviceRegistry,
    host: &SigningKey,
    id: Uuid,
    ack: &Acknowledgment,
) -> Result<()> {
    locked(registry, home, id, None, |path| {
        restore::check_installation(home)?;
        check_installed_host(home, host.verifying_key())?;
        let mut stored = read(path, id)?.context("routing material unavailable")?;
        let generation = stored
            .pending
            .as_ref()
            .or(stored.committed.as_ref())
            .context("routing material unavailable")?;
        ensure!(
            matches_ack(generation, ack),
            "routing handover acknowledgment mismatch"
        );
        authority::check_current(
            registry,
            &snapshot(generation, host.verifying_key())?.material.pairing,
            true,
            Utc::now(),
        )?;
        let target_generation = generation.generation;
        let target_epoch = generation.material.revocation_epoch;
        restore::check_handover(home, id, target_generation, target_epoch)?;
        if stored.pending.is_some() {
            stored.committed = stored.pending.take();
        }
        save(path, &stored)?;
        restore::confirm_handover(home, id, target_generation, target_epoch)
    })
}
pub fn load(
    home: &Path,
    registry: &DeviceRegistry,
    host: &VerifyingKey,
    id: Uuid,
    transport: Transport,
) -> Result<Option<Snapshot>> {
    restore::check_application(home, registry, id)?;
    check_installed_host(home, host)?;
    let path = home.join("mobile").join(format!("transport-{id}.json"));
    let Some(stored) = read(&path, id)? else {
        ensure!(
            !requires_v2(home, id)?,
            "missing v2 state requires authenticated recovery"
        );
        return Ok(None);
    };
    if stored.pending.is_some() {
        return Ok(None);
    };
    let value = snapshot(active(&stored)?, host)?;
    authority::check_current(
        registry,
        &value.material.pairing,
        is_direct(transport),
        Utc::now(),
    )?;
    Ok(Some(value))
}
fn pairing_matches(actual: &Pairing, expected: &Pairing) -> bool {
    actual.device_id == expected.device_id
        && actual.grant_id == expected.grant_id
        && actual.revocation_epoch == expected.revocation_epoch
        && actual.host_key == expected.host_key
        && actual.device_key == expected.device_key
        && actual.noise_key == expected.noise_key
}
pub fn refresh_proof(
    home: &Path,
    registry: &DeviceRegistry,
    pairing: &Pairing,
    transport: Transport,
    hint: HostProof,
    deadline: std::time::Instant,
) -> Result<HostProof> {
    locked(registry, home, pairing.device_id, Some(deadline), |path| {
        restore::check_application(home, registry, pairing.device_id)?;
        check_installed_host(home, &pairing.host_key)?;
        let stored = read(path, pairing.device_id)?.context("routing material unavailable")?;
        let current = snapshot(active(&stored)?, &pairing.host_key)?;
        ensure!(
            pairing_matches(&current.material.pairing, pairing),
            "routing pairing changed"
        );
        authority::check_current(registry, pairing, is_direct(transport), Utc::now())?;
        ensure!(
            current.generation == hint.generation,
            "routing generation changed"
        );
        ensure!(
            current.state(transport).floor >= hint.host_floor,
            "routing floor regressed"
        );
        current.state(transport).proof(hint.counter)
    })
}
/// An established channel can retain a consumed room, but not stale generation,
/// key, grant or protocol authority. No caller-supplied routing credentials grant
/// permission to use the legacy profile once the v2 floor is established.
pub fn check_channel(
    home: &Path,
    registry: &DeviceRegistry,
    pairing: &Pairing,
    transport: Transport,
    generation: Option<u64>,
) -> Result<()> {
    restore::check_application(home, registry, pairing.device_id)?;
    registry.ensure_running()?;
    check_installed_host(home, &pairing.host_key)?;
    authority::check_current(registry, pairing, is_direct(transport), Utc::now())?;
    let stored = read(
        &home
            .join("mobile")
            .join(format!("transport-{}.json", pairing.device_id)),
        pairing.device_id,
    )?;
    if let Some(generation) = generation {
        let stored = stored.context("v2 routing state unavailable")?;
        let current = snapshot(active(&stored)?, &pairing.host_key)?;
        ensure!(
            current.generation == generation && pairing_matches(&current.material.pairing, pairing),
            "routing channel generation changed"
        );
    } else {
        ensure!(
            stored.is_none() && !requires_v2(home, pairing.device_id)?,
            "v2 routing required; legacy channel refused"
        );
    }
    Ok(())
}
/// Local operator commands must be able to fence a corrupt registry without
/// constructing or manufacturing a replacement registry merely to acquire locks.
pub(crate) fn with_operator_lock<T>(
    home: &Path,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    locked_checked(home, Uuid::nil(), None, || Ok(()), |_| operation())
}
pub(crate) fn with_installation_lock<T>(
    home: &Path,
    registry: &DeviceRegistry,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    locked(registry, home, Uuid::nil(), None, |_| operation())
}
/// Serialize application admission and effects with generation handovers.
/// Bootstrap owns this lock internally and must not call through this wrapper.
pub fn with_channel_current<T>(
    home: &Path,
    registry: &DeviceRegistry,
    pairing: &Pairing,
    transport: Transport,
    generation: Option<u64>,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    locked(registry, home, pairing.device_id, None, |_| {
        check_channel(home, registry, pairing, transport, generation)?;
        operation()
    })
}
/// Production consumption accepts only a proof verified by the v2 channel,
/// bound to its actual pairing and original absolute challenge deadline.
pub fn consume_verified(
    home: &Path,
    registry: &DeviceRegistry,
    verified: routing::channel::VerifiedDeviceProof,
) -> Result<()> {
    consume_inner(
        home,
        registry,
        verified.pairing(),
        verified.transport(),
        verified.route(),
        verified.deadline(),
    )
}
#[cfg(test)]
fn consume(
    home: &Path,
    registry: &DeviceRegistry,
    pairing: &Pairing,
    transport: Transport,
    proof: HostProof,
) -> Result<()> {
    consume_inner(
        home,
        registry,
        pairing,
        transport,
        proof,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
}
fn consume_inner(
    home: &Path,
    registry: &DeviceRegistry,
    pairing: &Pairing,
    transport: Transport,
    proof: HostProof,
    deadline: std::time::Instant,
) -> Result<()> {
    locked(registry, home, pairing.device_id, Some(deadline), |path| {
        restore::check_application(home, registry, pairing.device_id)?;
        check_installed_host(home, &pairing.host_key)?;
        let mut stored = read(path, pairing.device_id)?.context("routing material unavailable")?;
        let current = snapshot(active(&stored)?, &pairing.host_key)?;
        ensure!(
            current.material.pairing.grant_id == pairing.grant_id
                && current.material.pairing.revocation_epoch == pairing.revocation_epoch
                && current.material.pairing.device_key == pairing.device_key
                && current.material.pairing.noise_key == pairing.noise_key,
            "routing pairing changed"
        );
        authority::check_current(
            registry,
            &current.material.pairing,
            is_direct(transport),
            Utc::now(),
        )?;
        ensure!(
            std::time::Instant::now() < deadline,
            "possession deadline expired"
        );
        let next = current.state(transport).consume(proof)?;
        stored
            .committed
            .as_mut()
            .context("routing material unavailable")?
            .floors[index(transport)] = next.floor;
        save(path, &stored)
    })
}
/// Direct ingress, current identity and signed request authentication are
/// required at the gateway. This lower persistence boundary never lowers c.
pub fn resync_direct(
    home: &Path,
    registry: &DeviceRegistry,
    host: &SigningKey,
    id: Uuid,
    transport: Transport,
    generation: u64,
    device_next: u64,
) -> Result<HostState> {
    locked(registry, home, id, None, |path| {
        restore::check_application(home, registry, id)?;
        check_installed_host(home, host.verifying_key())?;
        let mut stored = read(path, id)?.context("routing material unavailable")?;
        let current = snapshot(active(&stored)?, host.verifying_key())?;
        authority::check_current(registry, &current.material.pairing, true, Utc::now())?;
        ensure!(
            generation == current.generation,
            "routing generation changed"
        );
        let state = HostState {
            generation,
            floor: current.state(transport).floor.max(device_next),
        };
        stored
            .committed
            .as_mut()
            .context("routing material unavailable")?
            .floors[index(transport)] = state.floor;
        save(path, &stored)?;
        Ok(state)
    })
}

/// Explicit state-loss recovery over authenticated direct ingress.
pub fn recover_direct(
    home: &Path,
    registry: &DeviceRegistry,
    host: &SigningKey,
    id: Uuid,
    known_generation: Option<u64>,
) -> Result<Handover> {
    locked(registry, home, id, None, |path| {
        restore::check_installation(home)?;
        check_installed_host(home, host.verifying_key())?;
        let existing = match read(path, id) {
            Ok(value) => value,
            Err(_) => {
                // Explicit recovery may replace malformed private state, never
                // an unsafe object, oversized input or a future protocol.
                let encoded = read_private(path)?;
                if let Ok(header) = serde_json::from_slice::<Version>(&encoded) {
                    ensure!(
                        header.version == 1 || header.version == 2,
                        "unsupported routing state version"
                    );
                }
                None
            }
        };
        let mut stored = if let Some(stored) = existing {
            let floor = stored
                .committed
                .as_ref()
                .map(|g| g.generation)
                .or(stored.minimum_generation);
            if let Some(known) = known_generation {
                ensure!(
                    Some(known) == floor
                        || stored
                            .pending
                            .as_ref()
                            .is_some_and(|g| g.generation == known),
                    "routing generation disagreement requires restore ceremony"
                );
            }
            stored
        } else {
            let minimum_generation = known_generation.context(
                "routing generation unavailable; authenticated anchor or restore ceremony required",
            )?;
            Stored {
                version: 2,
                minimum_generation: Some(minimum_generation),
                committed: None,
                pending: None,
            }
        };
        prepare_current_pending(&mut stored, registry, host, id)?;
        let pending = stored
            .pending
            .as_ref()
            .context("missing pending handover")?;
        authority::check_current(
            registry,
            &snapshot(pending, host.verifying_key())?.material.pairing,
            true,
            Utc::now(),
        )?;
        restore::check_handover(
            home,
            id,
            pending.generation,
            pending.material.revocation_epoch,
        )?;
        require_v2(home, id)?;
        save(path, &stored)?;
        handover(pending, host)
    })
}

#[cfg(test)]
mod tests;
