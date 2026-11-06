//! Device-side durable adoption and consuming-attempt serialization.
use super::*;
use crate::mobile_memory::transport::{
    routing::{channel, DeviceState},
    Channel, Records,
};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredDevice {
    version: u16,
    handover: Handover,
    states: [DeviceState; 2],
}
impl StoredDevice {
    fn verify(&self, host: &VerifyingKey, id: Uuid, device: &VerifyingKey) -> Result<Pairing> {
        ensure!(self.version == 2, "unsupported device routing state");
        for state in self.states {
            ensure!(
                state.generation == self.handover.generation
                    && state.next >= state.floor
                    && state.next - state.floor <= routing::WINDOW,
                "invalid device routing counters"
            );
        }
        self.handover.verify(host, id, device)
    }
}
fn path(home: &Path, id: Uuid) -> PathBuf {
    home.join("mobile")
        .join(format!("device-transport-{id}.json"))
}
fn read(path: &Path) -> Result<StoredDevice> {
    Ok(serde_json::from_slice(&read_private(path)?)?)
}
struct DeviceLock(fs::File);
impl Drop for DeviceLock {
    fn drop(&mut self) {
        // A concurrent fork can temporarily duplicate this open description
        // before exec closes it. Closing only our descriptor would leave that
        // unrelated child holding the ended attempt's lock. Release explicitly;
        // a failed unlock still falls back to close and remains fail-closed.
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
fn try_lock(home: &Path) -> Result<DeviceLock> {
    let directory = ensure_private_mobile_dir(home)?;
    let lock = crate::state_lock::open_lock_file(&directory.join(".transport.lock"))?;
    lock.try_lock_exclusive()
        .context("another device routing attempt is in flight")?;
    Ok(DeviceLock(lock))
}
fn same_handover(left: &Handover, right: &Handover) -> bool {
    left.version == right.version
        && left.generation == right.generation
        && left.device_id == right.device_id
        && left.grant_id == right.grant_id
        && left.revocation_epoch == right.revocation_epoch
        && left.host_key == right.host_key
        && left.device_key == right.device_key
        && left.noise_public == right.noise_public
        && left.routing_seed == right.routing_seed
}

/// Holds the OS transport lock from before room join through proof/completion.
/// One attempt may be used only once, including after uncertain persistence.
pub struct Attempt {
    _lock: DeviceLock,
    path: PathBuf,
    record: StoredDevice,
    pairing: Pairing,
    transport: Transport,
    usable: bool,
}
impl Attempt {
    pub fn state(&self) -> DeviceState {
        self.record.states[index(self.transport)]
    }
    pub fn rendezvous(&self) -> Result<([u8; 32], Zeroizing<[u8; 32]>)> {
        ensure!(self.usable, "device routing attempt already used");
        routing::derive(
            &self.record.handover.routing_seed,
            self.transport,
            self.record.handover.generation,
            self.state().room()?,
        )
    }
    fn persist_verified(&mut self, state: DeviceState) -> Result<()> {
        let previous = self.state();
        ensure!(
            state.generation == previous.generation
                && previous.next.checked_add(1) == Some(state.next)
                && state.floor >= previous.floor
                && state.floor <= previous.next,
            "invalid authenticated device transition"
        );
        self.record.states[index(self.transport)] = state;
        let result = save(&self.path, &self.record);
        if result.is_err() {
            self.usable = false;
        }
        result
    }
    pub fn connect<R: Records>(&mut self, records: R, device: &SigningKey) -> Result<Channel<R>> {
        let (room, _) = self.rendezvous()?;
        self.usable = false;
        let pairing = self.pairing.clone();
        let binding = self.record.handover.binding_signature.clone();
        let admission = channel::DeviceAdmission {
            pairing: &pairing,
            binding: &binding,
            device,
            context: &room,
            direct: is_direct(self.transport),
            state: self.state(),
        };
        channel::connect(records, admission, |next| self.persist_verified(next))
    }
}

/// Call only with a handover received on the already pinned authenticated direct
/// channel. The Noise-key certificate verifies identity; it does not itself sign
/// the seed/generation, whose authenticity comes from that direct channel.
/// A higher generation may skip values burned by a grant/key change before the
/// device adopted them. Equal generations still require the identical value.
pub fn adopt_direct(
    home: &Path,
    handover: Handover,
    host: &VerifyingKey,
    id: Uuid,
    device: &VerifyingKey,
) -> Result<Acknowledgment> {
    handover.verify(host, id, device)?;
    let _lock = try_lock(home)?;
    let path = path(home, id);
    let record = if exists(&path)? {
        let mut previous = read(&path)?;
        // Pins may have changed through an independently authenticated key
        // rotation. Validate the private old record against its retained keys,
        // and the new handover against the caller's newly trusted pins above.
        previous.verify(
            &VerifyingKey::from_sec1_bytes(&previous.handover.host_key)?,
            id,
            &VerifyingKey::from_sec1_bytes(&previous.handover.device_key)?,
        )?;
        if previous.handover.generation == handover.generation {
            ensure!(
                same_handover(&previous.handover, &handover),
                "conflicting routing handover"
            );
            // Authenticated endpoint metadata may change without replacing the
            // generation or its cryptographic material. Retain consumed counters.
            previous.handover.relay_endpoints = handover.relay_endpoints.clone();
            previous
        } else {
            ensure!(
                handover.generation > previous.handover.generation,
                "routing generation rollback"
            );
            let generation = handover.generation;
            StoredDevice {
                version: 2,
                handover,
                states: [DeviceState {
                    generation,
                    next: 0,
                    floor: 0,
                }; 2],
            }
        }
    } else {
        let generation = handover.generation;
        StoredDevice {
            version: 2,
            handover,
            states: [DeviceState {
                generation,
                next: 0,
                floor: 0,
            }; 2],
        }
    };
    save(&path, &record)?;
    Ok(record.handover.acknowledgment())
}
pub fn begin(
    home: &Path,
    host: &VerifyingKey,
    id: Uuid,
    device: &VerifyingKey,
    transport: Transport,
) -> Result<Attempt> {
    let lock = try_lock(home)?;
    let path = path(home, id);
    let record = read(&path)?;
    let pairing = record.verify(host, id, device)?;
    record.states[index(transport)].room()?;
    Ok(Attempt {
        _lock: lock,
        path,
        record,
        pairing,
        transport,
        usable: true,
    })
}
/// The caller authenticates the direct resync response. A stale response cannot
/// lower the device's next room or highest authenticated floor.
pub fn resync_direct(
    home: &Path,
    host: &VerifyingKey,
    id: Uuid,
    device: &VerifyingKey,
    transport: Transport,
    state: HostState,
) -> Result<()> {
    let _lock = try_lock(home)?;
    let path = path(home, id);
    let mut record = read(&path)?;
    record.verify(host, id, device)?;
    let previous = record.states[index(transport)];
    ensure!(
        state.generation == previous.generation
            && state.floor >= previous.next
            && state.floor >= previous.floor,
        "routing resync generation or floor rollback"
    );
    record.states[index(transport)] = DeviceState {
        generation: state.generation,
        next: state.floor,
        floor: state.floor,
    };
    save(&path, &record)
}

/// Explicit direct recovery may replace malformed private device state. A valid
/// old image still enforces ordinary monotonic/idempotent adoption rules.
pub fn adopt_recovery_direct(
    home: &Path,
    handover: Handover,
    host: &VerifyingKey,
    id: Uuid,
    device: &VerifyingKey,
) -> Result<Acknowledgment> {
    handover.verify(host, id, device)?;
    let lock = try_lock(home)?;
    let path = path(home, id);
    if exists(&path)? {
        let encoded = read_private(&path)?;
        if let Ok(header) = serde_json::from_slice::<Version>(&encoded) {
            ensure!(
                header.version == 2,
                "unsupported device routing state version"
            );
        }
        if let Ok(previous) = serde_json::from_slice::<StoredDevice>(&encoded) {
            let valid = (|| -> Result<()> {
                previous.verify(
                    &VerifyingKey::from_sec1_bytes(&previous.handover.host_key)?,
                    id,
                    &VerifyingKey::from_sec1_bytes(&previous.handover.device_key)?,
                )?;
                Ok(())
            })();
            if valid.is_ok() {
                drop(lock);
                return adopt_direct(home, handover, host, id, device);
            }
        }
    }
    let generation = handover.generation;
    let record = StoredDevice {
        version: 2,
        handover,
        states: [DeviceState {
            generation,
            next: 0,
            floor: 0,
        }; 2],
    };
    save(&path, &record)?;
    Ok(record.handover.acknowledgment())
}

#[cfg(test)]
mod tests;
