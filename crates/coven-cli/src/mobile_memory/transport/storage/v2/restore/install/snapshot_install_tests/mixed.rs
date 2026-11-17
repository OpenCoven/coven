//! Mixed-source restore. The selected snapshot, the retained destination and
//! the latest signed authority are three independent images of one installation
//! lineage, so they can disagree in registry, authorization keys, owner policy
//! and routing generations at once. Owner policy is present at every point.
//!
//! Expectations are derived from the raw captured files, not from the
//! installer's own readers, so a precedence mistake cannot hide in a shared
//! helper.
use super::*;
use crate::mobile_memory::{
    assurance::{AssuranceClass, AuthorizationKeyRegistry, NewAuthorizationKey},
    grant::{DeviceGrant, DeviceScope},
    registry::DeviceRecord,
};
use serde_json::Value;
use std::path::PathBuf;

pub(super) type Files = BTreeMap<String, Vec<u8>>;

/// Pairing ids. `FIRST` is the shared fixture's pairing.
pub(super) const FIRST: Uuid = Uuid::from_u128(5);
pub(super) const SECOND: Uuid = Uuid::from_u128(6);
pub(super) const THIRD: Uuid = Uuid::from_u128(7);
pub(super) const LATE: Uuid = Uuid::from_u128(8);

/// Lineage points, in the order they happened.
pub(super) const BASE: usize = 0;
pub(super) const OFFERED: usize = 1;
pub(super) const TIGHTENED: usize = 2;
pub(super) const ACKNOWLEDGED: usize = 3;
pub(super) const RESTORED: usize = 4;
pub(super) const HANDED_OVER: usize = 5;
pub(super) const FORGOTTEN: usize = 6;
pub(super) const RESTORED_AGAIN: usize = 7;

pub(super) struct Point {
    pub(super) name: &'static str,
    /// The complete private mobile directory at this instant.
    pub(super) files: Files,
    /// The signed authority export captured at the same instant.
    pub(super) authority: PathBuf,
}

pub(super) struct Lineage {
    pub(super) host: SigningKey,
    pub(super) pin: [u8; 32],
    pub(super) points: Vec<Point>,
    backup: tempfile::TempDir,
    _homes: Vec<tempfile::TempDir>,
}

