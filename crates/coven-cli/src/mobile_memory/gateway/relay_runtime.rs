//! Daemon-owned outbound workers; network state never changes device authority.
use super::*;
use crate::mobile_memory::transport::{
    self, authority, cancellation::Cancellation, storage, websocket,
};
use p256::ecdsa::SigningKey;
use std::collections::BTreeMap;
use std::sync::Condvar;
use std::time::Instant;

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

struct Retry {
    count: usize,
    after: Instant,
}

pub(super) struct RelayHandle {
    stop: Arc<Stop>,
    thread: Option<JoinHandle<()>>,
}
impl RelayHandle {
    pub(super) fn start(state: Arc<MobileGatewayState>, signer: Arc<SigningKey>) -> Result<Self> {
        transport::diagnostics::initialize()?;
        let stop = Arc::new(Stop::default());
        let manager_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("coven-relay-manager".into())
            .spawn(move || manage(state, signer, manager_stop))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for RelayHandle {
    fn drop(&mut self) {
        self.stop.request();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn manage(state: Arc<MobileGatewayState>, signer: Arc<SigningKey>, stop: Arc<Stop>) {
    let mut workers: BTreeMap<Uuid, Worker> = BTreeMap::new();
    let mut retries: BTreeMap<Uuid, Retry> = BTreeMap::new();
    let mut configuration = None;
    let mut endpoints: Vec<Arc<websocket::RelayEndpoint>> = Vec::new();
    let mut cursor = 0_usize;
    while !stop.requested.load(Ordering::Acquire) {
        let finished: Vec<_> = workers
            .iter()
            .filter(|(_, worker)| {
                worker
                    .thread
                    .as_ref()
                    .is_none_or(|thread| thread.is_finished())
            })
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            let worker = workers.remove(&id).expect("worker exists");
            let authenticated = worker.authenticated.load(Ordering::Acquire);
            drop(worker);
            let retry = retries.entry(id).or_insert(Retry {
                count: 0,
                after: Instant::now(),
            });
            retry.count = if authenticated {
                0
            } else {
                retry.count.saturating_add(1)
            };
            let seconds = (1_u64 << retry.count.min(5)).min(30);
            let jitter = retry_jitter();
            retry.after = Instant::now() + Duration::from_secs(seconds) + jitter;
        }
        let next = storage::RelayConfig::load(&state.coven_home)
            .ok()
            .flatten()
            .filter(|config| config.enabled);
        if next != configuration {
            for worker in workers.values() {
                worker.control.cancel();
            }
            endpoints = next
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
            configuration = next;
            retries.clear();
        }
        let identity_matches =
            super::super::identity::load_existing_host_identity(&state.coven_home)
                .is_ok_and(|identity| identity.public_key_fingerprint == state.host_fingerprint);
        let devices = state
            .registry
            .reload()
            .and_then(|()| state.registry.list_status());
        if endpoints.is_empty() || !identity_matches || devices.is_err() {
            for worker in workers.values() {
                worker.control.cancel();
            }
            stop.wait();
            continue;
        }
        for worker in workers.values() {
            if authority::check_current(&state.registry, &worker.pairing, false, Utc::now())
                .is_err()
            {
                worker.control.cancel();
            }
        }
        let devices = devices.unwrap_or_default();
        retries.retain(|id, _| devices.iter().any(|device| device.id == *id));
        for offset in 0..devices.len() {
            if workers.len() >= MAX_WORKERS || stop.requested.load(Ordering::Acquire) {
                break;
            }
            let device = &devices[cursor.wrapping_add(offset) % devices.len()];
            if workers.contains_key(&device.id)
                || retries
                    .get(&device.id)
                    .is_some_and(|retry| retry.after > Instant::now())
            {
                continue;
            }
            let material = match storage::load_existing(
                &state.coven_home,
                &state.registry,
                signer.verifying_key(),
                device.id,
            ) {
                Ok(Some(material)) => material,
                _ => continue,
            };
            let attempt = retries.get(&device.id).map_or(0, |retry| retry.count);
            let endpoint = Arc::clone(&endpoints[attempt % endpoints.len()]);
            let control = Arc::new(WorkerControl::default());
            let authenticated = Arc::new(AtomicBool::new(false));
            let worker_state = Arc::clone(&state);
            let worker_signer = Arc::clone(&signer);
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
                    workers.insert(
                        device.id,
                        Worker {
                            control,
                            authenticated,
                            pairing,
                            thread: Some(thread),
                        },
                    );
                }
                Err(_) => {
                    retries.insert(
                        device.id,
                        Retry {
                            count: attempt.saturating_add(1),
                            after: Instant::now() + Duration::from_secs(30),
                        },
                    );
                }
            }
        }
        cursor = cursor.wrapping_add(1);
        stop.wait();
    }
    for worker in workers.values() {
        worker.control.cancel();
    }
    drop(workers);
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
