//! Daemon-owned outbound workers; network state never changes device authority.
use super::*;
use crate::mobile_memory::transport::{
    self, authority, cancellation::Cancellation, storage, websocket,
};
use p256::ecdsa::SigningKey;
use std::collections::BTreeMap;
use std::sync::Condvar;
use std::time::Instant;

mod cache;
use cache::Cache;

#[cfg(test)]
mod tests;

const MAX_WORKERS: usize = 32;
const MANAGER_INTERVAL: Duration = Duration::from_secs(1);
const CONNECT_BUDGET: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Stop {
    requested: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
}
impl Stop {
    fn request(&self) {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.requested.store(true, Ordering::Release);
        self.wake.notify_all();
    }
    fn wait(&self) {
        let guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        if !self.requested.load(Ordering::Acquire) {
            drop(self.wake.wait_timeout(guard, MANAGER_INTERVAL));
        }
    }
}

#[derive(Default)]
struct WorkerControl(Mutex<(bool, Option<Cancellation>)>);
impl WorkerControl {
    fn attempt(&self) -> Result<Cancellation> {
        let mut control = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("relay worker unavailable"))?;
        anyhow::ensure!(!control.0, "relay worker stopped");
        if let Some(previous) = control.1.take() {
            previous.cancel();
        }
        let cancellation = Cancellation::default();
        control.1 = Some(cancellation.clone());
        Ok(cancellation)
    }
    fn cancelled(&self) -> bool {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).0
    }
    fn cancel(&self) {
        let mut control = self.0.lock().unwrap_or_else(|error| error.into_inner());
        control.0 = true;
        if let Some(cancellation) = control.1.take() {
            cancellation.cancel();
        }
    }
}

struct Worker {
    control: Arc<WorkerControl>,
    authenticated: Arc<AtomicBool>,
    pairing: transport::Pairing,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.control.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug)]
struct Retry {
    count: usize,
    after: Instant,
}

