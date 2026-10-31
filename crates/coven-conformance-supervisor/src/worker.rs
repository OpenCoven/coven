//! Independent blocking work with one cooperative cleanup guard for the group.
use crate::{control::Refusal, Failure};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Workers {
    pub stopping: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    cleanup_failed: bool,
}
impl Workers {
    pub fn new() -> Self {
        Self {
            stopping: Arc::new(AtomicBool::new(false)),
            threads: Vec::new(),
            cleanup_failed: false,
        }
    }
    pub fn spawn<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<T, Refusal> + Send + 'static,
    ) -> Result<Receiver<Result<T, Refusal>>, Refusal> {
        if self.stopping.load(Ordering::Acquire) || self.threads.len() >= 8 {
            return Err(Refusal::IoRefused);
        }
        let (send, receive) = mpsc::sync_channel(1);
        let stopping = self.stopping.clone();
        let worker = thread::Builder::new()
            .name("coven-audit-io".into())
            .spawn(move || {
                let _ = send.send(work(stopping));
            })
            .map_err(|_| Refusal::IoRefused)?;
        self.threads.push(worker);
        Ok(receive)
    }
    /// Prove only worker-thread termination, never cleanup of values delivered
    /// into result queues. The coordinator must drain/drop or assume ownership
    /// of every result (especially jobs/images) before claiming resource cleanup.
    /// Failure is permanent; on unproven termination the standalone helper exits.
    pub fn stop(&mut self) -> Result<(), Failure> {
        self.stop_with_guard(Duration::from_secs(10))
    }
    fn stop_with_guard(&mut self, guard: Duration) -> Result<(), Failure> {
        self.stopping.store(true, Ordering::Release);
        let started = Instant::now();

        loop {
            let mut index = 0;
            while index < self.threads.len() {
                if self.threads[index].is_finished() {
                    let worker = self.threads.swap_remove(index);
                    self.cleanup_failed |= worker.join().is_err();
                } else {
                    cancel_sync(&self.threads[index]);
                    index += 1;
                }
            }
            if self.threads.is_empty() {
                return if self.cleanup_failed {
                    Err(Failure::CleanupUnproven)
                } else {
                    Ok(())
                };
            }
            // One group hang guard, not a per-worker latency/SLO assertion.
            if started.elapsed() >= guard {
                // Dropping a JoinHandle detaches; its closure still owns all I/O
                // storage. The standalone caller must exit on this result.
                self.cleanup_failed = true;
                self.threads.clear();
                return Err(Failure::CleanupUnproven);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Default for Workers {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        // Best effort only. Call stop explicitly for reportable cleanup proof.
        self.stopping.store(true, Ordering::Release);
        for worker in &self.threads {
            cancel_sync(worker);
        }
    }
}
fn cancel_sync(worker: &JoinHandle<()>) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: This owned JoinHandle retains the exact thread handle. The
        // request may race completion; neither result proves I/O completion.
        unsafe { windows_sys::Win32::System::IO::CancelSynchronousIo(worker.as_raw_handle()) };
    }
    #[cfg(not(windows))]
    let _ = worker;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancels_all_workers_and_retains_each_bounded_result() {
        let mut workers = Workers::new();
        let results: Vec<_> = (0..3)
            .map(|value| {
                workers
                    .spawn(move |stop| {
                        while !stop.load(Ordering::Acquire) {
                            thread::yield_now();
                        }
                        Ok(value)
                    })
                    .unwrap()
            })
            .collect();
        workers.stop().unwrap();
        assert!(workers.stopping.load(Ordering::Acquire));
        assert!(workers.threads.is_empty());
        for (value, result) in results.into_iter().enumerate() {
            assert_eq!(result.recv().unwrap(), Ok(value));
        }
    }
    #[test]
    fn refuses_worker_growth_and_work_after_stop() {
        let mut workers = Workers::new();
        for _ in 0..8 {
            workers.spawn(|_| Ok(())).unwrap();
        }
        assert!(matches!(workers.spawn(|_| Ok(())), Err(Refusal::IoRefused)));
        workers.stop().unwrap();
        assert!(matches!(workers.spawn(|_| Ok(())), Err(Refusal::IoRefused)));
    }

    #[test]
    fn joined_threads_do_not_prove_cleanup_of_queued_resource_values() {
        use std::sync::atomic::AtomicUsize;
        struct Resource(Arc<AtomicUsize>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::AcqRel);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let resource = Resource(dropped.clone());
        let mut workers = Workers::new();
        let result = workers.spawn(move |_| Ok(resource)).unwrap();
        workers.stop().unwrap();
        assert_eq!(dropped.load(Ordering::Acquire), 0);
        drop(result); // The coordinator owns this separate resource cleanup step.
        assert_eq!(dropped.load(Ordering::Acquire), 1);
    }

    #[cfg(windows)]
    #[test]
    fn cancels_a_group_of_kernel_pending_reads_with_peers_still_open() {
        use std::{
            fs::File,
            io::Read,
            os::windows::io::{AsRawHandle, FromRawHandle},
            ptr::{null, null_mut},
        };
        use windows_sys::Win32::System::{Pipes::CreatePipe, Threading::GetThreadIOPendingFlag};
        let mut workers = Workers::new();
        let mut peers = Vec::new();
        let mut results = Vec::new();
        for _ in 0..3 {
            let (mut read, mut write) = (null_mut(), null_mut());
            // SAFETY: Writable outputs and noninheritable default byte-pipe handles.
            assert_ne!(
                unsafe { CreatePipe(&mut read, &mut write, null(), 4096) },
                0
            );
            // SAFETY: Successful creation returned independently owned handles.
            let (read, write) =
                unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) };
            peers.push(write);
            results.push(
                workers
                    .spawn(move |stop| {
                        let mut pipe = crate::pipe::CancellablePipe::new(read, stop).unwrap();
                        pipe.read(&mut [0])
                            .map(|_| ())
                            .map_err(|_| Refusal::IoRefused)
                    })
                    .unwrap(),
            );
        }
        let started = Instant::now();
        loop {
            let all_pending = workers.threads.iter().all(|worker| {
                let mut pending = 0;
                // SAFETY: Exact owned thread handle and writable output.
                assert_ne!(
                    unsafe { GetThreadIOPendingFlag(worker.as_raw_handle(), &mut pending) },
                    0
                );
                pending != 0
            });
            if all_pending {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "group fixture hang guard after {:?}",
                started.elapsed()
            );
            thread::sleep(Duration::from_millis(10));
        }
        workers.stop().unwrap();
        assert!(workers.threads.is_empty());
        for result in results {
            assert_eq!(result.recv().unwrap(), Err(Refusal::IoRefused));
        }
        drop(peers); // Cancellation, not writer closure, released the native reads.
    }

    #[test]
    fn panic_is_unproven_cleanup_even_after_the_thread_ends() {
        let mut workers = Workers::new();
        let result = workers.spawn::<()>(|_| panic!("fixture panic")).unwrap();
        assert!(result.recv().is_err());
        assert_eq!(workers.stop(), Err(Failure::CleanupUnproven));
        assert!(workers.threads.is_empty());
        assert_eq!(workers.stop(), Err(Failure::CleanupUnproven));
    }
    #[test]
    fn exhausted_group_guard_detaches_without_claiming_cleanup() {
        let mut workers = Workers::new();
        let (release, wait) = mpsc::channel();
        let result = workers
            .spawn(move |_| {
                wait.recv().unwrap();
                Ok(17)
            })
            .unwrap();
        assert_eq!(
            workers.stop_with_guard(Duration::ZERO),
            Err(Failure::CleanupUnproven)
        );
        assert!(workers.stopping.load(Ordering::Acquire));
        assert!(workers.threads.is_empty());
        assert_eq!(workers.stop(), Err(Failure::CleanupUnproven));
        release.send(()).unwrap();
        assert_eq!(result.recv().unwrap(), Ok(17));
    }
}
