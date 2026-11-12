//! Explicit Unix retained-namespace images for mixed restores into a retained
//! destination (#1680). A killed child keeps the page cache, so it never shows
//! what a cut before namespace durability may leave behind. These images are
//! reconstructed from the installer's own write trace instead: every observed
//! checkpoint yields each namespace a Unix filesystem may legally retain there.
//! This models legal outcomes; it does not simulate a physical storage device.
//!
//! The model is derived from what the installer does, not from which writer a
//! component uses. The installer never has more than one canonical name
//! pending between two directory barriers, and the capture asserts that, so
//! "old namespace or new namespace" is the complete set for a rename. A
//! component created directly under its canonical name additionally yields
//! its absent, empty, partial and full contents until its file is synced.
//!
//! Each full matrix repeats every durability barrier once per image, so it
//! takes minutes and is ignored by default like the native matrices beside it.
use super::super::super::mixed::{write_files, Files};
use super::contract::{canonical, fence, image, is_staging};
use super::*;
use crate::mobile_memory::config::MobileWritePhase as Write;
use std::{cell::RefCell, rc::Rc};

struct Image {
    checkpoint: String,
    kind: &'static str,
    files: Files,
}

struct ClearObserver;
impl Drop for ClearObserver {
    fn drop(&mut self) {
        config::set_write_observer(Box::new(|_, _| Ok(())));
    }
}

/// Runs one uninterrupted installation and derives the retained images.
fn capture(contract: &Contract, sources: &Sources, pin: [u8; 32], operation: Uuid) -> Vec<Image> {
    let normal = destination(sources.destination);
    let home = normal.path().to_owned();
    let mut stable = image(&home);
    let images = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&images);
    let mut ordinal = 0;
    config::set_write_observer(Box::new(move |path, phase| {
        assert_eq!(path.parent().unwrap(), home.join("mobile"));
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        ordinal += 1;
        let checkpoint = format!("{ordinal} {name} {phase:?}");
        let visible = authority_files(&read_files(&home));
        let current = canonical(&visible);
        let pending: Vec<&String> = stable
            .keys()
            .chain(current.keys())
            .filter(|name| stable.get(*name) != current.get(*name))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(
            pending.iter().all(|changed| **changed == name) && pending.len() <= 1,
            "{checkpoint}: canonical names pending outside this write: {pending:?}"
        );
        let push = |kind: &'static str, files: Files| {
            sink.borrow_mut().push(Image {
                checkpoint: checkpoint.clone(),
                kind,
                files,
            });
        };
        match phase {
            // On Unix each of these barriers flushes the whole directory,
            // including every earlier rename and creation.
            Write::DirectorySynced | Write::RetainedParentSynced | Write::RetainedHomeSynced => {
                stable = current.clone();
                push("durable", current);
            }
            Write::Renamed if pending.is_empty() => push("identical-name", current),
            Write::Renamed => {
                push("old-name", stable.clone());
                push("new-name", current);
            }
            Write::TempCreated | Write::Written | Write::FileSynced if pending.is_empty() => {
                // Only a private staging file exists. It is not authority, but
                // it may be retained beside the previous canonical namespace.
                let mut files = stable.clone();
                files.extend(
                    visible
                        .iter()
                        .filter(|(name, _)| is_staging(name))
                        .map(|(name, bytes)| (name.clone(), bytes.clone())),
                );
                push("previous-canonical", files);
            }
            Write::TempCreated | Write::Written | Write::FileSynced => {
                // Created directly under its canonical name: before the file
                // is synced the retained name guarantees none of its bytes.
                assert!(
                    !stable.contains_key(&name),
                    "{checkpoint}: an existing canonical file was written in place"
                );
                let bytes = current[&name].clone();
                push("absent-create", stable.clone());
                match phase {
                    Write::TempCreated => {
                        assert!(bytes.is_empty(), "{checkpoint}: written before creation");
                        push("empty-create", current);
                    }
                    Write::Written => {
                        let mut empty = current.clone();
                        empty.insert(name.clone(), Vec::new());
                        push("empty-create", empty);
                        let mut partial = current.clone();
                        partial.insert(name.clone(), bytes[..bytes.len() / 2].to_vec());
                        push("partial-create", partial);
                        push("full-create", current);
                    }
                    _ => push("synced-create", current),
                }
            }
        }
        Ok(())
    }));
    let observer = ClearObserver;
    let result = install_from_snapshot(
        normal.path(),
        contract.source(),
        contract.authority(),
        pin,
        operation,
    );
    drop(observer);
    result.unwrap();
    Rc::try_unwrap(images).ok().unwrap().into_inner()
}

