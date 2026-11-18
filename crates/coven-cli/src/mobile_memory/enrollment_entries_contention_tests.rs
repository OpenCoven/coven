//! Exact IntroductionLedger.entries loop; all-platform component regression.
use super::*;
use std::collections::BTreeMap;

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
fn cancelled_runtime_exits_actual_introduction_entries_wait_without_admission() {
    let tests::Fixture { home, ledger, .. } = tests::Fixture::new_at(Utc::now());
    let ledger = Arc::new(ledger);
    let before = authority_image(home.path());
    let (ready, waiting) = std::sync::mpsc::sync_channel(1);
    *ledger.lock_wait_observer.lock().unwrap() = Some(ready);
    let mut held = Some(ledger.entries.lock().unwrap());
    let runtime = crate::mobile_memory::store_lock::Runtime::default();
    let worker_runtime = runtime.clone();
    let worker_ledger = Arc::clone(&ledger);
    let admitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_admitted = Arc::clone(&admitted);
    let (finished, result) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let _scope = worker_runtime.enter();
        let outcome = worker_ledger
            .entries()
            .map(|entries| {
                worker_admitted.store(true, std::sync::atomic::Ordering::Release);
                drop(entries);
            })
            .map_err(|error| format!("{error:#}"));
        let _ = finished.send(outcome);
    });
    // Readiness is an actual failed attempt; 30s is only a scheduling hang guard.
    let observed = waiting.recv_timeout(Duration::from_secs(30));
    let started = std::time::Instant::now();
    runtime.cancel();
    let outcome = result.recv_timeout(Duration::from_secs(2));
    let receipt_elapsed = started.elapsed();
    // A timeout is failure cleanup only: release so the producer can exit.
    // On success keep actual competing ownership through result AND join.
    if outcome.is_err() {
        drop(held.take());
    }
    let joined = worker.join();
    let joined_elapsed = started.elapsed();
    drop(held.take());
    *ledger.lock_wait_observer.lock().unwrap() = None;
    joined.unwrap();
    assert!(
        ledger.registry.ensure_running().is_ok(),
        "registry was cancelled"
    );
    let after = authority_image(home.path());
    assert!(
        observed.is_ok(),
        "no actual failed introduction entries try_lock: {observed:?}"
    );
    assert!(
            outcome.is_ok() && joined_elapsed < Duration::from_secs(2),
            "introduction cancellation exceeded two seconds: receipt={receipt_elapsed:?}, joined={joined_elapsed:?}, outcome={outcome:?}",
        );
    let error = outcome.unwrap().unwrap_err();
    assert!(
        error.contains("cancelled"),
        "wrong cancellation class: {error}"
    );
    assert!(
        !admitted.load(std::sync::atomic::Ordering::Acquire),
        "cancelled ledger wait was admitted"
    );
    assert!(
        after == before,
        "cancelled ledger wait changed durable authority"
    );
    let fresh = crate::mobile_memory::store_lock::Runtime::default();
    let _scope = fresh.enter();
    assert!(
        ledger.entries().is_ok(),
        "cancelled scope poisoned later ledger admission"
    );
}