pub(super) fn device_key(id: Uuid) -> SigningKey {
    // The shared fixture pairs `FIRST` with this fixed key.
    let byte = if id == FIRST { 2 } else { id.as_u128() as u8 };
    SigningKey::from_slice(&[byte; 32]).unwrap()
}
fn public(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes())
}
fn pair(f: &Fixture, id: Uuid) {
    let key = public(&device_key(id));
    let now = Utc::now();
    let record = DeviceRecord {
        id,
        display_name: format!("Synthetic mixed restore phone {}", id.as_u128()),
        public_key_x963: key.clone(),
        paired_at: now,
        revoked_at: None,
        suspended_at: None,
        scopes: vec![DeviceScope::MemoryRead],
    };
    let grant = DeviceGrant::for_device(id, &key, record.scopes.clone(), now).unwrap();
    f.registry.register_with_grant(record, grant).unwrap();
}
fn offer(f: &Fixture, id: Uuid) -> Handover {
    prepare_direct(f.home.path(), &f.registry, &f.host, id).unwrap()
}
fn commit(f: &Fixture, id: Uuid, handover: &Handover) {
    acknowledge_direct(
        f.home.path(),
        &f.registry,
        &f.host,
        id,
        &handover.acknowledgment(),
    )
    .unwrap();
}
fn regrant(f: &Fixture, id: Uuid) {
    let record = f.registry.authorization_record(id).unwrap().unwrap();
    let now = Utc::now();
    let replacement = record
        .grant
        .reissue(
            &record.device.public_key_x963,
            record.grant.scopes.clone(),
            record.grant.restrictions.clone(),
            record.grant.minimum_assurance,
            now,
            now + chrono::Duration::hours(6),
        )
        .unwrap();
    f.registry.replace_grant(id, replacement).unwrap();
}
pub(super) fn read_files(home: &Path) -> Files {
    fs::read_dir(home.join("mobile"))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
/// Materializes a captured image. These copies are fixtures, not evidence, so
/// on Unix they skip the durability barriers that the installer under test
/// repeats itself; elsewhere the production private writer sets the access.
pub(super) fn write_files(home: &Path, files: &Files) {
    let directory = ensure_private_mobile_dir(home).unwrap();
    for (name, bytes) in files {
        #[cfg(unix)]
        {
            use std::{io::Write, os::unix::fs::OpenOptionsExt};
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(directory.join(name))
                .unwrap()
                .write_all(bytes)
                .unwrap();
        }
        #[cfg(not(unix))]
        atomic_replace_private(&directory.join(name), bytes).unwrap();
    }
}
/// Advisory lock files carry no authority and are created by readers.
pub(super) fn authority_files(files: &Files) -> Files {
    files
        .iter()
        .filter(|(name, _)| !name.ends_with(".lock"))
        .map(|(name, bytes)| (name.clone(), bytes.clone()))
        .collect()
}
fn continue_in(home: tempfile::TempDir, host: &SigningKey) -> Fixture {
    let registry = DeviceRegistry::load(home.path()).unwrap();
    Fixture {
        home,
        registry,
        host: host.clone(),
        device: device_key(FIRST),
        id: FIRST,
    }
}

impl Lineage {
    fn capture(&mut self, name: &'static str, f: &Fixture) {
        assert_eq!(POINTS[self.points.len()], name, "lineage order changed");
        let authority = self
            .backup
            .path()
            .join("mobile")
            .join(format!("{name}.json"));
        export::export_authority(f.home.path(), &authority).unwrap();
        self.points.push(Point {
            name,
            files: read_files(f.home.path()),
            authority,
        });
    }
    /// A real operator restore into an empty home, which then becomes live.
    fn restore(&self, snapshot: usize, authority: usize) -> tempfile::TempDir {
        let source = tempfile::tempdir().unwrap();
        write_files(source.path(), &self.points[snapshot].files);
        let home = tempfile::tempdir().unwrap();
        install_from_snapshot(
            home.path(),
            source.path(),
            &self.points[authority].authority,
            self.pin,
            Uuid::new_v4(),
        )
        .unwrap();
        home
    }
    pub(super) fn build() -> Self {
        let f = Fixture::new();
        let backup = tempfile::tempdir().unwrap();
        ensure_private_mobile_dir(backup.path()).unwrap();
        let mut lineage = Self {
            host: f.host.clone(),
            pin: identity::load_existing_host_identity(f.home.path())
                .unwrap()
                .public_key_fingerprint,
            points: Vec::new(),
            backup,
            _homes: Vec::new(),
        };

        // BASE: three committed pairings, one enrolled authorization key and
        // owner policy revision 1.
        export::tests::install_policy(&f);
        pair(&f, SECOND);
        pair(&f, THIRD);
        for id in [FIRST, SECOND, THIRD] {
            let handover = offer(&f, id);
            assert_eq!(handover.generation, 0);
            commit(&f, id, &handover);
        }
        let keys = AuthorizationKeyRegistry::load(f.home.path()).unwrap();
        keys.enroll_initial(
            SECOND,
            &public(&device_key(SECOND)),
            NewAuthorizationKey {
                public_key_x963: public(&SigningKey::from_slice(&[16; 32]).unwrap()),
                assurance_class: AssuranceClass::BiometricOnly,
                enrolled_at: Utc::now(),
            },
        )
        .unwrap();
        lineage.capture(POINTS[BASE], &f);

        // OFFERED: generation 1 is offered but never acknowledged. Registry,
        // keys and policy are unchanged; only routing moved.
        assert_eq!(offer(&f, FIRST).generation, 1);
        lineage.capture(POINTS[OFFERED], &f);

        // TIGHTENED: a re-grant burns the pending generation 1 and offers 2;
        // the second pairing and its key are revoked; policy moves to 2.
        regrant(&f, FIRST);
        let pending = offer(&f, FIRST);
        assert_eq!((pending.generation, pending.revocation_epoch), (2, 1));
        keys.revoke(SECOND, Utc::now()).unwrap();
        f.registry.revoke(SECOND, Utc::now()).unwrap();
        export::tests::install_policy(&f);
        lineage.capture(POINTS[TIGHTENED], &f);

        // ACKNOWLEDGED: generation 2 commits and the third pairing is offered
        // generation 1. Again only routing moved.
        commit(&f, FIRST, &pending);
        assert_eq!(offer(&f, THIRD).generation, 1);
        lineage.capture(POINTS[ACKNOWLEDGED], &f);

        // RESTORED: the base snapshot is restored with the latest authority
        // into a new home. Nothing is handed over yet, so every target is a
        // journal reservation.
        let f = continue_in(lineage.restore(BASE, ACKNOWLEDGED), &lineage.host);
        lineage.capture(POINTS[RESTORED], &f);

        // HANDED_OVER: the first pairing completes its fresh handover, the
        // third is offered its reserved generation without acknowledging and
        // policy moves to 3.
        let fresh = offer(&f, FIRST);
        commit(&f, FIRST, &fresh);
        offer(&f, THIRD);
        export::tests::install_policy(&f);
        lineage.capture(POINTS[HANDED_OVER], &f);

        // FORGOTTEN: every pairing is forgotten, leaving journal-only floors,
        // and a late pairing is registered and committed. Policy is unchanged.
        f.registry.forget_all().unwrap();
        pair(&f, LATE);
        let late = offer(&f, LATE);
        commit(&f, LATE, &late);
        lineage.capture(POINTS[FORGOTTEN], &f);
        let previous = f;

        // RESTORED_AGAIN: a second restore advances the journal-only floors;
        // the late pairing is offered its reservation and policy moves to 4.
        let f = continue_in(lineage.restore(RESTORED, FORGOTTEN), &lineage.host);
        offer(&f, LATE);
        export::tests::install_policy(&f);
        lineage.capture(POINTS[RESTORED_AGAIN], &f);
        lineage._homes.extend([previous.home, f.home]);
        lineage
    }
}
pub(super) const POINTS: [&str; 8] = [
    "base",
    "offered",
    "tightened",
    "acknowledged",
    "restored",
    "handed-over",
    "forgotten",
    "restored-again",
];

pub(super) fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
fn id(value: &Value) -> Uuid {
    value.as_str().unwrap().parse().unwrap()
}
fn raise(map: &mut BTreeMap<Uuid, u64>, id: Uuid, value: u64) {
    map.entry(id)
        .and_modify(|current| *current = (*current).max(value))
        .or_insert(value);
}

/// Everything one source proves about floors, read from its raw bytes.
#[derive(Debug, Default, Clone)]
pub(super) struct Known {
    pub(super) installation_epoch: u64,
    pub(super) generations: BTreeMap<Uuid, u64>,
    pub(super) epochs: BTreeMap<Uuid, u64>,
    pub(super) pairings: BTreeSet<Uuid>,
}
impl Known {
    fn registry(&mut self, registry: &Value) {
        self.installation_epoch = self
            .installation_epoch
            .max(registry["restoreEpoch"].as_u64().unwrap_or(0));
        for record in registry["devices"].as_array().unwrap() {
            let device = id(&record["device"]["id"]);
            self.pairings.insert(device);
            raise(
                &mut self.epochs,
                device,
                record["grant"]["revocationEpoch"].as_u64().unwrap(),
            );
        }
    }
    fn target(&mut self, target: &Value) {
        let device = id(&target["deviceId"]);
        self.pairings.insert(device);
        raise(
            &mut self.generations,
            device,
            target["generation"].as_u64().unwrap(),
        );
        raise(
            &mut self.epochs,
            device,
            target["revocationEpoch"].as_u64().unwrap(),
        );
    }
    pub(super) fn image(files: &Files) -> Self {
        let mut known = Self::default();
        known.registry(&json(&files["devices.json"]));
        if let Some(journal) = files.get(JOURNAL) {
            let journal = json(journal);
            known.installation_epoch = known
                .installation_epoch
                .max(journal["installationEpoch"].as_u64().unwrap());
            for target in journal["pairings"].as_array().unwrap() {
                known.target(target);
            }
        }
        for (name, bytes) in files {
            let Some(device) = name
                .strip_prefix("transport-")
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|name| name.parse::<Uuid>().ok())
            else {
                continue;
            };
            let stored = json(bytes);
            assert_eq!(stored["version"], 2, "lineage holds only v2 routing");
            if let Some(minimum) = stored["minimumGeneration"].as_u64() {
                raise(&mut known.generations, device, minimum);
            }
            for generation in [&stored["committed"], &stored["pending"]] {
                if generation.is_null() {
                    continue;
                }
                raise(
                    &mut known.generations,
                    device,
                    generation["generation"].as_u64().unwrap(),
                );
                raise(
                    &mut known.epochs,
                    device,
                    generation["material"]["revocationEpoch"].as_u64().unwrap(),
                );
            }
        }
        known
    }
    pub(super) fn export(authority: &Path) -> Self {
        let export = json(&fs::read(authority).unwrap());
        let mut known = Self {
            installation_epoch: export["installationEpoch"].as_u64().unwrap(),
            ..Self::default()
        };
        known.registry(&exported(&export, "registry"));
        for anchor in export["routing"].as_array().unwrap() {
            if anchor["profile"] == "counter" {
                raise(
                    &mut known.generations,
                    id(&anchor["deviceId"]),
                    anchor["highestGeneration"].as_u64().unwrap(),
                );
            }
        }
        for target in export["retainedTargets"].as_array().unwrap() {
            known.target(target);
        }
        known
    }
}
fn exported(export: &Value, field: &str) -> Value {
    json(
        &URL_SAFE_NO_PAD
            .decode(export[field].as_str().unwrap())
            .unwrap(),
    )
}

