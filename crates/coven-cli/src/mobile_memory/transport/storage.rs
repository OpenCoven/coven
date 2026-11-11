//! Private, versioned relay configuration and pairwise transport provisioning.
use super::websocket::EndpointConfig;
use super::{authority, generate_noise_key, nonce, Pairing};
use crate::mobile_memory::{
    config::{
        atomic_replace_private, ensure_private_mobile_dir, validate_private_directory,
        validate_private_file,
    },
    registry::DeviceRegistry,
};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use fs2::FileExt;
use p256::ecdsa::{SigningKey, VerifyingKey};
use ring::hkdf;
use serde::{Deserialize, Serialize};
use snow::resolvers::{CryptoResolver, DefaultResolver};
use std::{fs, io::Read, path::Path, sync::Mutex};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Versioned durable host and device helpers for authenticated v2 transport.
pub mod v2;

static MATERIAL_LOCK: Mutex<()> = Mutex::new(());
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

const MAX_STATE_BYTES: u64 = 16 * 1024;
pub const SLOT_SECONDS: u64 = 600;
pub const MAX_LISTENERS: usize = 512;
fn default_listener_limit() -> usize {
    MAX_LISTENERS
}
fn is_default_listener_limit(limit: &usize) -> bool {
    *limit == MAX_LISTENERS
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RelayConfig {
    #[serde(
        default = "default_listener_limit",
        skip_serializing_if = "is_default_listener_limit"
    )]
    pub listener_limit: usize,
    pub version: u16,
    pub enabled: bool,
    pub endpoints: Vec<EndpointConfig>,
}
impl RelayConfig {
    fn disabled() -> Self {
        Self {
            listener_limit: MAX_LISTENERS,
            version: 1,
            enabled: false,
            endpoints: Vec::new(),
        }
    }

    fn with_config_lock(home: &Path, operation: impl FnOnce() -> Result<()>) -> Result<()> {
        let _process = crate::mobile_memory::store_lock::process(&CONFIG_LOCK)
            .context("relay configuration lock unavailable")?;
        let directory = ensure_private_mobile_dir(home)?;
        let lock = crate::state_lock::open_lock_file(&directory.join(".relay.lock"))?;
        crate::mobile_memory::store_lock::lock_exclusive(&lock, &directory.join(".relay.lock"))?;
        operation()?;
        FileExt::unlock(&lock)?;
        Ok(())
    }

    /// Serialize incremental operator changes across threads and processes.
    pub fn update(home: &Path, operation: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        Self::with_config_lock(home, || {
            let mut config = Self::load(home)?.unwrap_or_else(Self::disabled);
            operation(&mut config)?;
            config.save(home)
        })
    }

    /// Replace configuration without decoding damaged state. Private-file and
    /// atomic-replacement checks still apply; paired material is never opened.
    pub fn reset(home: &Path) -> Result<()> {
        Self::with_config_lock(home, || Self::disabled().save(home))
    }

    pub fn disable(home: &Path) -> Result<bool> {
        if Self::load(home)?.is_none() {
            return Ok(false);
        }
        let mut enabled = false;
        Self::update(home, |config| {
            enabled = config.enabled;
            config.enabled = false;
            Ok(())
        })?;
        Ok(enabled)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.listener_limit <= MAX_LISTENERS,
            "listener limit exceeds 512 rooms"
        );
        ensure!(self.version == 1, "unsupported relay configuration");
        ensure!(
            self.endpoints.len() <= 4 && (!self.enabled || !self.endpoints.is_empty()),
            "invalid relay endpoint count"
        );
        let mut seen = std::collections::HashSet::new();
        for endpoint in &self.endpoints {
            endpoint.parse()?;
            ensure!(
                seen.insert(url::Url::parse(&endpoint.url)?),
                "duplicate relay endpoint"
            );
        }
        ensure!(
            serde_json::to_vec(self)?.len() as u64 <= MAX_STATE_BYTES,
            "relay configuration too large"
        );
        Ok(())
    }
    pub fn load(home: &Path) -> Result<Option<Self>> {
        Self::load_inner(home).context(
            "cannot read relay configuration; to discard relay settings and disable access, run `coven memory mobile relay reset --yes` (paired devices and identity are retained)",
        )
    }

    fn load_inner(home: &Path) -> Result<Option<Self>> {
        let directory = home.join("mobile");
        if !exists(&directory)? {
            return Ok(None);
        }
        validate_private_directory(&directory)?;
        let path = directory.join("relay.json");
        if !exists(&path)? {
            return Ok(None);
        }
        let config: Self = serde_json::from_slice(&read_private(&path)?).map_err(|error| {
            // Serde data errors can quote saved field names and values. The CLI
            // prints error chains, so retain only structural parse diagnostics.
            anyhow::anyhow!(
                "invalid relay configuration JSON ({:?}, line {}, column {})",
                error.classify(),
                error.line(),
                error.column()
            )
        })?;
        config.validate()?;
        Ok(Some(config))
    }
    pub fn save(&self, home: &Path) -> Result<()> {
        self.validate()?;
        let path = ensure_private_mobile_dir(home)?.join("relay.json");
        atomic_replace_private(&path, &serde_json::to_vec(self)?)
    }
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredMaterial {
    version: u16,
    device_id: String,
    grant_id: String,
    revocation_epoch: u64,
    host_key: Vec<u8>,
    device_key: Vec<u8>,
    noise_private: Vec<u8>,
    noise_public: [u8; 32],
    routing_seed: [u8; 32],
}

