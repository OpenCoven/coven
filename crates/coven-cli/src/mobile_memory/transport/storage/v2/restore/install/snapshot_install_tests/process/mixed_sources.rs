//! Native interruption of mixed-source restores into a retained destination.
//! The selected snapshot, the destination and the signed authority are three
//! different points of one lineage, each with owner policy present. Every
//! observed installer checkpoint is cut once by SIGKILL and once by a returned
//! error. Process death is not a power-loss simulation.
//!
//! Each cut starts a child, retries the installation and completes fresh
//! handovers, so the two matrices take several minutes of test-thread time.
//! The required Linux test job has no room for that, so they are ignored by
//! default: run them with `--ignored` when the installer, its checkpoints or
//! restore precedence change.
use super::super::mixed::{
    authority_files, destination, json, read_files, selected, verify_installed, Authority, Known,
    Lineage, Sources, ACKNOWLEDGED, BASE, FIRST, HANDED_OVER, LATE, RESTORED, RESTORED_AGAIN,
    SECOND, THIRD, TIGHTENED,
};
use super::*;

fn policy_revision(files: &BTreeMap<String, Vec<u8>>) -> u64 {
    json(&files["introduction-policy.json"])["revision"]
        .as_u64()
        .unwrap()
}

/// Cuts every checkpoint of one mixed restore and returns the checkpoint count.
fn exercise(lineage: &Lineage, sources: &Sources) -> usize {
    let label = sources.label();
    let retained = sources.destination.expect("a retained destination");
    let original = authority_files(&retained.files);
    let floors = Known::image(&retained.files);
    let source = selected(sources.snapshot);
    let source_before = read_files(source.path());
    let authority = &sources.authority.authority;
    let authority_before = fs::read(authority).unwrap();
    let (_, expected) = sources.expected();

    let normal = destination(sources.destination);
    let trace = run_sources(
        source.path(),
        authority,
        lineage.pin,
        normal.path(),
        Uuid::new_v4(),
        0,
        false,
    );
    for component in [
        "transport-restore.json",
        "devices.json",
        "authorization-keys.json",
        "introduction-policy.json",
    ] {
        assert!(
            trace
                .iter()
                .any(|line| line.contains(&format!(" {component} RetainedHomeSynced"))),
            "{label}: {component} barriers are not observed: {trace:?}"
        );
    }
    for device in expected.keys() {
        assert!(
            trace
                .iter()
                .any(|line| line.contains(&format!(" transport-{device}.json RetainedHomeSynced"))),
            "{label}: {device} routing floor is not observed"
        );
    }

    for kill in [true, false] {
        for cut in 1..=trace.len() {
            let home = destination(sources.destination);
            let operation = Uuid::new_v4();
            let observed = run_sources(
                source.path(),
                authority,
                lineage.pin,
                home.path(),
                operation,
                cut,
                kill,
            );
            assert_eq!(observed, trace[..cut], "{label}: write ordering changed");

            let interrupted = authority_files(&read_files(home.path()));
            let fenced = interrupted
                .get(JOURNAL)
                .is_some_and(|journal| json(journal)["operationId"] == operation.to_string());
            if fenced {
                // This operation's journal is durable: nothing is usable, and
                // nothing the destination retained has moved down.
                let registry = DeviceRegistry::load(home.path()).unwrap();
                for device in expected.keys() {
                    assert!(
                        check_application(home.path(), &registry, *device).is_err(),
                        "{label} cut {cut}: {device} usable under the fence"
                    );
                }
                let now = Known::image(&interrupted);
                assert!(now.installation_epoch >= floors.installation_epoch);
                for (device, generation) in &floors.generations {
                    assert!(
                        now.generations[device] >= *generation,
                        "{label} cut {cut}: {device} generation floor lowered"
                    );
                }
                for (device, epoch) in &floors.epochs {
                    assert!(
                        now.epochs[device] >= *epoch,
                        "{label} cut {cut}: {device} epoch floor lowered"
                    );
                }
            } else {
                // Before the fence the retained installation is untouched.
                for (name, bytes) in &original {
                    assert!(
                        interrupted.get(name) == Some(bytes),
                        "{label} cut {cut}: {name} changed before the fence"
                    );
                }
            }

            let state = install_from_snapshot(
                home.path(),
                source.path(),
                authority,
                lineage.pin,
                operation,
            )
            .unwrap_or_else(|e| panic!("{label} cut {cut} kill={kill}: {e:#}"));
            verify_installed(lineage, sources, home.path(), &state);
            assert!(
                read_files(source.path()) == source_before,
                "{label} cut {cut}: the selected snapshot changed"
            );
            assert_eq!(fs::read(authority).unwrap(), authority_before);
            println!(
                "restore-process mixed {label} kill={kill} cut={cut} {}",
                trace[cut - 1]
            );
        }
    }
    trace.len()
}

