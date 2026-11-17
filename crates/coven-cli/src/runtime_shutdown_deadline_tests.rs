//! Deterministic shared-deadline/Drop tests. Include from daemon.rs as a
//! cfg(test) child module; no sleeps, OS processes or global daemon flags.
use super::*;

struct SignalWitness {
    signals: Arc<std::sync::atomic::AtomicUsize>,
    waits: Arc<Mutex<Vec<Duration>>>,
}
impl RuntimeKiller for SignalWitness {
    fn kill(&mut self) -> Result<()> {
        self.signals.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn wait_for_shutdown_quiescence(&mut self, budget: Duration) -> Result<()> {
        self.waits.lock().unwrap().push(budget);
        Ok(())
    }
}

struct RestoreWaitTrace(Option<std::sync::mpsc::SyncSender<Duration>>);
impl Drop for RestoreWaitTrace {
    fn drop(&mut self) {
        LIVE_LAUNCH_DRAIN_WAITS.with(|slot| {
            slot.replace(self.0.take());
        });
    }
}

#[test]
fn runtime_drop_does_not_renew_waits_and_keeps_late_spawn_backstops() {
    let signals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let waits = Arc::new(Mutex::new(Vec::new()));
    let runtime = LiveSessionRuntime::default();
    let published = runtime.launch_gate.begin().unwrap();
    let late = runtime.launch_gate.begin().unwrap();
    let (_, _published_killer) = published
        .spawn_owned(|publish| {
            publish(Box::new(SignalWitness {
                signals: Arc::clone(&signals),
                waits: Arc::clone(&waits),
            }))?;
            Ok(())
        })
        .unwrap();
    let (observations, observed) = std::sync::mpsc::sync_channel(16);
    let previous = LIVE_LAUNCH_DRAIN_WAITS.with(|slot| slot.replace(Some(observations)));
    let trace = RestoreWaitTrace(previous);
    // Both provisional tokens deliberately outlive the runtime's entire
    // explicit shutdown AND Drop. A renewed 500ms drain wait is then observed
    // deterministically by the actual wait call, without a wall-clock race.
    runtime.shutdown_all_until(Instant::now()).unwrap();
    drop(runtime);
    let drain_waits = observed.try_iter().collect::<Vec<_>>();
    drop(trace);
    let late_signals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rejected = late.spawn_owned(|publish| {
        publish(Box::new(SignalWitness {
            signals: Arc::clone(&late_signals),
            waits: Arc::clone(&waits),
        }))?;
        Ok(())
    });
    drop(late);
    drop(published);
    assert!(
        drain_waits.is_empty(),
        "Drop renewed an admission wait: {drain_waits:?}"
    );
    assert!(
        signals.load(Ordering::SeqCst) >= 2,
        "Drop lost the provisional exact-tree signal retry"
    );
    assert!(
        rejected.is_err(),
        "closed admission accepted a late native spawn"
    );
    assert_eq!(
        late_signals.load(Ordering::SeqCst),
        1,
        "late rejected tree lost its kill/backstop"
    );
}

#[test]
fn runtime_quiescence_uses_zero_remaining_budget_after_shared_deadline() {
    let signals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let waits = Arc::new(Mutex::new(Vec::new()));
    let runtime = LiveSessionRuntime::default();
    runtime
        .register(
            "deadline-witness".to_owned(),
            Box::new(std::io::sink()),
            Box::new(SignalWitness {
                signals: Arc::clone(&signals),
                waits: Arc::clone(&waits),
            }),
        )
        .unwrap();
    runtime
        .shutdown_all_until(Instant::now() - Duration::from_secs(1))
        .unwrap();
    drop(runtime);
    assert_eq!(*waits.lock().unwrap(), vec![Duration::ZERO]);
    assert_eq!(signals.load(Ordering::SeqCst), 1);
}