/// What the latest signed authority says, decoded from the export itself.
pub(super) struct Authority {
    pub(super) registry: Value,
    pub(super) keys: Value,
    pub(super) policy: Value,
}
impl Authority {
    pub(super) fn read(authority: &Path) -> Self {
        let export = json(&fs::read(authority).unwrap());
        assert_eq!(
            export["policy"]["state"], "present",
            "mixed restore cases keep owner policy present"
        );
        Self {
            registry: exported(&export, "registry"),
            keys: exported(&export, "authorizationKeys"),
            policy: json(
                &URL_SAFE_NO_PAD
                    .decode(export["policy"]["encoded"].as_str().unwrap())
                    .unwrap(),
            ),
        }
    }
}

/// The comparable authority facts of one source, for the rollback oracle.
struct Facts {
    installation_epoch: u64,
    /// Grant epoch and whether the pairing is revoked.
    devices: BTreeMap<Uuid, (u64, bool)>,
    /// Key epoch and whether the key is revoked, by pairing and subject key.
    keys: BTreeMap<(Uuid, String), (u64, bool)>,
    policy_revision: u64,
}
impl Facts {
    fn new(registry: &Value, keys: Option<&Value>, policy: Option<&Value>) -> Self {
        Self {
            installation_epoch: registry["restoreEpoch"].as_u64().unwrap_or(0),
            devices: registry["devices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|record| {
                    (
                        id(&record["device"]["id"]),
                        (
                            record["grant"]["revocationEpoch"].as_u64().unwrap(),
                            !record["device"]["revokedAt"].is_null(),
                        ),
                    )
                })
                .collect(),
            keys: keys
                .into_iter()
                .flat_map(|keys| keys["keys"].as_array().unwrap())
                .map(|key| {
                    (
                        (
                            id(&key["deviceId"]),
                            key["subjectKeyId"].as_str().unwrap().to_owned(),
                        ),
                        (
                            key["keyEpoch"].as_u64().unwrap(),
                            !key["revokedAt"].is_null(),
                        ),
                    )
                })
                .collect(),
            policy_revision: policy.map_or(0, |policy| policy["revision"].as_u64().unwrap()),
        }
    }
    fn image(files: &Files) -> Self {
        let keys = files.get("authorization-keys.json").map(|b| json(b));
        let policy = files.get("introduction-policy.json").map(|b| json(b));
        Self::new(
            &json(&files["devices.json"]),
            keys.as_ref(),
            policy.as_ref(),
        )
    }
    /// Whether installing `self` over `other` would undo something `other`
    /// already records: an epoch, a revocation or a policy revision.
    fn rolls_back(&self, other: &Self) -> bool {
        self.installation_epoch < other.installation_epoch
            || self.policy_revision < other.policy_revision
            || other.devices.iter().any(|(device, (epoch, revoked))| {
                self.devices
                    .get(device)
                    .is_some_and(|(next, still)| next < epoch || (*revoked && !*still))
            })
            || other.keys.iter().any(|(key, (epoch, revoked))| {
                self.keys
                    .get(key)
                    .is_some_and(|(next, still)| next < epoch || (*revoked && !*still))
            })
    }
}