/// Recovers the selected images and returns how many were taken per kind and
/// fence. `every` selects all images; otherwise the first of each class.
fn exercise(lineage: &Lineage, sources: &Sources, every: bool) -> BTreeMap<String, usize> {
    let contract = Contract::new(lineage, sources);
    let label = contract.label();
    let operation = Uuid::new_v4();
    let images = capture(&contract, sources, lineage.pin, operation);
    let checkpoints: BTreeSet<&String> = images.iter().map(|image| &image.checkpoint).collect();
    println!(
        "restore-retained-image mixed {label} checkpoints={} images={}",
        checkpoints.len(),
        images.len()
    );
    let mut counts = BTreeMap::new();
    for image in &images {
        let class = format!(
            "{}/{:?}",
            image.kind,
            fence(&canonical(&image.files), operation)
        );
        let seen = counts.entry(class.clone()).or_insert(0);
        if !every && *seen > 0 {
            continue;
        }
        *seen += 1;
        let home = tempfile::tempdir().unwrap();
        write_files(home.path(), &image.files);
        let at = format!("image {} {}", image.checkpoint, image.kind);
        let observed = contract.interrupted(home.path(), operation, &at);
        contract.retry(home.path(), operation, &at);
        println!(
            "restore-retained-image mixed {label} {} {} fence={observed:?} recovered",
            image.checkpoint, image.kind
        );
    }
    println!("restore-retained-image mixed {label} classes={counts:?}");
    // Both sides of the fence, a pending rename and a completed journal. The
    // journal is the first canonical write, so no barrier precedes the fence.
    for class in [
        "previous-canonical/Absent",
        "old-name/Absent",
        "new-name/Installing",
        "old-name/Installing",
        "durable/Installing",
        "new-name/AwaitingHandovers",
        "durable/AwaitingHandovers",
    ] {
        assert!(
            counts.contains_key(class),
            "{label}: no retained image of class {class}: {counts:?}"
        );
    }
    counts
}

/// A second interruption. One image of every class is retried, and that retry
/// is itself cut by a returned error at each of its own checkpoints. The
/// twice-interrupted destination must satisfy the same contract before the
/// final retry. Returns the first images taken and the second cuts made.
fn exercise_twice(lineage: &Lineage, sources: &Sources) -> (usize, usize) {
    let contract = Contract::new(lineage, sources);
    let label = contract.label();
    let operation = Uuid::new_v4();
    let images = capture(&contract, sources, lineage.pin, operation);
    let mut classes = BTreeSet::new();
    let mut cuts = 0;
    for image in &images {
        let class = format!(
            "{}/{:?}",
            image.kind,
            fence(&canonical(&image.files), operation)
        );
        if !classes.insert(class.clone()) {
            continue;
        }
        // The retry's own write sequence, which differs with the image.
        let counted = Rc::new(RefCell::new(0));
        let counter = Rc::clone(&counted);
        let home = tempfile::tempdir().unwrap();
        write_files(home.path(), &image.files);
        config::set_write_observer(Box::new(move |_, _| {
            *counter.borrow_mut() += 1;
            Ok(())
        }));
        let observer = ClearObserver;
        let result = contract.attempt(home.path(), operation);
        drop(observer);
        result.unwrap_or_else(|e| panic!("{label} {} {}: {e:#}", image.checkpoint, image.kind));
        let checkpoints = *counted.borrow();
        assert!(checkpoints > 0, "{label}: a retry observed no write");

        for cut in 1..=checkpoints {
            let home = tempfile::tempdir().unwrap();
            write_files(home.path(), &image.files);
            let mut ordinal = 0;
            config::set_write_observer(Box::new(move |_, _| {
                ordinal += 1;
                anyhow::ensure!(ordinal != cut, "injected second restore interruption");
                Ok(())
            }));
            let observer = ClearObserver;
            let result = contract.attempt(home.path(), operation);
            drop(observer);
            let error = result
                .map(|_| ())
                .expect_err("the second cut was not reached");
            assert!(
                format!("{error:#}").contains("injected second restore interruption"),
                "{label} {} {} retry cut {cut}: {error:#}",
                image.checkpoint,
                image.kind
            );
            let at = format!(
                "image {} {} then retry cut {cut}/{checkpoints}",
                image.checkpoint, image.kind
            );
            contract.interrupted(home.path(), operation, &at);
            contract.retry(home.path(), operation, &at);
            cuts += 1;
        }
        println!(
            "restore-retained-image mixed {label} twice {} {class} retry-checkpoints={checkpoints}",
            image.checkpoint
        );
    }
    println!(
        "restore-retained-image mixed {label} twice images={} cuts={cuts}",
        classes.len()
    );
    (classes.len(), cuts)
}

