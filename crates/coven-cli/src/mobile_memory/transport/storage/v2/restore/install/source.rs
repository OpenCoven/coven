//! Captured private mobile snapshot. Current signed authority supersedes its
//! grants, policy and routing secrets; only identity/listener settings are copied.
use super::*;
use crate::mobile_memory::config;

const MAX_SNAPSHOT_BYTES: usize = 20 * 1024 * 1024;
const COPIED: [&str; 4] = [
    "host-key.pem",
    "host-identity.json",
    "gateway.json",
    "relay.json",
];

pub(super) struct SnapshotSource {
    files: BTreeMap<String, Option<Zeroizing<Vec<u8>>>>,
    pub registry: RestoreRegistry,
    pub generations: BTreeMap<Uuid, u64>,
    pub revocation_epochs: BTreeMap<Uuid, u64>,
    pub digest: [u8; 32],
}
fn component(home: &Path, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let path = home.join("mobile").join(name);
    if !exists(&path)? {
        return Ok(None);
    }
    validate_private_file(&path)?;
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(&path)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "snapshot component exceeds size limit"
    );
    Ok(Some(bytes))
}
impl SnapshotSource {
    pub fn require_disjoint_homes(home: &Path, source: &Path) -> Result<()> {
        use cap_fs_ext::MetadataExt;
        for (root, other) in [(home, source), (source, home)] {
            let root = match fs::symlink_metadata(root) {
                Ok(_) => cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())?
                    .dir_metadata()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            // Compare the nearest existing ancestor before creating any part
            // of a missing destination. Canonicalization resolves parent aliases;
            // directory identity also handles case-insensitive spellings.
            let absolute = std::path::absolute(other)?;
            let mut existing = None;
            for ancestor in absolute.ancestors() {
                match fs::canonicalize(ancestor) {
                    Ok(path) => {
                        existing = Some(path);
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            let existing = existing.context("snapshot path has no existing ancestor")?;
            for ancestor in existing.ancestors() {
                let metadata =
                    cap_std::fs::Dir::open_ambient_dir(ancestor, cap_std::ambient_authority())?
                        .dir_metadata()?;
                ensure!(
                    root.dev() != metadata.dev() || root.ino() != metadata.ino(),
                    "snapshot source and destination must be separate, non-overlapping homes"
                );
            }
        }
        Ok(())
    }
    pub fn capture(home: &Path) -> Result<Self> {
        validate_private_directory(&home.join("mobile"))?;
        let mut files = BTreeMap::new();
        for name in [
            "host-key.pem",
            "host-identity.json",
            "gateway.json",
            "relay.json",
            "devices.json",
            "authorization-keys.json",
            "introduction-policy.json",
            MARKER,
            JOURNAL,
        ] {
            files.insert(name.to_owned(), component(home, name)?);
        }
        let required = |name: &str| -> Result<&[u8]> {
            files
                .get(name)
                .and_then(Option::as_ref)
                .map(|b| b.as_slice())
                .context("selected snapshot component missing")
        };
        let host = identity::decode_snapshot_identity(
            required("host-key.pem")?,
            required("host-identity.json")?,
        )?;
        let registry = RestoreRegistry::decode(required("devices.json")?)?;
        // Snapshot restoration metadata must be complete and coherent. This is
        // read-only; ordinary readers never manufacture missing source state.
        super::super::check_installation(home)?;
        if let Some(config) = files.get("gateway.json").and_then(Option::as_ref) {
            config::validate_mobile_config(&serde_json::from_slice(config)?)?;
        }
        if let Some(config) = files.get("relay.json").and_then(Option::as_ref) {
            let config: RelayConfig = serde_json::from_slice(config)?;
            config.validate()?;
        }
        let mut generations = BTreeMap::new();
        let mut revocation_epochs = BTreeMap::new();
        let journal = super::journal_for_install(home, host.public_key_fingerprint)?;
        let revisions = registry.revisions();
        for revision in &revisions {
            let anchor = export::anchor(home, revision.device_id)?;
            generations.insert(
                revision.device_id,
                anchor.counter(revision.device_id).unwrap_or(0),
            );
            revocation_epochs.insert(revision.device_id, revision.revocation_epoch);
        }
        if let Some(journal) = &journal {
            for target in &journal.pairings {
                let retained = export::retained_target(home, target)?;
                generations
                    .entry(retained.device_id)
                    .and_modify(|g| *g = (*g).max(retained.generation))
                    .or_insert(retained.generation);
                revocation_epochs
                    .entry(retained.device_id)
                    .and_modify(|e| *e = (*e).max(retained.revocation_epoch))
                    .or_insert(retained.revocation_epoch);
            }
        }
        for id in generations.keys() {
            for name in [
                format!("transport-{id}.json"),
                format!(".transport-v2-{id}.required"),
            ] {
                files.insert(name.clone(), component(home, &name)?);
            }
        }
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        digest.update(b"coven.mobile.restore.snapshot.v1\0");
        let mut total = 0_usize;
        for (name, bytes) in &files {
            digest.update(&(name.len() as u64).to_be_bytes());
            digest.update(name.as_bytes());
            digest.update(&[u8::from(bytes.is_some())]);
            if let Some(bytes) = bytes {
                total = total
                    .checked_add(bytes.len())
                    .context("snapshot size overflow")?;
                ensure!(
                    total <= MAX_SNAPSHOT_BYTES,
                    "selected snapshot exceeds size limit"
                );
                digest.update(&(bytes.len() as u64).to_be_bytes());
                digest.update(bytes);
            }
        }
        Ok(Self {
            files,
            registry,
            generations,
            revocation_epochs,
            digest: digest.finish().as_ref().try_into()?,
        })
    }
    pub fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.files
            .get(name)
            .and_then(Option::as_ref)
            .map(|b| b.as_slice())
    }
    pub fn host(&self) -> Result<identity::HostIdentity> {
        identity::decode_snapshot_identity(
            self.bytes("host-key.pem").context("snapshot key missing")?,
            self.bytes("host-identity.json")
                .context("snapshot identity missing")?,
        )
    }
    pub fn validate_authority(
        &self,
        incoming: &RestoreRegistry,
        keys: &[u8],
        policy: Option<&[u8]>,
        store: &OwnerPolicyStore,
    ) -> Result<()> {
        incoming.check_not_older_than(&self.registry)?;
        assurance::validate_restore_key_source(keys, self.bytes("authorization-keys.json"))?;
        store.validate_restore_source(policy, self.bytes("introduction-policy.json"))
    }
    pub fn install(&self, home: &Path) -> Result<()> {
        // Caller has already made Installing durable. Do not copy old routing
        // keys or registry authority, and never copy lock/fence paths.
        for name in COPIED {
            let path = home.join("mobile").join(name);
            if exists(&path)? {
                validate_private_file(&path)?;
            }
            if let Some(bytes) = self.bytes(name) {
                atomic_replace_private(&path, bytes)?;
            } else if exists(&path)? {
                fs::remove_file(&path)?;
            }
            // Repeat namespace barriers even for an already absent optional
            // file, so an uncertain deletion cannot skip persistence on retry.
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
        }
        ensure!(
            identity::load_existing_host_identity(home)?.public_key_fingerprint
                == self.host()?.public_key_fingerprint,
            "installed snapshot identity does not match captured pin"
        );
        Ok(())
    }
}