pub(super) struct Sources<'a> {
    pub(super) snapshot: &'a Point,
    pub(super) destination: Option<&'a Point>,
    pub(super) authority: &'a Point,
}
impl Sources<'_> {
    pub(super) fn label(&self) -> String {
        format!(
            "snapshot={} destination={} authority={}",
            self.snapshot.name,
            self.destination.map_or("empty", |point| point.name),
            self.authority.name
        )
    }
    fn known(&self) -> Vec<(&'static str, Known)> {
        let mut known = vec![
            ("selected snapshot", Known::image(&self.snapshot.files)),
            ("latest authority", Known::export(&self.authority.authority)),
        ];
        if let Some(destination) = self.destination {
            known.push(("retained destination", Known::image(&destination.files)));
        }
        known
    }
    /// The export would undo authority recorded by the snapshot or destination.
    pub(super) fn rolls_back(&self) -> bool {
        let authority = Authority::read(&self.authority.authority);
        let incoming = Facts::new(
            &authority.registry,
            Some(&authority.keys),
            Some(&authority.policy),
        );
        incoming.rolls_back(&Facts::image(&self.snapshot.files))
            || self
                .destination
                .is_some_and(|point| incoming.rolls_back(&Facts::image(&point.files)))
    }
    /// One above the highest generation and epoch any source proves.
    pub(super) fn expected(&self) -> (u64, BTreeMap<Uuid, (u64, u64)>) {
        let known = self.known();
        let pairings: BTreeSet<Uuid> = known
            .iter()
            .flat_map(|(_, known)| known.pairings.iter().copied())
            .collect();
        let highest = |select: fn(&Known) -> &BTreeMap<Uuid, u64>, device: &Uuid| {
            known
                .iter()
                .filter_map(|(_, known)| select(known).get(device).copied())
                .max()
        };
        let targets = pairings
            .iter()
            .map(|device| {
                (
                    *device,
                    (
                        highest(|k| &k.generations, device).unwrap_or(0) + 1,
                        highest(|k| &k.epochs, device).expect("pairing without an epoch") + 1,
                    ),
                )
            })
            .collect();
        let epoch = known
            .iter()
            .map(|(_, known)| known.installation_epoch)
            .max()
            .unwrap()
            + 1;
        (epoch, targets)
    }
    /// Every routing seed any source ever held; none may be offered again.
    fn seeds(&self) -> BTreeSet<Vec<u8>> {
        let mut seeds = BTreeSet::new();
        for files in [
            Some(&self.snapshot.files),
            self.destination.map(|p| &p.files),
        ]
        .into_iter()
        .flatten()
        {
            for (name, bytes) in files {
                if !name.starts_with("transport-") || name == JOURNAL {
                    continue;
                }
                let stored = json(bytes);
                for generation in [&stored["committed"], &stored["pending"]] {
                    if !generation.is_null() {
                        seeds.insert(
                            serde_json::to_vec(&generation["material"]["routingSeed"]).unwrap(),
                        );
                    }
                }
            }
        }
        seeds
    }
}

