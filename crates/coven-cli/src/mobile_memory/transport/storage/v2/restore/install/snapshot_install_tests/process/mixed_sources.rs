//! Native interruption of mixed-source restores into a retained destination.
//! The selected snapshot, the destination and the signed authority are three
//! different points of one lineage, each with owner policy present. Every
//! observed installer checkpoint is cut once by SIGKILL and once by a returned
//! error. Process death is not a power-loss simulation: `retained` models
//! what a cut before namespace durability may leave, and `contract` holds
//! what every interruption of either kind must satisfy before its retry.
//!
//! Each cut starts a child, retries the installation and completes fresh
//! handovers, so the matrices take several minutes of test-thread time.
//! The required Linux test job has no room for that, so they are ignored by
//! default: run them with `--ignored` when the installer, its checkpoints or
//! restore precedence change.
use super::super::mixed::{
    authority_files, destination, json, read_files, selected, verify_installed, Authority, Known,
    Lineage, Sources, ACKNOWLEDGED, BASE, FIRST, HANDED_OVER, LATE, RESTORED, RESTORED_AGAIN,
    SECOND, THIRD, TIGHTENED,
};
use super::*;
use contract::{Contract, Fence};

mod contract;
mod retained;

fn policy_revision(files: &BTreeMap<String, Vec<u8>>) -> u64 {
    json(&files["introduction-policy.json"])["revision"]
        .as_u64()
        .unwrap()
}

/// Cuts every checkpoint of one mixed restore and returns the checkpoint count.
fn exercise(lineage: &Lineage, sources: &Sources) -> usize {
    let contract = Contract::new(lineage, sources);
    let label = contract.label();
    let authority = contract.authority();

    let normal = destination(sources.destination);
    let trace = run_sources(
        contract.source(),
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
    for device in contract.devices() {
        assert!(
            trace
                .iter()
                .any(|line| line.contains(&format!(" transport-{device}.json RetainedHomeSynced"))),
            "{label}: {device} routing floor is not observed"
        );
    }

    let mut fences = BTreeMap::new();
    for kill in [true, false] {
        for cut in 1..=trace.len() {
            let home = destination(sources.destination);
            let operation = Uuid::new_v4();
            let observed = run_sources(
                contract.source(),
                authority,
                lineage.pin,
                home.path(),
                operation,
                cut,
                kill,
            );
            assert_eq!(observed, trace[..cut], "{label}: write ordering changed");
            let at = format!("cut {cut} kill={kill}");
            let fence = contract.interrupted(home.path(), operation, &at);
            contract.retry(home.path(), operation, &at);
            *fences.entry(fence).or_insert(0) += 1;
            println!(
                "restore-process mixed {label} kill={kill} cut={cut} {} fence={fence:?}",
                trace[cut - 1]
            );
        }
    }
    // Both sides of the fence and the completed journal are all interrupted.
    for fence in [Fence::Absent, Fence::Installing, Fence::AwaitingHandovers] {
        assert!(
            fences.get(&fence).copied().unwrap_or(0) >= 2,
            "{label}: no cut left the destination {fence:?}: {fences:?}"
        );
    }
    println!("restore-process mixed {label} fences={fences:?}");
    trace.len()
}

/// Three pairings into an empty destination: two usable and one revoked in
/// the latest authority, none of them known to the destination.
#[test]
#[ignore = "native interruption matrix; minutes of child processes and durability barriers"]
fn mixed_restore_process_interruptions_install_three_pairings_into_an_empty_destination() {
    let lineage = Lineage::build();
    let sources = retained::empty_destination(&lineage);
    let authority = Authority::read(&sources.authority.authority);
    let revoked: Vec<_> = authority.registry["devices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| !record["device"]["revokedAt"].is_null())
        .map(|record| record["device"]["id"].clone())
        .collect();
    assert_eq!(revoked, [serde_json::json!(SECOND.to_string())]);
    let (epoch, expected) = sources.expected();
    assert_eq!(epoch, 1);
    assert_eq!(expected[&FIRST], (3, 2));
    assert_eq!(expected[&SECOND], (1, 2));
    assert_eq!(expected[&THIRD], (1, 1));
    let checkpoints = exercise(&lineage, &sources);
    println!("restore-process mixed empty-destination checkpoints={checkpoints}");
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
