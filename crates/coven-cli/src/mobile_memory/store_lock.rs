//! Lock admission for runtime-owned mobile storage. Deadlines apply to waits,
//! not to filesystem calls or transactions that have already been admitted.
use anyhow::{Context, Result};
use fs2::FileExt;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

// One absolute budget spans nested initial stores. This bounds contention
// before a runtime handle exists; it does not promise a storage I/O deadline.
const STARTUP_LOCK_BUDGET: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

/// Runtime-owned wait cancellation, independent of durable authority state.
/// Every gateway worker enters this scope before calling nested store code.
#[derive(Clone, Default)]
pub(super) struct Runtime(Arc<AtomicBool>);

impl Runtime {
    pub(super) fn current() -> Self {
        RUNTIME.with(|slot| slot.borrow().clone().unwrap_or_default())
    }

    pub(super) fn enter(&self) -> Scope {
        Scope {
            previous: RUNTIME.with(|slot| slot.replace(Some(self.clone()))),
            // Restoring another thread's TLS would be wrong; scopes cannot move.
            _thread: std::marker::PhantomData,
        }
    }

    pub(super) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) struct Scope {
    previous: Option<Runtime>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Drop for Scope {
    fn drop(&mut self) {
        RUNTIME.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

fn is_scoped() -> bool {
    DEADLINE.with(|slot| slot.get().is_some()) || RUNTIME.with(|slot| slot.borrow().is_some())
}

struct RestoreDeadline(Option<Instant>);
impl Drop for RestoreDeadline {
    fn drop(&mut self) {
        DEADLINE.with(|slot| slot.set(self.0));
    }
}

pub(super) fn during_startup<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let deadline = Instant::now() + STARTUP_LOCK_BUDGET;
    let previous = DEADLINE.with(|slot| {
        let previous = slot.get();
        slot.set(Some(previous.map_or(deadline, |old| old.min(deadline))));
        previous
    });
    let _restore = RestoreDeadline(previous);
    let runtime = Runtime::current();
    let _scope = runtime.enter();
    check()?;
    operation()
}

pub(super) fn check() -> Result<()> {
    anyhow::ensure!(
        RUNTIME.with(|slot| slot
            .borrow()
            .as_ref()
            .is_none_or(|runtime| !runtime.0.load(Ordering::Acquire))),
        "mobile store operation cancelled during runtime shutdown"
    );
    anyhow::ensure!(
        DEADLINE.with(|slot| slot.get().is_none_or(|end| Instant::now() < end)),
        "mobile startup store-lock admission deadline expired"
    );
    Ok(())
}

pub(super) fn process<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    check()?;
    if !is_scoped() {
        return mutex
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile store process lock poisoned"));
    }
    loop {
        check()?;
        match mutex.try_lock() {
            Ok(guard) => {
                check()?;
                return Ok(guard);
            }
            Err(TryLockError::WouldBlock) => {
                #[cfg(test)]
                notify_process_wait();
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(TryLockError::Poisoned(_)) => anyhow::bail!("mobile store process lock poisoned"),
        }
    }
}

pub(super) fn lock_exclusive(file: &std::fs::File, path: &Path) -> Result<()> {
    loop {
        check()?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                check()?;
                return Ok(());
            }
            Err(error) if crate::state_lock::is_lock_contended(&error) => {
                #[cfg(test)]
                notify(path);
                if !is_scoped() {
                    return file
                        .lock_exclusive()
                        .with_context(|| format!("failed to lock {}", path.display()));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("failed to lock {}", path.display()))
            }
        }
    }
}

#[cfg(test)]
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{mpsc, LazyLock},
};

#[cfg(test)]
static WAITERS: LazyLock<Mutex<HashMap<PathBuf, mpsc::SyncSender<()>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub(super) struct Observation(PathBuf);
#[cfg(test)]
impl Drop for Observation {
    fn drop(&mut self) {
        WAITERS.lock().unwrap().remove(&self.0);
    }
}