fn targets(state: &State) -> BTreeMap<Uuid, (u64, u64)> {
    state
        .pairings
        .iter()
        .map(|p| (p.device_id, (p.generation, p.revocation_epoch)))
        .collect()
}

/// The three acceptance properties, checked on a completed installation: the
/// latest signed authority supersedes snapshot authority, no retained
/// generation or epoch floor is lowered, and every pairing stays unusable
/// until its own fresh handover.
pub(super) fn verify_installed(lineage: &Lineage, sources: &Sources, home: &Path, state: &State) {
    let label = sources.label();
    let (epoch, expected) = sources.expected();
    assert!(state.phase == Phase::AwaitingHandovers, "{label}");
    assert_eq!(state.installation_epoch, epoch, "{label}");
    assert_eq!(targets(state), expected, "{label}");
    assert!(
        state.pairings.iter().all(|p| !p.handover_complete),
        "{label}: a restore target started complete"
    );

    // Floors: strictly above everything each source proves, source by source.
    for (source, known) in sources.known() {
        assert!(
            state.installation_epoch > known.installation_epoch,
            "{label}: installation epoch not above the {source}"
        );
        for (device, generation) in &known.generations {
            assert!(
                expected[device].0 > *generation,
                "{label}: {device} generation not above the {source}"
            );
        }
        for (device, epoch) in &known.epochs {
            assert!(
                expected[device].1 > *epoch,
                "{label}: {device} epoch not above the {source}"
            );
        }
    }

    // Authority: exactly the signed export, with only the restore epochs
    // advanced. Nothing from the snapshot or the old destination survives.
    let authority = Authority::read(&sources.authority.authority);
    let mut registry = authority.registry.clone();
    registry["restoreEpoch"] = epoch.into();
    for record in registry["devices"].as_array_mut().unwrap() {
        let device = id(&record["device"]["id"]);
        record["grant"]["revocationEpoch"] = expected[&device].1.into();
    }
    let installed = read_files(home);
    assert_eq!(json(&installed["devices.json"]), registry, "{label}");
    assert_eq!(
        json(&installed["authorization-keys.json"]),
        authority.keys,
        "{label}"
    );
    assert_eq!(
        json(&installed["introduction-policy.json"]),
        authority.policy,
        "{label}"
    );
    assert_eq!(
        identity::load_existing_host_identity(home)
            .unwrap()
            .public_key_fingerprint,
        lineage.pin,
        "{label}"
    );

    // Routing: a bare floor for every known pairing, and no old secret.
    for (device, (generation, _)) in &expected {
        assert_eq!(
            json(&installed[&format!("transport-{device}.json")]),
            serde_json::json!({
                "version": 2,
                "minimumGeneration": generation - 1,
                "committed": null,
                "pending": null,
            }),
            "{label}: {device}"
        );
        assert_eq!(
            installed[&format!(".transport-v2-{device}.required")],
            b"coven.mobile.routing.v2\n",
            "{label}: {device}"
        );
    }

    // Admission: nothing is usable before its own fresh handover.
    let registry = DeviceRegistry::load(home).unwrap();
    for device in expected.keys() {
        assert!(
            check_application(home, &registry, *device).is_err(),
            "{label}: {device} usable before fresh handover"
        );
    }
    let seeds = sources.seeds();
    let mut completed = BTreeSet::new();
    for (device, (generation, epoch)) in &expected {
        let Some(record) = registry.authorization_record(*device).unwrap() else {
            // Journal-only: the floor is retained but the pairing is gone.
            assert!(
                prepare_direct(home, &registry, &lineage.host, *device).is_err(),
                "{label}: forgotten {device} offered a handover"
            );
            continue;
        };
        if record.device.revoked_at.is_some() {
            assert!(
                prepare_direct(home, &registry, &lineage.host, *device).is_err(),
                "{label}: revoked {device} offered a handover"
            );
            continue;
        }
        let fresh = prepare_direct(home, &registry, &lineage.host, *device)
            .unwrap_or_else(|e| panic!("{label}: {device}: {e:#}"));
        assert_eq!(
            (fresh.generation, fresh.revocation_epoch),
            (*generation, *epoch),
            "{label}: {device}"
        );
        assert!(
            !seeds.contains(&serde_json::to_vec(&fresh.routing_seed).unwrap()),
            "{label}: {device} was offered a retained seed"
        );
        assert!(
            check_application(home, &registry, *device).is_err(),
            "{label}: {device} usable on an unacknowledged offer"
        );
        let phone = tempfile::tempdir().unwrap();
        let acknowledgment = crate::mobile_memory::transport::storage::v2::device::adopt_direct(
            phone.path(),
            fresh,
            lineage.host.verifying_key(),
            *device,
            device_key(*device).verifying_key(),
        )
        .unwrap();
        acknowledge_direct(home, &registry, &lineage.host, *device, &acknowledgment).unwrap();
        check_application(home, &registry, *device)
            .unwrap_or_else(|e| panic!("{label}: {device}: {e:#}"));
        completed.insert(*device);
    }
    let journal = read(home).unwrap().unwrap();
    for target in &journal.pairings {
        assert_eq!(
            target.handover_complete,
            completed.contains(&target.device_id),
            "{label}: {}",
            target.device_id
        );
        if !target.handover_complete {
            assert!(
                check_application(home, &registry, target.device_id).is_err(),
                "{label}: {} usable without a handover",
                target.device_id
            );
        }
    }
}