pub(super) fn empty_destination(lineage: &Lineage) -> Sources<'_> {
    Sources {
        snapshot: &lineage.points[BASE],
        destination: None,
        authority: &lineage.points[TIGHTENED],
    }
}
fn burned_and_pending(lineage: &Lineage) -> Sources<'_> {
    Sources {
        snapshot: &lineage.points[BASE],
        destination: Some(&lineage.points[ACKNOWLEDGED]),
        authority: &lineage.points[TIGHTENED],
    }
}
fn three_journals(lineage: &Lineage) -> Sources<'_> {
    Sources {
        snapshot: &lineage.points[RESTORED],
        destination: Some(&lineage.points[HANDED_OVER]),
        authority: &lineage.points[RESTORED_AGAIN],
    }
}

/// One image of every class. Each one still repeats every durability barrier,
/// so off Linux, where those are full device flushes, run it with `--ignored`.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "a minute of durability barriers off Linux; run with --ignored"
)]
fn mixed_restore_recovers_one_retained_image_of_every_class() {
    let lineage = Lineage::build();
    for sources in [
        empty_destination(&lineage),
        burned_and_pending(&lineage),
        three_journals(&lineage),
    ] {
        exercise(&lineage, &sources, false);
    }
}

#[test]
#[ignore = "retained-image matrix; minutes of durability barriers"]
fn mixed_restore_retained_images_install_three_pairings_into_an_empty_destination() {
    let lineage = Lineage::build();
    let counts = exercise(&lineage, &empty_destination(&lineage), true);
    println!("restore-retained-image mixed empty-destination total={}", {
        counts.values().sum::<usize>()
    });
}

#[test]
#[ignore = "retained-image matrix; minutes of durability barriers"]
fn mixed_restore_retained_images_keep_burned_and_pending_floors_above_older_authority() {
    let lineage = Lineage::build();
    let counts = exercise(&lineage, &burned_and_pending(&lineage), true);
    println!("restore-retained-image mixed burned-pending total={}", {
        counts.values().sum::<usize>()
    });
}

#[test]
#[ignore = "retained-image matrix; minutes of durability barriers"]
fn mixed_restore_retained_images_keep_journal_only_floors_across_three_journals() {
    let lineage = Lineage::build();
    let counts = exercise(&lineage, &three_journals(&lineage), true);
    println!("restore-retained-image mixed journal-only total={}", {
        counts.values().sum::<usize>()
    });
}

#[test]
#[ignore = "second-interruption matrix; minutes of durability barriers"]
fn mixed_restore_twice_interrupted_install_into_an_empty_destination_recovers() {
    let lineage = Lineage::build();
    exercise_twice(&lineage, &empty_destination(&lineage));
}

#[test]
#[ignore = "second-interruption matrix; minutes of durability barriers"]
fn mixed_restore_twice_interrupted_burned_and_pending_floors_recover() {
    let lineage = Lineage::build();
    exercise_twice(&lineage, &burned_and_pending(&lineage));
}

#[test]
#[ignore = "second-interruption matrix; minutes of durability barriers"]
fn mixed_restore_twice_interrupted_journal_only_floors_recover() {
    let lineage = Lineage::build();
    exercise_twice(&lineage, &three_journals(&lineage));
}