pub(super) struct RelayHandle {
    stop: Arc<Stop>,
    registry: Arc<DeviceRegistry>,
    thread: Option<JoinHandle<()>>,
}
impl RelayHandle {
    pub(super) fn start(state: Arc<MobileGatewayState>, signer: Arc<SigningKey>) -> Result<Self> {
        transport::diagnostics::initialize()?;
        let stop = Arc::new(Stop::default());
        let manager_stop = Arc::clone(&stop);
        let registry = Arc::clone(&state.registry);
        let thread = std::thread::Builder::new()
            .name("coven-relay-manager".into())
            .spawn(move || manage(state, signer, manager_stop))?;
        Ok(Self {
            stop,
            registry,
            thread: Some(thread),
        })
    }
}
impl Drop for RelayHandle {
    fn drop(&mut self) {
        self.stop.request();
        self.registry.cancel_pending_operations();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct Manager {
    workers: BTreeMap<Uuid, Worker>,
    retries: BTreeMap<Uuid, Retry>,
    configuration: Option<storage::RelayConfig>,
    endpoints: Vec<Arc<websocket::RelayEndpoint>>,
    identity: Cache<bool>,
    registry: Cache<Vec<super::super::registry::DeviceAuthorizationRecord>>,
    cursor: usize,
}

impl Retry {
    fn failed(retries: &mut BTreeMap<Uuid, Self>, id: Uuid, recovered: bool, now: Instant) {
        let retry = retries.entry(id).or_insert(Self {
            count: 0,
            after: now,
        });
        retry.count = if recovered {
            0
        } else {
            retry.count.saturating_add(1)
        };
        let seconds = (1_u64 << retry.count.min(5)).min(30);
        retry.after = now + Duration::from_secs(seconds) + retry_jitter();
    }
}

impl Manager {
    fn tick(
        &mut self,
        state: &Arc<MobileGatewayState>,
        signer: &Arc<SigningKey>,
        stop: &Stop,
        now: Instant,
    ) {
        let finished: Vec<_> = self
            .workers
            .iter()
            .filter(|(_, worker)| {
                worker
                    .thread
                    .as_ref()
                    .is_none_or(|thread| thread.is_finished())
            })
            .map(|(id, _)| *id)
            .collect();
        let worker_event = !finished.is_empty();
        let next = storage::RelayConfig::load(&state.coven_home)
            .ok()
            .flatten()
            .filter(|config| config.enabled);
        if next != self.configuration {
            for worker in self.workers.values() {
                worker.control.cancel();
            }
            self.endpoints = next
                .as_ref()
                .map(|config| {
                    config
                        .endpoints
                        .iter()
                        .map(|endpoint| endpoint.parse().map(Arc::new))
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            self.configuration = next;
            // Configuration changes must not erase a policy failure's history.
        }
        self.identity.refresh(
            &state.coven_home,
            &["host-key.pem", "host-identity.json"],
            worker_event,
            || {
                super::super::identity::load_existing_host_identity(&state.coven_home)
                    .map(|identity| identity.public_key_fingerprint == state.host_fingerprint)
            },
        );
        self.registry.refresh(
            &state.coven_home,
            &[super::super::registry::DEVICES_FILE],
            worker_event,
            || {
                state.registry.reload()?;
                state.registry.authorization_snapshot()
            },
        );
        for id in finished {
            let worker = self.workers.remove(&id).expect("worker exists");
            // Read cancellation before Drop cancels the control for cleanup.
            // Authentication from an earlier policy generation is not recovery.
            let recovered = worker.authenticated.load(Ordering::Acquire)
                && !worker.control.cancelled()
                && self.identity.value == Some(true)
                && self.registry.value.as_ref().is_some_and(|records| {
                    records
                        .iter()
                        .find(|record| record.device.id == id)
                        .is_some_and(|record| {
                            authority::check_record(record, &worker.pairing, false, Utc::now())
                                .is_ok()
                        })
                });
            drop(worker);
            Retry::failed(&mut self.retries, id, recovered, now);
        }
        let Some(devices) = self.registry.value.as_ref() else {
            for worker in self.workers.values() {
                worker.control.cancel();
            }
            return;
        };
        self.retries
            .retain(|id, _| devices.iter().any(|record| record.device.id == *id));
        if self.endpoints.is_empty() || self.identity.value != Some(true) {
            for worker in self.workers.values() {
                worker.control.cancel();
            }
            return;
        }
        for (id, worker) in &self.workers {
            let valid = devices
                .iter()
                .find(|record| record.device.id == *id)
                .is_some_and(|record| {
                    authority::check_record(record, &worker.pairing, false, Utc::now()).is_ok()
                });
            if !valid {
                worker.control.cancel();
            }
        }
        for offset in 0..devices.len() {
            if self.workers.len() >= MAX_WORKERS || stop.requested.load(Ordering::Acquire) {
                break;
            }
            let record = &devices[self.cursor.wrapping_add(offset) % devices.len()];
            let id = record.device.id;
            if self.workers.contains_key(&id)
                || self.retries.get(&id).is_some_and(|retry| retry.after > now)
            {
                continue;
            }
            let material = match storage::load_existing_for_record(
                &state.coven_home,
                signer.verifying_key(),
                record,
            ) {
                Ok(Some(material)) => material,
                _ => {
                    Retry::failed(&mut self.retries, id, false, now);
                    continue;
                }
            };
            let attempt = self.retries.get(&id).map_or(0, |retry| retry.count);
            let endpoint = Arc::clone(&self.endpoints[attempt % self.endpoints.len()]);
            let control = Arc::new(WorkerControl::default());
            let authenticated = Arc::new(AtomicBool::new(false));
            let worker_state = Arc::clone(state);
            let worker_signer = Arc::clone(signer);
            let worker_control = Arc::clone(&control);
            let worker_authenticated = Arc::clone(&authenticated);
            let pairing = material.pairing.clone();
            let spawned = std::thread::Builder::new()
                .name("coven-relay-peer".into())
                .spawn(move || {
                    let _ = serve(
                        worker_state,
                        worker_signer,
                        material,
                        endpoint,
                        worker_control,
                        worker_authenticated,
                    );
                });
            match spawned {
                Ok(thread) => {
                    self.workers.insert(
                        id,
                        Worker {
                            control,
                            authenticated,
                            pairing,
                            thread: Some(thread),
                        },
                    );
                }
                Err(_) => Retry::failed(&mut self.retries, id, false, now),
            }
        }
        self.cursor = self.cursor.wrapping_add(1);
    }
}

fn manage(state: Arc<MobileGatewayState>, signer: Arc<SigningKey>, stop: Arc<Stop>) {
    let mut manager = Manager::default();
    while !stop.requested.load(Ordering::Acquire) {
        manager.tick(&state, &signer, &stop, Instant::now());
        stop.wait();
    }
    for worker in manager.workers.values() {
        worker.control.cancel();
    }
    drop(manager);
}

fn retry_jitter() -> Duration {
    use ring::rand::SecureRandom;
    let mut byte = [0_u8];
    let _ = ring::rand::SystemRandom::new().fill(&mut byte);
    Duration::from_millis(u64::from(byte[0]))
}

fn serve(
    state: Arc<MobileGatewayState>,
    signer: Arc<SigningKey>,
    material: storage::Material,
    endpoint: Arc<websocket::RelayEndpoint>,
    control: Arc<WorkerControl>,
    authenticated: Arc<AtomicBool>,
) -> Result<()> {
    let slot = u64::try_from(Utc::now().timestamp())? / storage::SLOT_SECONDS;
    let (room, credential) = material.rendezvous(slot, false)?;
    let deadline = Instant::now() + CONNECT_BUDGET;
    let cancellation = control.attempt()?;
    let addresses = endpoint.resolve(&cancellation, deadline)?;
    let mut records = None;
    for address in addresses {
        let cancellation = control.attempt()?;
        if let Ok(connection) = websocket::connect_cancellable(
            &endpoint,
            address,
            websocket::Role::Host,
            &room,
            &credential,
            deadline,
            &cancellation,
        ) {
            records = Some(connection);
            break;
        }
    }
    let records = records.context("relay endpoints unavailable")?;
    let mut channel = transport::accept(
        records,
        &material.pairing,
        &material.noise_private,
        &signer,
        &room,
        false,
        |pairing, direct| authority::check_current(&state.registry, pairing, direct, Utc::now()),
    )?;
    authenticated.store(true, Ordering::Release);
    while channel.is_open() {
        serve_transport_with_state(&state, &mut channel)?;
    }
    Ok(())
}