#[derive(Clone)]
pub struct Material {
    pub pairing: Pairing,
    pub noise_private: Zeroizing<Vec<u8>>,
    routing_seed: Zeroizing<[u8; 32]>,
}

/// Contains a routing secret. Serialize only inside pinned TLS or an authenticated
/// encrypted channel and never Debug/log it. This struct zeroizes its own fields
/// on drop, but gateway serialization copies do not; see the transport README.
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Provisioning {
    pub version: u16,
    pub device_id: String,
    pub grant_id: String,
    pub revocation_epoch: u64,
    pub host_key: Vec<u8>,
    pub device_key: Vec<u8>,
    pub noise_public: [u8; 32],
    pub binding_signature: Vec<u8>,
    pub routing_seed: [u8; 32],
    pub slot_seconds: u64,
    pub relay_endpoints: Vec<EndpointConfig>,
}

impl Material {
    pub fn provisioning(&self, host: &SigningKey) -> Result<Provisioning> {
        Ok(Provisioning {
            version: 1,
            device_id: self.pairing.device_id.to_string(),
            grant_id: self.pairing.grant_id.to_string(),
            revocation_epoch: self.pairing.revocation_epoch,
            host_key: self
                .pairing
                .host_key
                .to_sec1_point(false)
                .as_bytes()
                .to_vec(),
            device_key: self
                .pairing
                .device_key
                .to_sec1_point(false)
                .as_bytes()
                .to_vec(),
            noise_public: self.pairing.noise_key,
            binding_signature: self.pairing.sign_noise_binding(host)?,
            routing_seed: *self.routing_seed,
            slot_seconds: SLOT_SECONDS,
            relay_endpoints: Vec::new(),
        })
    }

    pub fn rendezvous(&self, slot: u64, direct: bool) -> Result<([u8; 32], Zeroizing<[u8; 32]>)> {
        derive_rendezvous(&self.routing_seed, slot, direct)
    }
}

/// Authenticate provisioned transport keys against existing pins, not fields
/// supplied by the provisioning response. The routing secret grants no authority.
impl Provisioning {
    pub fn verify(
        &self,
        pinned_host: &VerifyingKey,
        device_id: Uuid,
        device: &VerifyingKey,
    ) -> Result<Pairing> {
        ensure!(
            self.version == 1 && self.slot_seconds == SLOT_SECONDS,
            "unsupported provisioning"
        );
        ensure!(
            self.device_id == device_id.to_string(),
            "provisioning device mismatch"
        );
        ensure!(
            self.host_key == pinned_host.to_sec1_point(false).as_bytes()
                && self.device_key == device.to_sec1_point(false).as_bytes(),
            "provisioning identity mismatch"
        );
        RelayConfig {
            listener_limit: crate::mobile_memory::transport::storage::MAX_LISTENERS,
            version: 1,
            enabled: !self.relay_endpoints.is_empty(),
            endpoints: self.relay_endpoints.clone(),
        }
        .validate()?;
        let pairing = Pairing {
            device_id,
            grant_id: Uuid::parse_str(&self.grant_id)?,
            revocation_epoch: self.revocation_epoch,
            host_key: *pinned_host,
            device_key: *device,
            noise_key: self.noise_public,
        };
        pairing.verify_noise_binding(&self.binding_signature)?;
        Ok(pairing)
    }
    pub fn rendezvous(&self, slot: u64, direct: bool) -> Result<([u8; 32], Zeroizing<[u8; 32]>)> {
        derive_rendezvous(&self.routing_seed, slot, direct)
    }
}

