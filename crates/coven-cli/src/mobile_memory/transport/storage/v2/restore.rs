//! Durable admission fence for an interrupted installation restore.
//! The operator installer must publish this fence before replacing authority
//! files, and may publish AwaitingHandovers only after every dependency is durable.
use super::*;
mod export;
mod install;
pub(crate) use export::run_export;
pub(crate) use install::run_install;

const MARKER: &str = ".transport-restore.required";
const JOURNAL: &str = "transport-restore.json";
const MARKER_BYTES: &[u8] = b"coven.mobile.restore.v1\n";
const MAX_RESTORE_BYTES: u64 = 64 * 1024;
// Bound registry unions plus retained restore history. Exhaustion fails closed
// rather than silently discarding already-known generation or epoch floors.
const MAX_PAIRINGS: usize = 384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Installing,
    AwaitingHandovers,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PairingTarget {
    device_id: Uuid,
    generation: u64,
    revocation_epoch: u64,
    handover_complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct State {
    version: u16,
    operation_id: Uuid,
    source_digest: [u8; 32],
    installation_fingerprint: [u8; 32],
    installation_epoch: u64,
    phase: Phase,
    pairings: Vec<PairingTarget>,
}
impl State {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported mobile restore state");
        ensure!(self.installation_epoch > 0, "invalid mobile restore epoch");
        ensure!(
            self.pairings.len() <= MAX_PAIRINGS,
            "too many restore pairings"
        );
        let mut seen = std::collections::BTreeSet::new();
        for pairing in &self.pairings {
            ensure!(seen.insert(pairing.device_id), "duplicate restore pairing");
            ensure!(
                pairing.generation > 0 && pairing.revocation_epoch > 0,
                "invalid restore handover target"
            );
            ensure!(
                self.phase != Phase::Installing || !pairing.handover_complete,
                "unfinished restore contains completed handover"
            );
        }
        Ok(())
    }
}
fn read(home: &Path) -> Result<Option<State>> {
    let directory = home.join("mobile");
    if !exists(&directory)? {
        return Ok(None);
    }
    validate_private_directory(&directory)?;
    let marker = directory.join(MARKER);
    let journal = directory.join(JOURNAL);
    let marked = exists(&marker)?;
    let recorded = exists(&journal)?;
    if !marked && !recorded {
        ensure!(
            crate::mobile_memory::registry::retained_restore_epoch(home)?.is_none(),
            "restored authority requires its restore metadata"
        );
        return Ok(None);
    }
    ensure!(
        marked && recorded,
        "mobile restore state missing; operator recovery required"
    );
    ensure!(
        read_private(&marker)?.as_slice() == MARKER_BYTES,
        "invalid mobile restore marker"
    );
    validate_private_file(&journal)?;
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(&journal)?
        .take(MAX_RESTORE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_RESTORE_BYTES as usize,
        "mobile restore state exceeds limit"
    );
    let state: State = serde_json::from_slice(&bytes)?;
    state.validate()?;
    if state.phase == Phase::AwaitingHandovers {
        ensure!(
            crate::mobile_memory::registry::retained_restore_epoch(home)?
                == Some(state.installation_epoch),
            "restore installation epoch not durably installed"
        );
    }
    let identity = crate::mobile_memory::identity::load_existing_host_identity(home)?;
    ensure!(
        identity.public_key_fingerprint == state.installation_fingerprint,
        "mobile restore installation changed"
    );
    Ok(Some(state))
}
fn save(home: &Path, state: &State) -> Result<()> {
    state.validate()?;
    let path = home.join("mobile").join(JOURNAL);
    let encoded = Zeroizing::new(serde_json::to_vec(state)?);
    ensure!(
        encoded.len() <= MAX_RESTORE_BYTES as usize,
        "mobile restore state exceeds limit"
    );
    atomic_replace_private(&path, &encoded)?;
    crate::durable_fs::sync_parent(&path)?;
    #[cfg(test)]
    crate::mobile_memory::config::write_checkpoint(
        &path,
        crate::mobile_memory::config::MobileWritePhase::RetainedParentSynced,
    )?;
    crate::durable_fs::sync_parent(path.parent().context("missing restore directory")?)?;
    #[cfg(test)]
    crate::mobile_memory::config::write_checkpoint(
        &path,
        crate::mobile_memory::config::MobileWritePhase::RetainedHomeSynced,
    )?;
    Ok(())
}

pub(crate) fn check_installation(home: &Path) -> Result<()> {
    if let Some(state) = read(home)? {
        ensure!(
            state.phase == Phase::AwaitingHandovers,
            "mobile restore installation incomplete"
        );
    }
    Ok(())
}
pub(crate) fn check_application(
    home: &Path,
    registry: &DeviceRegistry,
    device_id: Uuid,
) -> Result<()> {
    if let Some(state) = read(home)? {
        ensure!(
            state.phase == Phase::AwaitingHandovers,
            "mobile restore installation incomplete"
        );
        if let Some(target) = state
            .pairings
            .iter()
            .find(|pairing| pairing.device_id == device_id)
        {
            ensure!(target.handover_complete, "fresh restore handover required");
            // A completed journal cannot make an older mixed routing/registry
            // image current. Both must still meet the recorded restore floor.
            let path = home
                .join("mobile")
                .join(format!("transport-{device_id}.json"));
            let stored =
                super::read(&path, device_id)?.context("restored routing material missing")?;
            let generation = active(&stored)?;
            ensure!(
                generation.generation >= target.generation
                    && generation.material.revocation_epoch >= target.revocation_epoch,
                "routing state below restore target"
            );
            let host = VerifyingKey::from_sec1_bytes(&generation.material.host_key)?;
            check_installed_host(home, &host)?;
            let material = snapshot(generation, &host)?;
            authority::check_current(registry, &material.material.pairing, true, Utc::now())?;
        }
    }
    Ok(())
}
pub(crate) fn check_handover(
    home: &Path,
    device_id: Uuid,
    generation: u64,
    epoch: u64,
) -> Result<()> {
    if let Some(state) = read(home)? {
        ensure!(
            state.phase == Phase::AwaitingHandovers,
            "mobile restore installation incomplete"
        );
        if let Some(target) = state
            .pairings
            .iter()
            .find(|pairing| pairing.device_id == device_id)
        {
            ensure!(
                generation >= target.generation && epoch >= target.revocation_epoch,
                "restore handover target not reached"
            );
        }
    }
    Ok(())
}
/// Called under the transport lock, after the exact pending acknowledgment has
/// been checked and the host generation durably published. A retry repeats the
/// journal barriers, including when its first namespace result was uncertain.
pub(super) fn confirm_handover(
    home: &Path,
    device_id: Uuid,
    generation: u64,
    epoch: u64,
) -> Result<()> {
    check_handover(home, device_id, generation, epoch)?;
    if let Some(mut state) = read(home)? {
        if let Some(target) = state
            .pairings
            .iter_mut()
            .find(|pairing| pairing.device_id == device_id)
        {
            target.handover_complete = true;
            save(home, &state)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod control_tests;