#[cfg(test)]
pub(super) fn observe(path: &Path) -> (Observation, mpsc::Receiver<()>) {
    let (send, receive) = mpsc::sync_channel(1);
    assert!(WAITERS
        .lock()
        .unwrap()
        .insert(path.to_owned(), send)
        .is_none());
    (Observation(path.to_owned()), receive)
}

#[cfg(test)]
pub(super) fn notify(path: &Path) {
    if let Some(sender) = WAITERS.lock().unwrap().get(path) {
        let _ = sender.try_send(());
    }
}

#[cfg(test)]
thread_local! {
    static PROCESS_WAITER: RefCell<Option<mpsc::SyncSender<()>>> = const { RefCell::new(None) };
}
#[cfg(test)]
fn notify_process_wait() {
    PROCESS_WAITER.with(|slot| {
        if let Some(sender) = slot.borrow().as_ref() {
            let _ = sender.try_send(());
        }
    });
}

#[cfg(test)]
pub(super) fn observe_process_waits(sender: mpsc::SyncSender<()>) {
    PROCESS_WAITER.with(|slot| {
        slot.replace(Some(sender));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_cancel_interrupts_process_mutex_wait_without_admission() {
        let mutex = Arc::new(Mutex::new(()));
        let held = mutex.lock().unwrap();
        let runtime = Runtime::default();
        let worker_runtime = runtime.clone();
        let worker_mutex = Arc::clone(&mutex);
        let (ready, waiting) = mpsc::sync_channel(1);
        let (finished, stopped) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            PROCESS_WAITER.with(|slot| {
                slot.replace(Some(ready));
            });
            let _scope = worker_runtime.enter();
            let result = process(&worker_mutex)
                .map(drop)
                .map_err(|error| error.to_string());
            let _ = finished.send(result);
        });
        let observed = waiting.recv_timeout(Duration::from_secs(30));
        let started = Instant::now();
        runtime.cancel();
        let result = stopped.recv_timeout(Duration::from_secs(2));
        let elapsed = started.elapsed();
        drop(held);
        worker.join().unwrap();
        assert!(
            observed.is_ok(),
            "no actual failed process-lock attempt: {observed:?}"
        );
        let error = result
            .unwrap_or_else(|error| {
                panic!("process wait exceeded shutdown contract after {elapsed:?}: {error:?}")
            })
            .unwrap_err();
        assert!(error.contains("cancelled"), "unexpected refusal: {error}");
        assert!(
            process(&mutex).is_ok(),
            "runtime cancellation affected standalone admission"
        );
    }

    #[test]
    fn cancelled_scope_is_restored_without_poisoning_independent_runtime() {
        let mutex = Mutex::new(());
        let stopped = Runtime::default();
        let independent = Runtime::default();
        stopped.cancel();
        {
            let _scope = stopped.enter();
            assert!(
                process(&mutex).is_err(),
                "cancelled runtime admitted new work"
            );
            {
                let _other = independent.enter();
                assert!(
                    process(&mutex).is_ok(),
                    "cancellation crossed runtime ownership"
                );
            }
            assert!(
                process(&mutex).is_err(),
                "nested scope erased outer cancellation"
            );
        }
        assert!(
            process(&mutex).is_ok(),
            "scope leaked into a standalone operation"
        );
    }

    #[test]
    fn nested_startup_never_renews_an_expired_admission_deadline() {
        let expired = Instant::now() - Duration::from_secs(1);
        let previous = DEADLINE.with(|slot| slot.replace(Some(expired)));
        let _restore = RestoreDeadline(previous);
        let mut admitted = false;
        let result = during_startup(|| {
            admitted = true;
            Ok(())
        });
        assert!(result.is_err(), "nested startup renewed an expired budget");
        assert!(!admitted, "expired startup reached store work");
        assert_eq!(DEADLINE.with(Cell::get), Some(expired));
    }
}