#[test]
#[ignore = "native interruption matrix; minutes of child processes and durability barriers"]
fn mixed_restore_process_interruptions_keep_burned_and_pending_floors_above_older_authority() {
    let lineage = Lineage::build();
    let sources = Sources {
        snapshot: &lineage.points[BASE],
        destination: Some(&lineage.points[ACKNOWLEDGED]),
        authority: &lineage.points[TIGHTENED],
    };
    let snapshot = Known::image(&sources.snapshot.files);
    let retained = Known::image(&lineage.points[ACKNOWLEDGED].files);
    let export = Known::export(&sources.authority.authority);
    let authority = Authority::read(&sources.authority.authority);
    // Registry, keys and policy: the export is ahead of the snapshot.
    assert_eq!(policy_revision(&sources.snapshot.files), 1);
    assert_eq!(authority.policy["revision"], 2);
    assert!(json(&sources.snapshot.files["authorization-keys.json"]) != authority.keys);
    assert!(json(&sources.snapshot.files["devices.json"]) != authority.registry);
    assert!((snapshot.epochs[&FIRST], export.epochs[&FIRST]) == (0, 1));
    // Routing: the export captured the first pairing with generation 1 burned
    // and generation 2 pending; the destination later offered the third
    // pairing a generation the export never saw.
    let burned = json(&lineage.points[TIGHTENED].files[&format!("transport-{FIRST}.json")]);
    assert_eq!(burned["minimumGeneration"], 1);
    assert_eq!(burned["pending"]["generation"], 2);
    assert!(burned["committed"].is_null());
    assert_eq!(snapshot.generations[&FIRST], 0);
    assert_eq!(export.generations[&FIRST], 2);
    assert_eq!(
        (export.generations[&THIRD], retained.generations[&THIRD]),
        (0, 1)
    );
    let (epoch, expected) = sources.expected();
    assert_eq!(epoch, 1);
    assert_eq!(expected[&FIRST], (3, 2));
    assert_eq!(expected[&SECOND], (1, 2));
    assert_eq!(expected[&THIRD], (2, 1));
    let checkpoints = exercise(&lineage, &sources);
    println!("restore-process mixed burned-pending checkpoints={checkpoints}");
}

#[test]
#[ignore = "native interruption matrix; minutes of child processes and durability barriers"]
fn mixed_restore_process_interruptions_keep_journal_only_floors_across_three_journals() {
    let lineage = Lineage::build();
    let sources = Sources {
        snapshot: &lineage.points[RESTORED],
        destination: Some(&lineage.points[HANDED_OVER]),
        authority: &lineage.points[RESTORED_AGAIN],
    };
    let snapshot = Known::image(&sources.snapshot.files);
    let retained = Known::image(&lineage.points[HANDED_OVER].files);
    let export = Known::export(&sources.authority.authority);
    let authority = Authority::read(&sources.authority.authority);
    // Three different policy revisions, and three different restore journals.
    assert_eq!(policy_revision(&sources.snapshot.files), 2);
    assert_eq!(policy_revision(&lineage.points[HANDED_OVER].files), 3);
    assert_eq!(authority.policy["revision"], 4);
    assert_eq!(
        (
            snapshot.installation_epoch,
            retained.installation_epoch,
            export.installation_epoch
        ),
        (1, 1, 2)
    );
    // The export's registry holds only the late pairing: the first three are
    // journal-only there, with floors the second restore advanced.
    let registered: Vec<_> = authority.registry["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["device"]["id"].clone())
        .collect();
    assert_eq!(registered, [serde_json::json!(LATE.to_string())]);
    for device in [FIRST, SECOND, THIRD] {
        assert!(export.generations[&device] > retained.generations[&device]);
        assert!(export.epochs[&device] > retained.epochs[&device]);
        assert!(retained.generations[&device] >= snapshot.generations[&device]);
    }
    // The destination completed one fresh handover and left another offered.
    let journal = json(&lineage.points[HANDED_OVER].files[JOURNAL]);
    let complete: Vec<_> = journal["pairings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|target| target["handoverComplete"] == true)
        .map(|target| target["deviceId"].clone())
        .collect();
    assert_eq!(complete, [serde_json::json!(FIRST.to_string())]);
    let offered = json(&lineage.points[HANDED_OVER].files[&format!("transport-{THIRD}.json")]);
    assert!(offered["committed"].is_null() && !offered["pending"].is_null());
    let (epoch, expected) = sources.expected();
    assert_eq!(epoch, 3);
    assert_eq!(expected.len(), 4);
    let checkpoints = exercise(&lineage, &sources);
    println!("restore-process mixed journal-only checkpoints={checkpoints}");
}