pub fn load_or_provision(
    home: &Path,
    registry: &DeviceRegistry,
    host: &SigningKey,
    device_id: Uuid,
) -> Result<Material> {
    let _process = crate::mobile_memory::store_lock::process(&MATERIAL_LOCK)
        .context("transport state lock unavailable")?;
    let directory = ensure_private_mobile_dir(home)?;
    let lock = crate::state_lock::open_lock_file(&directory.join(".transport.lock"))?;
    crate::mobile_memory::store_lock::lock_exclusive(&lock, &directory.join(".transport.lock"))?;
    v2::restore::check_application(home, registry, device_id)?;
    ensure!(
        !requires_v2(home, device_id)?,
        "v2 routing handover required; v1 downgrade refused"
    );
    registry.reload()?;
    let record = registry
        .authorization_record(device_id)?
        .context("unknown transport device")?;
    let device_key = URL_SAFE_NO_PAD.decode(&record.device.public_key_x963)?;
    let host_key = host
        .verifying_key()
        .to_sec1_point(false)
        .as_bytes()
        .to_vec();
    let path = directory.join(format!("transport-{device_id}.json"));
    let existing: Option<StoredMaterial> = if exists(&path)? {
        Some(serde_json::from_slice(&read_private(&path)?)?)
    } else {
        None
    };
    if let Some(stored) = &existing {
        ensure!(
            stored.version == 1 && stored.device_id == device_id.to_string(),
            "invalid transport state"
        );
        validate_keypair(stored)?;
    }
    let matches = existing.as_ref().is_some_and(|stored| {
        stored.grant_id == record.grant.id.to_string()
            && stored.revocation_epoch == record.grant.revocation_epoch
            && stored.host_key == host_key
            && stored.device_key == device_key
    });
    let material = if matches {
        let stored = existing.as_ref().context("missing transport state")?;
        Material {
            pairing: Pairing {
                device_id,
                grant_id: record.grant.id,
                revocation_epoch: record.grant.revocation_epoch,
                host_key: *host.verifying_key(),
                device_key: VerifyingKey::from_sec1_bytes(&device_key)?,
                noise_key: stored.noise_public,
            },
            noise_private: Zeroizing::new(stored.noise_private.clone()),
            routing_seed: Zeroizing::new(stored.routing_seed),
        }
    } else {
        let (noise_private, noise_public) = generate_noise_key()?;
        Material {
            pairing: Pairing {
                device_id,
                grant_id: record.grant.id,
                revocation_epoch: record.grant.revocation_epoch,
                host_key: *host.verifying_key(),
                device_key: VerifyingKey::from_sec1_bytes(&device_key)?,
                noise_key: noise_public,
            },
            noise_private,
            routing_seed: Zeroizing::new(nonce()?),
        }
    };
    // Provisioning can use the already-pinned direct channel even if relay access
    // is disallowed. Actual relay admission enforces DirectOnly separately.
    authority::check_current(registry, &material.pairing, true, Utc::now())?;
    if !matches {
        let stored = StoredMaterial {
            version: 1,
            device_id: device_id.to_string(),
            grant_id: record.grant.id.to_string(),
            revocation_epoch: record.grant.revocation_epoch,
            host_key,
            device_key,
            noise_private: material.noise_private.to_vec(),
            noise_public: material.pairing.noise_key,
            routing_seed: *material.routing_seed,
        };
        let encoded = Zeroizing::new(serde_json::to_vec(&stored)?);
        atomic_replace_private(&path, &encoded)?;
    }
    FileExt::unlock(&lock)?;
    Ok(material)
}

