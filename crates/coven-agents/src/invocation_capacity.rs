//! Local admission accounting shared by durable invocation runners.
//!
//! A slot covers adoption through known durable settlement. Unknown adoption,
//! interrupted execution and failed settlement retain their charge. This is
//! process-local admission control, not a runtime containment or recovery proof.

use std::{
    num::NonZeroUsize,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A host-owned local capacity pool. Pass the same `Arc` to every durable
/// runner whose work must share a bound. There is no implicit default limit,
/// waiting queue, automatic retry or authority to release unresolved work.
///
/// Charges do not survive process restart. Hosts still have to discover and
/// reconcile old journal records before dispatching replacement work; creating
/// a new pool never fences old attempts or permits replay of an unknown id.
#[derive(Debug)]
pub struct InvocationCapacity {
    limit: NonZeroUsize,
    used: AtomicUsize,
}

impl InvocationCapacity {
    pub fn new(limit: NonZeroUsize) -> Self {
        Self {
            limit,
            used: AtomicUsize::new(0),
        }
    }

    pub fn limit(&self) -> NonZeroUsize {
        self.limit
    }

    /// Includes unresolved charges, not just futures currently being polled.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }

    pub(crate) fn try_acquire(&self) -> Option<InvocationPermit<'_>> {
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < self.limit.get()).then(|| used + 1)
            })
            .ok()?;
        Some(InvocationPermit {
            capacity: self,
            release_on_drop: true,
        })
    }
}

pub(crate) struct InvocationPermit<'a> {
    capacity: &'a InvocationCapacity,
    release_on_drop: bool,
}

impl InvocationPermit<'_> {
    /// Must precede awaiting the adoption write: dropping that write can
    /// leave its durable outcome unknown even before execution is polled.
    pub(crate) fn retain(&mut self) {
        self.release_on_drop = false;
    }

    pub(crate) fn release(&mut self) {
        self.release_on_drop = true;
    }
}

impl Drop for InvocationPermit<'_> {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.capacity.used.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_admission_never_exceeds_the_shared_limit() {
        let capacity = Arc::new(InvocationCapacity::new(NonZeroUsize::new(3).unwrap()));
        let entered = Arc::new(Barrier::new(17));
        let leave = Arc::new(Barrier::new(17));
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let capacity = capacity.clone();
                let entered = entered.clone();
                let leave = leave.clone();
                std::thread::spawn(move || {
                    let permit = capacity.try_acquire();
                    entered.wait();
                    leave.wait();
                    permit.is_some()
                })
            })
            .collect();
        entered.wait();
        let used_at_barrier = capacity.used();
        leave.wait();
        let admitted = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(used_at_barrier, 3);
        assert_eq!(admitted, 3);
        assert_eq!(capacity.used(), 0);
    }
}
