//! Public RelayConfig::update entry point, actual CONFIG_LOCK/.relay.lock.
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
fn cancelled_relay_config_update_exits_its_actual_process_and_file_waits() {
    for kind in ["process", "file"] {
        let home = tempfile::tempdir().unwrap();
        RelayConfig {
            listener_limit: MAX_LISTENERS,
            version: 1,
            enabled: false,
            endpoints: Vec::new(),
        }
        .save(home.path())
        .unwrap();
        let lock_path = home.path().join("mobile/.relay.lock");
        let (ready, waiting) = std::sync::mpsc::sync_channel(1);
        let (file_observer, mut held) = if kind == "process" {
            (
                None,
                Some(Held::Process {
                    _guard: CONFIG_LOCK.lock().unwrap(),
                }),
            )
        } else {
            let file = crate::state_lock::open_lock_file(&lock_path).unwrap();
            fs2::FileExt::lock_exclusive(&file).unwrap();
            let (observer, actual) = crate::mobile_memory::store_lock::observe(&lock_path);
            (Some((observer, actual)), Some(Held::File { _file: file }))
        };
        let before = authority_image(home.path());
        let runtime = crate::mobile_memory::store_lock::Runtime::default();
        let worker_runtime = runtime.clone();
        let worker_home = home.path().to_owned();
        let admitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_admitted = Arc::clone(&admitted);
        let (finished, result) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let _scope = worker_runtime.enter();
            if kind == "process" {
                crate::mobile_memory::store_lock::observe_process_waits(ready);
            }
            let outcome = RelayConfig::update(&worker_home, |config| {
                worker_admitted.store(true, std::sync::atomic::Ordering::Release);
                config.listener_limit = MAX_LISTENERS - 2;
                Ok(())
            })
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
        let after = authority_image(home.path());
        assert!(
            observed.is_ok(),
            "no actual {kind} relay-config failed attempt: {observed:?}"
        );
        assert!(
            outcome.is_ok() && joined_elapsed < Duration::from_secs(2),
            "relay config {kind} cancellation exceeded two seconds: receipt={receipt_elapsed:?}, joined={joined_elapsed:?}, outcome={outcome:?}",
        );
        let error = outcome.unwrap().unwrap_err();
        assert!(
            error.contains("cancelled"),
            "wrong {kind} cancellation class: {error}"
        );
        assert!(
            !admitted.load(std::sync::atomic::Ordering::Acquire),
            "cancelled {kind} config update ran its mutation"
        );
        assert!(
            after == before,
            "cancelled {kind} config wait changed authority"
        );
        let fresh = crate::mobile_memory::store_lock::Runtime::default();
        let _scope = fresh.enter();
        RelayConfig::update(home.path(), |config| {
            config.listener_limit = MAX_LISTENERS - 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            RelayConfig::load(home.path())
                .unwrap()
                .unwrap()
                .listener_limit,
            MAX_LISTENERS - 1
        );
    }
}
