//! Actual v2::prepare_direct entry point and locked_checked custom loops.
use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, MutexGuard},
    time::{Duration, Instant},
};

enum Held {
    Process { _guard: MutexGuard<'static, ()> },
    File { _file: std::fs::File },
}
fn authority_image(home: &Path) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(home.join("mobile"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            entry.file_type().unwrap().is_file()
                && !entry.file_name().to_string_lossy().ends_with(".lock")
        })
        .map(|entry| (entry.file_name(), std::fs::read(entry.path()).unwrap()))
        .collect()
}

#[test]
fn cancelled_v2_prepare_exits_actual_material_process_and_file_waits() {
    for kind in ["process", "file"] {
        let fixture = Arc::new(Fixture::new());
        let lock_path = fixture.home.path().join("mobile/.transport.lock");
        let (ready, waiting) = std::sync::mpsc::sync_channel(1);
        let (file_observer, mut held) = if kind == "process" {
            (
                None,
                Some(Held::Process {
                    _guard: crate::mobile_memory::transport::storage::MATERIAL_LOCK
                        .lock()
                        .unwrap(),
                }),
            )
        } else {
            let file = crate::state_lock::open_lock_file(&lock_path).unwrap();
            fs2::FileExt::lock_exclusive(&file).unwrap();
            let (observer, actual) = crate::mobile_memory::store_lock::observe(&lock_path);
            (Some((observer, actual)), Some(Held::File { _file: file }))
        };
        let before = authority_image(fixture.home.path());
        let runtime = crate::mobile_memory::store_lock::Runtime::default();
        let worker_runtime = runtime.clone();
        let worker_fixture = Arc::clone(&fixture);
        let (finished, result) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let _scope = worker_runtime.enter();
            if kind == "process" {
                crate::mobile_memory::store_lock::observe_process_waits(ready);
            }
            let outcome = prepare_direct(
                worker_fixture.home.path(),
                &worker_fixture.registry,
                &worker_fixture.host,
                worker_fixture.id,
            )
            .map(drop)
            .map_err(|error| format!("{error:#}"));
            let _ = finished.send(outcome);
        });
        // Readiness is an actual failed attempt; 30s is only a scheduling hang guard.
        let observed = match file_observer.as_ref() {
            Some((_, actual)) => actual.recv_timeout(Duration::from_secs(30)),
            None => waiting.recv_timeout(Duration::from_secs(30)),
        };
        let started = Instant::now();
        runtime.cancel();
        let outcome = result.recv_timeout(Duration::from_secs(2));
        let receipt_elapsed = started.elapsed();
        if outcome.is_err() {
            drop(held.take());
        }
        let joined = worker.join();
        let joined_elapsed = started.elapsed();
        drop(held.take());
        drop(file_observer);
        joined.unwrap();
        assert!(
            fixture.registry.ensure_running().is_ok(),
            "registry was cancelled"
        );
        let after = authority_image(fixture.home.path());
        assert!(
            observed.is_ok(),
            "no actual v2 {kind} failed attempt: {observed:?}"
        );
        assert!(
            outcome.is_ok() && joined_elapsed < Duration::from_secs(2),
            "v2 {kind} cancellation exceeded two seconds: receipt={receipt_elapsed:?}, joined={joined_elapsed:?}, outcome={outcome:?}",
        );
        let error = outcome.unwrap().unwrap_err();
        assert!(
            error.contains("cancelled"),
            "wrong v2 {kind} cancellation class: {error}"
        );
        assert!(
            after == before,
            "cancelled v2 {kind} wait changed durable authority"
        );
        let fresh = crate::mobile_memory::store_lock::Runtime::default();
        let _scope = fresh.enter();
        assert!(
            prepare_direct(
                fixture.home.path(),
                &fixture.registry,
                &fixture.host,
                fixture.id
            )
            .is_ok(),
            "v2 {kind} cancellation poisoned fresh-scope prepare"
        );
    }
}