/// Read an already-bootstrapped pairing without taking the provisioning write
/// lock or manufacturing material the enrolled device has never received.
pub fn load_existing(
    home: &Path,
    registry: &DeviceRegistry,
    host: &VerifyingKey,
    device_id: Uuid,
) -> Result<Option<Material>> {
    let directory = home.join("mobile");
    validate_private_directory(&directory)?;
    v2::restore::check_application(home, registry, device_id)?;
    ensure!(
        !requires_v2(home, device_id)?,
        "v2 routing required; v1 downgrade refused"
    );
    let path = directory.join(format!("transport-{device_id}.json"));
    if !exists(&path)? {
        return Ok(None);
    }
    let stored: StoredMaterial = serde_json::from_slice(&read_private(&path)?)?;
    ensure!(
        stored.version == 1 && stored.device_id == device_id.to_string(),
        "invalid transport state"
    );
    ensure!(
        stored.host_key == host.to_sec1_point(false).as_bytes(),
        "transport host identity changed"
    );
    validate_keypair(&stored)?;
    let material = Material {
        pairing: Pairing {
            device_id,
            grant_id: Uuid::parse_str(&stored.grant_id)?,
            revocation_epoch: stored.revocation_epoch,
            host_key: *host,
            device_key: VerifyingKey::from_sec1_bytes(&stored.device_key)?,
            noise_key: stored.noise_public,
        },
        noise_private: Zeroizing::new(stored.noise_private.clone()),
        routing_seed: Zeroizing::new(stored.routing_seed),
    };
    authority::check_current(registry, &material.pairing, false, Utc::now())?;
    Ok(Some(material))
}

/// Presence is a fail-closed protocol floor, including an invalid or damaged
/// marker. Removing only material must never make legacy bootstrap eligible.
fn requires_v2(home: &Path, device_id: Uuid) -> Result<bool> {
    exists(
        &home
            .join("mobile")
            .join(format!(".transport-v2-{device_id}.required")),
    )
}

fn read_private(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    validate_private_file(path)?;
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)?
        .take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_STATE_BYTES as usize,
        "transport state exceeds limit"
    );
    Ok(bytes)
}

fn validate_keypair(stored: &StoredMaterial) -> Result<()> {
    ensure!(stored.noise_private.len() == 32, "invalid transport key");
    let mut dh = DefaultResolver
        .resolve_dh(&snow::params::DHChoice::Curve25519)
        .context("Noise key resolver unavailable")?;
    dh.set(&stored.noise_private);
    ensure!(dh.pubkey() == stored.noise_public, "transport key mismatch");
    Ok(())
}

fn derive_rendezvous(
    seed: &[u8; 32],
    slot: u64,
    direct: bool,
) -> Result<([u8; 32], Zeroizing<[u8; 32]>)> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"coven.mobile.rendezvous.v1");
    let key = salt.extract(seed);
    let epoch = slot.to_be_bytes();
    let transport: &[u8] = if direct { b"direct" } else { b"relay" };
    let derive = |label: &[u8]| -> Result<[u8; 32]> {
        let info = [transport, label, epoch.as_slice()];
        let output = key
            .expand(&info, hkdf::HKDF_SHA256)
            .map_err(|_| anyhow::anyhow!("rendezvous derivation failed"))?;
        let mut value = [0; 32];
        output
            .fill(&mut value)
            .map_err(|_| anyhow::anyhow!("rendezvous derivation failed"))?;
        Ok(value)
    };
    Ok((derive(b"room")?, Zeroizing::new(derive(b"credential")?)))
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod wire_vectors {
    use super::*;

    #[test]
    fn routing_matches_independent_hkdf_vectors() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("vectors/wire.json")).unwrap();
        let seed = std::array::from_fn(|index| index as u8);
        let hex = |value: &[u8]| {
            value
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        for case in vectors["routing"].as_array().unwrap() {
            let (room, credential) = derive_rendezvous(
                &seed,
                case["slot"].as_u64().unwrap(),
                case["transport"].as_str().unwrap() == "direct",
            )
            .unwrap();
            assert_eq!(hex(&room), case["room"].as_str().unwrap());
            assert_eq!(
                hex(credential.as_ref()),
                case["credential"].as_str().unwrap()
            );
        }
    }
}

#[cfg(test)]
mod listener_config_tests {
    use super::*;
    #[test]
    fn listener_limit_configuration_is_bounded_and_survives_private_roundtrip() {
        for limit in [0, 1, 4, 512] {
            let home = tempfile::tempdir().unwrap();
            let config:RelayConfig=serde_json::from_value(serde_json::json!({"version":1,"enabled":false,"endpoints":[],"listenerLimit":limit})).unwrap();
            config.save(home.path()).unwrap();
            assert!(RelayConfig::load(home.path()).unwrap().unwrap() == config);
            let before = fs::read(home.path().join("mobile/relay.json")).unwrap();
            if let Ok(bad) = serde_json::from_value::<RelayConfig>(
                serde_json::json!({"version":1,"enabled":false,"endpoints":[],"listenerLimit":513}),
            ) {
                assert!(bad.save(home.path()).is_err());
            }
            assert_eq!(
                fs::read(home.path().join("mobile/relay.json")).unwrap(),
                before
            );
        }
    }
}