/// A destination home holding the retained image, or nothing.
pub(super) fn destination(point: Option<&Point>) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    if let Some(point) = point {
        write_files(home.path(), &point.files);
    }
    home
}
pub(super) fn selected(point: &Point) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    write_files(home.path(), &point.files);
    home
}

const REFUSALS: [&str; 4] = ["predates", "conflicts with", "roll back", "would remove"];

type Triple = (usize, Option<usize>, usize);

/// Each triple either installs with all three acceptance properties or is
/// refused as older authority before the destination changes.
fn exercise(lineage: &Lineage, triples: &[Triple]) -> (usize, usize) {
    let snapshots: Vec<_> = lineage.points.iter().map(selected).collect();
    let snapshots_before: Vec<_> = snapshots.iter().map(|s| read_files(s.path())).collect();
    let mut outcomes = (0, 0);
    for &(snapshot, retained, authority) in triples {
        let sources = Sources {
            snapshot: &lineage.points[snapshot],
            destination: retained.map(|index| &lineage.points[index]),
            authority: &lineage.points[authority],
        };
        let label = sources.label();
        let home = destination(sources.destination);
        let before = sources
            .destination
            .map(|point| authority_files(&point.files))
            .unwrap_or_default();
        let result = install_from_snapshot(
            home.path(),
            snapshots[snapshot].path(),
            &sources.authority.authority,
            lineage.pin,
            Uuid::new_v4(),
        );
        if authority >= snapshot && retained.is_none_or(|index| authority >= index) {
            assert!(
                !sources.rolls_back(),
                "{label}: the oracle calls the newest authority a rollback"
            );
        }
        match result {
            Ok(state) => {
                assert!(
                    !sources.rolls_back(),
                    "{label}: accepted an authority that undoes recorded state"
                );
                verify_installed(lineage, &sources, home.path(), &state);
                outcomes.0 += 1;
            }
            Err(error) => {
                let error = format!("{error:#}");
                assert!(
                    sources.rolls_back(),
                    "{label}: refused an authority that undoes nothing: {error}"
                );
                assert!(
                    REFUSALS.iter().any(|class| error.contains(class)),
                    "{label}: not an authority-age refusal: {error}"
                );
                let after = if home.path().join("mobile").exists() {
                    authority_files(&read_files(home.path()))
                } else {
                    Files::new()
                };
                assert!(
                    after == before,
                    "{label}: a refused restore changed the destination"
                );
                outcomes.1 += 1;
            }
        }
        assert!(
            read_files(snapshots[snapshot].path()) == snapshots_before[snapshot],
            "{label}: the selected snapshot changed"
        );
    }
    outcomes
}

