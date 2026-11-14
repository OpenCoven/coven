//! Explicit Unix retained-namespace images derived from the actual write trace.
//! This models legal outcomes; it does not simulate a physical storage device.
use super::*;
use crate::mobile_memory::config::MobileWritePhase as Phase;
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

type Files = BTreeMap<String, Vec<u8>>;
struct Image {
    checkpoint: String,
    files: Files,
}
fn copy_files(home: &Path, files: &Files) {
    let directory = ensure_private_mobile_dir(home).unwrap();
    for (name, bytes) in files {
        atomic_replace_private(&directory.join(name), bytes).unwrap();
    }
}
fn visible(home: &Path, names: &BTreeSet<String>) -> Files {
    names
        .iter()
        .filter_map(|name| {
            let path = home.join("mobile").join(name);
            match fs::read(&path) {
                Ok(bytes) => Some((name.clone(), bytes)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => panic!("reading modeled namespace {}: {e}", path.display()),
            }
        })
        .collect()
}
struct ClearObserver;
impl Drop for ClearObserver {
    fn drop(&mut self) {
        config::set_write_observer(Box::new(|_, _| Ok(())));
    }
}
#[test]
fn rich_snapshot_restore_recovers_all_trace_derived_unix_retained_images() {
    let c = rich_case();
    let source_before = inventory(c.snapshot.path());
    let initial: Files = fs::read_dir(c.snapshot.path().join("mobile"))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    let normal = tempfile::tempdir().unwrap();
    copy_files(normal.path(), &initial);
    let operation = Uuid::new_v4();
    let images = Rc::new(RefCell::new(Vec::new()));
    let capture = Rc::clone(&images);
    let home = normal.path().to_owned();
    let mut names: BTreeSet<_> = initial.keys().cloned().collect();
    let mut stable = initial.clone();
    let mut ordinal = 0;
    let mut staged_creates = BTreeSet::new();
    config::set_write_observer(Box::new(move |path, phase| {
        assert_eq!(path.parent().unwrap(), home.join("mobile"));
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        names.insert(name.clone());
        ordinal += 1;
        let current = visible(&home, &names);
        let checkpoint = format!("{ordinal} {name} {phase:?}");
        let mut variants = Vec::new();
        match phase {
            // On Unix the generic barrier and the checked retained-parent
            // barrier flush this whole directory, including earlier renames.
            Phase::DirectorySynced | Phase::RetainedParentSynced | Phase::RetainedHomeSynced => {
                stable = current.clone();
                variants.push(("durable", current));
            }
            Phase::Renamed => {
                if staged_creates.remove(&name) {
                    assert_eq!(
                        current[&name],
                        crate::mobile_memory::transport::storage::v2::PROTOCOL_FLOOR_BYTES,
                        "published floor is not the complete synced image at {checkpoint}"
                    );
                    variants.push(("published-create", current.clone()));
                }
                variants.push(("old-name", stable.clone()));
                variants.push(("new-name", current));
            }
            Phase::TempCreated | Phase::Written | Phase::FileSynced
                if !stable.contains_key(&name)
                    && name.starts_with(".transport-v2-")
                    && name.ends_with(".required")
                    && (phase == Phase::TempCreated || staged_creates.contains(&name)) =>
            {
                // New floors are written only to private staging files. Even
                // after file sync, their canonical name must remain absent.
                assert!(
                    !current.contains_key(&name),
                    "unpublished floor became authority at {checkpoint}"
                );
                staged_creates.insert(name.clone());
                let staged: Vec<_> = fs::read_dir(home.join("mobile"))
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| {
                        let name = path.file_name().unwrap().to_string_lossy();
                        name.starts_with(".mobile-state-") && name.ends_with(".tmp")
                    })
                    .collect();
                assert_eq!(
                    staged.len(),
                    1,
                    "missing unique staging file at {checkpoint}"
                );
                let bytes = fs::read(&staged[0]).unwrap();
                let kind = match phase {
                    Phase::TempCreated => {
                        assert!(
                            bytes.is_empty(),
                            "staging was already written at {checkpoint}"
                        );
                        "staged-empty-canonical-absent"
                    }
                    Phase::Written => {
                        assert_eq!(
                            bytes,
                            crate::mobile_memory::transport::storage::v2::PROTOCOL_FLOOR_BYTES
                        );
                        "staged-written-canonical-absent"
                    }
                    Phase::FileSynced => {
                        assert_eq!(
                            bytes,
                            crate::mobile_memory::transport::storage::v2::PROTOCOL_FLOOR_BYTES
                        );
                        "staged-synced-canonical-absent"
                    }
                    _ => unreachable!(),
                };
                // Private staging bytes are not part of the authority image.
                variants.push((kind, current));
            }
            Phase::TempCreated | Phase::Written | Phase::FileSynced => {
                // A private temporary file is never authority. Before rename,
                // the previous durable canonical image remains authoritative.
                variants.push(("previous-canonical", stable.clone()));
            }
        }
        for (kind, files) in variants {
            capture.borrow_mut().push(Image {
                checkpoint: format!("{checkpoint} {kind}"),
                files,
            });
        }
        Ok(())
    }));
    let observer = ClearObserver;
    install_from_snapshot(
        normal.path(),
        c.snapshot.path(),
        &c.authority,
        c.pin,
        operation,
    )
    .unwrap();
    drop(observer);
    let images = Rc::try_unwrap(images).ok().unwrap().into_inner();
    assert!(images.iter().any(|i| i
        .checkpoint
        .contains("introduction-policy.json Renamed old-name")));
    for kind in [
        "staged-empty-canonical-absent",
        "staged-written-canonical-absent",
        "staged-synced-canonical-absent",
        "published-create",
    ] {
        assert!(
            images.iter().any(|i| i.checkpoint.ends_with(kind)),
            "missing {kind} checkpoint"
        );
    }
    assert!(images.iter().any(|i| i
        .checkpoint
        .contains("transport-restore.json Renamed new-name")));
    let mut counts = BTreeMap::new();
    for image in images {
        let destination = tempfile::tempdir().unwrap();
        copy_files(destination.path(), &image.files);
        let registry = DeviceRegistry::load(destination.path()).unwrap();
        if image.files.contains_key(JOURNAL) || image.files.contains_key(MARKER) {
            assert!(
                check_application(destination.path(), &registry, c.f.id).is_err(),
                "{} admitted restored authority before handover",
                image.checkpoint
            );
        } else {
            // Pre-transaction images retain the unchanged old installation.
            // No restore artifact or selected-snapshot replacement is durable.
            assert!(image.files == initial, "pre-fence image changed authority");
        }
        let state = install_from_snapshot(
            destination.path(),
            c.snapshot.path(),
            &c.authority,
            c.pin,
            operation,
        )
        .unwrap_or_else(|e| panic!("{}: {e:#}", image.checkpoint));
        assert_eq!(state.installation_epoch, 1, "{}", image.checkpoint);
        assert_eq!(state.pairings.len(), 2);
        for target in &state.pairings {
            assert_eq!(target.generation, 1, "{}", image.checkpoint);
            assert_eq!(
                target.revocation_epoch,
                if target.device_id == c.f.id { 1 } else { 2 }
            );
            assert!(!target.handover_complete);
        }
        check_rich_authority(&c, destination.path());
        assert!(check_application(destination.path(), &registry, c.f.id).is_err());
        let fresh = prepare_direct(destination.path(), &registry, &c.f.host, c.f.id).unwrap();
        let device_home = tempfile::tempdir().unwrap();
        let ack = crate::mobile_memory::transport::storage::v2::device::adopt_direct(
            device_home.path(),
            fresh,
            c.f.host.verifying_key(),
            c.f.id,
            c.f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(destination.path(), &registry, &c.f.host, c.f.id, &ack).unwrap();
        check_application(destination.path(), &registry, c.f.id).unwrap();
        check_rich_authority(&c, destination.path());
        assert_eq!(inventory(c.snapshot.path()), source_before);
        let kind = image
            .checkpoint
            .split_whitespace()
            .last()
            .unwrap()
            .to_owned();
        *counts.entry(kind).or_insert(0) += 1;
        println!("restore-retained-image {} recovered", image.checkpoint);
    }
    println!("restore-retained-image counts={counts:?}");
}