/// Every pair of sources meets at least once: 72 of the 576 triples.
fn covering() -> Vec<Triple> {
    let points = POINTS.len();
    let triples: Vec<Triple> = (0..points)
        .flat_map(|snapshot| {
            (0..=points).map(move |retained| {
                (
                    snapshot,
                    retained.checked_sub(1),
                    (snapshot + retained) % points,
                )
            })
        })
        .collect();
    let pairs = |select: fn(&Triple) -> (Option<usize>, Option<usize>)| {
        triples.iter().map(select).collect::<BTreeSet<_>>().len()
    };
    assert_eq!(pairs(|t| (Some(t.0), t.1)), points * (points + 1));
    assert_eq!(pairs(|t| (Some(t.0), Some(t.2))), points * points);
    assert_eq!(pairs(|t| (t.1, Some(t.2))), (points + 1) * points);
    triples
}

/// The disagreements the native interruption cases start from.
const NAMED: [Triple; 2] = [
    (BASE, Some(ACKNOWLEDGED), TIGHTENED),
    (RESTORED, Some(HANDED_OVER), RESTORED_AGAIN),
];

#[test]
fn mixed_sources_named_disagreements_install_latest_authority_above_every_floor() {
    let lineage = Lineage::build();
    assert_eq!(exercise(&lineage, &NAMED), (2, 0));
}

/// Each installation repeats every durability barrier. Where those are full
/// device flushes the larger matrices take minutes, so this runs by default
/// only on Linux; run it elsewhere with `--ignored`.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "minutes of durability barriers off Linux; run with --ignored"
)]
fn mixed_sources_install_latest_authority_above_every_floor_or_refuse_unchanged() {
    let lineage = Lineage::build();
    let mut triples = covering();
    for named in NAMED {
        if !triples.contains(&named) {
            triples.push(named);
        }
    }
    let (installed, refused) = exercise(&lineage, &triples);
    println!("mixed-restore covering installed={installed} refused={refused}");
    assert_eq!((installed, refused), (40, 34));
}

/// The complete cube. Each installation repeats every durability barrier, so
/// this takes minutes where directory syncs are slow; run it explicitly with
/// `--ignored` when restore precedence changes.
#[test]
#[ignore = "exhaustive 576-triple cube; minutes of durability barriers"]
fn mixed_sources_exhaustive_cube_installs_or_refuses_every_triple() {
    let lineage = Lineage::build();
    let points = POINTS.len();
    let triples: Vec<Triple> = (0..points)
        .flat_map(|authority| {
            (0..points).flat_map(move |snapshot| {
                (0..=points).map(move |retained| (snapshot, retained.checked_sub(1), authority))
            })
        })
        .collect();
    assert_eq!(triples.len(), points * (points + 1) * points);
    let (installed, refused) = exercise(&lineage, &triples);
    println!("mixed-restore cube installed={installed} refused={refused}");
    assert_eq!((installed, refused), (266, 310));
}

/// Forgetting pairings leaves no revocation record. An authentic export that
/// predates the forget is therefore not detectably older: it brings the
/// forgotten pairings back, but only behind the restore fence.
#[test]
fn export_predating_a_forget_returns_pairings_only_behind_fresh_handover() {
    let lineage = Lineage::build();
    let sources = Sources {
        snapshot: &lineage.points[RESTORED],
        destination: Some(&lineage.points[FORGOTTEN]),
        authority: &lineage.points[HANDED_OVER],
    };
    let retained = Known::image(&lineage.points[FORGOTTEN].files);
    let home = destination(sources.destination);
    let before = DeviceRegistry::load(home.path()).unwrap();
    assert!(before.authorization_record(FIRST).unwrap().is_none());
    check_application(home.path(), &before, LATE).unwrap();
    let source = selected(sources.snapshot);
    let state = install_from_snapshot(
        home.path(),
        source.path(),
        &sources.authority.authority,
        lineage.pin,
        Uuid::new_v4(),
    )
    .unwrap();
    let installed = targets(&state);
    let registry = DeviceRegistry::load(home.path()).unwrap();
    // The late pairing is unknown to the export: it leaves the registry and
    // keeps only its floors, so it cannot be used and cannot be re-offered.
    assert!(registry.authorization_record(LATE).unwrap().is_none());
    assert!(installed[&LATE].0 > retained.generations[&LATE]);
    // The revocation the export does carry is still in force.
    let second = registry.authorization_record(SECOND).unwrap().unwrap();
    assert!(second.device.revoked_at.is_some());
    // The returned pairings start above the floors the destination retained
    // for them while they were forgotten, and need their own fresh handover.
    for device in [FIRST, THIRD] {
        assert!(registry.authorization_record(device).unwrap().is_some());
        assert!(installed[&device].0 > retained.generations[&device]);
        assert!(installed[&device].1 > retained.epochs[&device]);
    }
    verify_installed(&lineage, &sources, home.path(), &state);
}
