//! Counter rooms share ownership, retry policy and installation capacity with v1.
use super::*;
use std::sync::atomic::AtomicU8;
use storage::v2;
use transport::routing::{self, Transport};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    id: Uuid,
    generation: u64,
    counter: u64,
    endpoint: usize,
}
// A counter room's handshake progress, published by its worker.
//
// From HOST_PROOF_SENT on, the device may already have durably consumed the
// room, so cancelling the worker before it commits loses that room (#1765).
// The manager never cancels a worker in HOST_PROOF_SENT or PROOF_VERIFIED for
// routing reasons; the handshake deadline still bounds how long it is held.
// Policy changes (generation, revocation, endpoints) still cancel it.
const WAITING_FOR_PROOF: u8 = 0;
const HOST_PROOF_SENT: u8 = 1;
const PROOF_VERIFIED: u8 = 2;
const CONSUMED: u8 = 3;
// The established channel has ended; it no longer holds the device's session.
const CLOSED: u8 = 4;

/// Whether routing may cancel a worker in `state` without losing a room the
/// device could already have consumed.
fn routing_may_cancel(state: u8) -> bool {
    !matches!(state, HOST_PROOF_SENT | PROOF_VERIFIED)
}

/// Marks the channel closed when `serve` stops serving it, on any exit path.
struct MarkClosed<'a>(&'a AtomicU8);

impl Drop for MarkClosed<'_> {
    fn drop(&mut self) {
        self.0.store(CLOSED, Ordering::Release);
    }
}

struct CounterWorker {
    worker: Worker,
    consumption: Arc<AtomicU8>,
}
#[derive(Default)]
pub(super) struct Workers {
    workers: BTreeMap<Key, CounterWorker>,
    retries: BTreeMap<Key, Retry>,
}
pub(super) struct Tick<'a> {
    pub state: &'a Arc<MobileGatewayState>,
    pub signer: &'a Arc<SigningKey>,
    pub endpoints: &'a [Arc<websocket::RelayEndpoint>],
    pub budget: &'a Arc<budget::ListenerBudget>,
    pub ids: &'a [Uuid],
    pub legacy: &'a BTreeMap<Uuid, Worker>,
    pub stop: &'a Stop,
}
impl Workers {
    pub(super) fn pairing_count(&self) -> usize {
        self.workers
            .keys()
            .map(|key| key.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
    pub(super) fn contains(&self, id: Uuid) -> bool {
        self.workers.keys().any(|key| key.id == id)
    }
    pub(super) fn cancel(&self) {
        for entry in self.workers.values() {
            entry.worker.control.cancel();
        }
    }
    pub(super) fn reap(&mut self) {
        let finished: Vec<_> = self
            .workers
            .iter()
            .filter(|(_, entry)| {
                entry
                    .worker
                    .thread
                    .as_ref()
                    .is_none_or(|thread| thread.is_finished())
            })
            .map(|(key, _)| *key)
            .collect();
        for key in finished {
            let entry = self.workers.remove(&key).expect("worker exists");
            let authenticated = entry.worker.authenticated.load(Ordering::Acquire);
            drop(entry);
            let retry = self.retries.entry(key).or_insert(Retry {
                count: 0,
                after: Instant::now(),
            });
            retry.count = if authenticated {
                0
            } else {
                retry.count.saturating_add(1)
            };
            retry.after = Instant::now()
                + Duration::from_secs((1_u64 << retry.count.min(5)).min(30))
                + retry_jitter();
        }
    }
    pub(super) fn tick(&mut self, tick: Tick<'_>) {
        let Tick {
            state,
            signer,
            endpoints,
            budget,
            ids,
            legacy,
            stop,
        } = tick;
        self.reap();
        let mut snapshots = BTreeMap::new();
        for id in ids {
            if legacy.contains_key(id) {
                continue;
            }
            if !self.contains(*id)
                && legacy.len()
                    + self.pairing_count()
                    + snapshots.keys().filter(|id| !self.contains(**id)).count()
                    >= MAX_WORKERS
            {
                continue;
            }
            if let Ok(Some(snapshot)) = v2::load(
                &state.coven_home,
                &state.registry,
                signer.verifying_key(),
                *id,
                Transport::Relay,
            ) {
                snapshots.insert(*id, snapshot);
            }
        }
        // Stale generations and policy changes close both pending and live peers.
        for (key, entry) in &self.workers {
            if snapshots
                .get(&key.id)
                .is_none_or(|snapshot| snapshot.generation != key.generation)
            {
                entry.worker.control.cancel();
            }
        }
        self.retries.retain(|key, _| {
            snapshots.get(&key.id).is_some_and(|snapshot| {
                let floor = snapshot.state(Transport::Relay).floor;
                key.generation == snapshot.generation
                    && key.counter >= floor
                    && floor
                        .checked_add(routing::WINDOW)
                        .is_some_and(|end| key.counter < end)
                    && key.endpoint < endpoints.len()
            })
        });
        for (id, snapshot) in snapshots {
            let floor = snapshot.state(Transport::Relay).floor;
            let Some(end) = floor.checked_add(routing::WINDOW) else {
                for (key, entry) in &self.workers {
                    if key.id == id {
                        entry.worker.control.cancel();
                    }
                }
                continue;
            };
            let winner = self
                .workers
                .iter()
                .filter(|(key, entry)| {
                    key.id == id
                        && key.generation == snapshot.generation
                        && entry.consumption.load(Ordering::Acquire) == CONSUMED
                })
                .max_by_key(|(key, _)| key.counter)
                .map(|(key, _)| *key);
            if let Some(winner) = winner {
                // Preserve the established channel and its permit. Rebuild the
                // next window only after it exits, never beyond W held rooms.
                // A closed channel is never the winner, and a reconnect that is
                // already past the host proof is kept so its room is not lost.
                for (key, entry) in &self.workers {
                    if key.id == id
                        && *key != winner
                        && routing_may_cancel(entry.consumption.load(Ordering::Acquire))
                    {
                        entry.worker.control.cancel();
                    }
                }
                continue;
            }
            for (key, entry) in &self.workers {
                if key.id == id
                    && key.counter < floor
                    && entry.consumption.load(Ordering::Acquire) == WAITING_FOR_PROOF
                {
                    entry.worker.control.cancel();
                }
            }
            // A commit's file rename can become visible before CONSUMED. A
            // verified proof may still be committing; preserve it and wait for
            // stale cancelled sockets to exit before replenishing the window.
            if self.workers.keys().any(|key| {
                key.id == id && (key.generation != snapshot.generation || key.counter < floor)
            }) {
                continue;
            }
            for (endpoint, target) in endpoints.iter().enumerate() {
                for counter in floor..end {
                    if stop.requested.load(Ordering::Acquire) {
                        return;
                    }
                    let key = Key {
                        id,
                        generation: snapshot.generation,
                        counter,
                        endpoint,
                    };
                    if self.workers.contains_key(&key)
                        || self
                            .retries
                            .get(&key)
                            .is_some_and(|retry| retry.after > Instant::now())
                    {
                        continue;
                    }
                    let held = self
                        .workers
                        .keys()
                        .filter(|key| key.id == id && key.endpoint == endpoint)
                        .count();
                    if held >= routing::WINDOW as usize {
                        continue;
                    }
                    let Some(permit) = budget.acquire() else {
                        return;
                    };
                    let control = Arc::new(WorkerControl::default());
                    let authenticated = Arc::new(AtomicBool::new(false));
                    let consumption = Arc::new(AtomicU8::new(WAITING_FOR_PROOF));
                    let worker_state = Arc::clone(state);
                    let worker_signer = Arc::clone(signer);
                    let worker_snapshot = snapshot.clone();
                    let worker_endpoint = Arc::clone(target);
                    let worker_control = Arc::clone(&control);
                    let worker_authenticated = Arc::clone(&authenticated);
                    let worker_consumption = Arc::clone(&consumption);
                    let pairing = snapshot.material.pairing.clone();
                    match std::thread::Builder::new()
                        .name("coven-counter-peer".into())
                        .spawn(move || {
                            let _ = serve(Connection {
                                state: worker_state,
                                signer: worker_signer,
                                snapshot: worker_snapshot,
                                counter,
                                endpoint: worker_endpoint,
                                control: worker_control,
                                authenticated: worker_authenticated,
                                consumption: worker_consumption,
                            });
                        }) {
                        Ok(thread) => {
                            self.workers.insert(
                                key,
                                CounterWorker {
                                    worker: Worker {
                                        _listener: permit,
                                        control,
                                        authenticated,
                                        pairing,
                                        thread: Some(thread),
                                    },
                                    consumption,
                                },
                            );
                        }
                        Err(_) => {
                            self.retries.insert(
                                key,
                                Retry {
                                    count: 1,
                                    after: Instant::now() + Duration::from_secs(30),
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

struct Connection {
    state: Arc<MobileGatewayState>,
    signer: Arc<SigningKey>,
    snapshot: v2::Snapshot,
    counter: u64,
    endpoint: Arc<websocket::RelayEndpoint>,
    control: Arc<WorkerControl>,
    authenticated: Arc<AtomicBool>,
    consumption: Arc<AtomicU8>,
}
fn serve(connection: Connection) -> Result<()> {
    let Connection {
        state,
        signer,
        snapshot,
        counter,
        endpoint,
        control,
        authenticated,
        consumption,
    } = connection;
    let _storage_scope = state.storage_runtime.enter();
    let mut connection = state
        .connections
        .begin(transport::connection::Transport::Relay)?;
    let (room, credential) = snapshot.rendezvous(Transport::Relay, counter)?;
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
    connection.authenticating()?;
    let mut channel = routing::channel::accept(
        records,
        routing::channel::HostAdmission {
            pairing: &snapshot.material.pairing,
            private: &snapshot.material.noise_private,
            host: &signer,
            context: &room,
            direct: false,
            proof: snapshot.state(Transport::Relay).proof(counter)?,
        },
        |pairing, _| {
            v2::check_channel(
                &state.coven_home,
                &state.registry,
                pairing,
                Transport::Relay,
                Some(snapshot.generation),
            )
        },
        |proof, deadline| {
            let current = v2::refresh_proof(
                &state.coven_home,
                &state.registry,
                &snapshot.material.pairing,
                Transport::Relay,
                proof,
                deadline,
            )?;
            // The host proof is signed and sent next. From here the device
            // may durably consume this room before the host commits it.
            consumption.store(HOST_PROOF_SENT, Ordering::Release);
            Ok(current)
        },
        |proof| {
            // Publish verification before storage can make the new floor visible.
            // The manager preserves a possible in-flight commit but promptly
            // cancels stale rooms that have never verified a device proof.
            consumption.store(PROOF_VERIFIED, Ordering::Release);
            v2::consume_verified(&state.coven_home, &state.registry, proof)?;
            consumption.store(CONSUMED, Ordering::Release);
            Ok(())
        },
    )?;
    let _closed = MarkClosed(&consumption);
    connection.online()?;
    authenticated.store(true, Ordering::Release);
    while channel.is_open() {
        serve_transport_with_state(&state, &mut channel)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile_memory::gateway::transport_tests::Fixture;
    use crate::mobile_memory::transport::test_support::Relay;
    use p256::pkcs8::DecodePrivateKey;
    fn admitted_pair() -> (Fixture, Arc<SigningKey>, tempfile::TempDir) {
        let fixture = Fixture::new();
        let identity =
            crate::mobile_memory::identity::load_existing_host_identity(&fixture.state.coven_home)
                .unwrap();
        let host = Arc::new(SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap());
        let id = fixture.pairing.device_id;
        let handover = v2::prepare_direct(
            &fixture.state.coven_home,
            &fixture.state.registry,
            &host,
            id,
        )
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let ack = v2::device::adopt_direct(
            home.path(),
            handover,
            host.verifying_key(),
            id,
            fixture.device.verifying_key(),
        )
        .unwrap();
        v2::acknowledge_direct(
            &fixture.state.coven_home,
            &fixture.state.registry,
            &host,
            id,
            &ack,
        )
        .unwrap();
        (fixture, host, home)
    }
    #[test]
    fn counter_workers_share_the_cap_across_endpoints_and_lowering_does_not_evict() {
        let (fixture, host, _device_home) = admitted_pair();
        let id = fixture.pairing.device_id;
        let relays = [Relay::start_literal(), Relay::start_literal()];
        let endpoints: Vec<_> = relays
            .iter()
            .map(|relay| Arc::new(relay.configuration().parse().unwrap()))
            .collect();
        let state = Arc::new(fixture.state);
        let budget = budget::ListenerBudget::new(5).unwrap();
        let mut workers = Workers::default();
        let legacy = BTreeMap::new();
        let stop = Stop::default();
        workers.tick(Tick {
            state: &state,
            signer: &host,
            endpoints: &endpoints,
            budget: &budget,
            ids: &[id],
            legacy: &legacy,
            stop: &stop,
        });
        assert_eq!(workers.workers.len(), 5);
        assert_eq!(
            workers
                .workers
                .keys()
                .filter(|key| key.endpoint == 0)
                .count(),
            4
        );
        assert_eq!(
            workers
                .workers
                .keys()
                .filter(|key| key.endpoint == 1)
                .count(),
            1
        );
        assert!(budget.acquire().is_none());
        budget.set_limit(2).unwrap();
        workers.tick(Tick {
            state: &state,
            signer: &host,
            endpoints: &endpoints,
            budget: &budget,
            ids: &[id],
            legacy: &legacy,
            stop: &stop,
        });
        // A real connection may finish independently and be reaped by tick.
        // Lowering must not cancel any listener still owned after that reap.
        assert!(workers.workers.len() <= 5);
        assert!(workers
            .workers
            .values()
            .all(|entry| !entry.worker.control.0.lock().unwrap().0));
        workers.cancel();
        // Cancellation alone cannot free permits before the sockets are joined.
        assert!(budget.acquire().is_none());
        drop(workers);
        let first = budget.acquire().unwrap();
        let second = budget.acquire().unwrap();
        assert!(budget.acquire().is_none());
        drop((first, second));
    }
    #[test]
    fn advancing_floor_retires_unconsumed_stale_listeners_without_waiting_for_challenge_expiry() {
        let (fixture, host, _device_home) = admitted_pair();
        let id = fixture.pairing.device_id;
        let relay = Relay::start_literal();
        let endpoints = vec![Arc::new(relay.configuration().parse().unwrap())];
        let state = Arc::new(fixture.state);
        let budget = budget::ListenerBudget::new(4).unwrap();
        let mut workers = Workers::default();
        let legacy = BTreeMap::new();
        let stop = Stop::default();
        workers.tick(Tick {
            state: &state,
            signer: &host,
            endpoints: &endpoints,
            budget: &budget,
            ids: &[id],
            legacy: &legacy,
            stop: &stop,
        });
        let stale = Arc::clone(
            &workers
                .workers
                .get(&Key {
                    id,
                    generation: 0,
                    counter: 0,
                    endpoint: 0,
                })
                .unwrap()
                .worker
                .control,
        );
        v2::resync_direct(
            &state.coven_home,
            &state.registry,
            &host,
            id,
            Transport::Relay,
            0,
            1,
        )
        .unwrap();
        workers.tick(Tick {
            state: &state,
            signer: &host,
            endpoints: &endpoints,
            budget: &budget,
            ids: &[id],
            legacy: &legacy,
            stop: &stop,
        });
        assert!(
            stale.0.lock().unwrap().0,
            "unconsumed stale room retained until its challenge deadline"
        );
        workers.cancel();
    }

    /// One relay, four listener rooms (counters 0..WINDOW) for the pair, and
    /// a manager pass closure. Rooms only listen; tests publish handshake
    /// states directly so the close, reconnect and pass are strictly ordered.
    struct RoomHarness {
        _device_home: tempfile::TempDir,
        _relay: Relay,
        state: Arc<MobileGatewayState>,
        host: Arc<SigningKey>,
        endpoints: Vec<Arc<websocket::RelayEndpoint>>,
        budget: Arc<budget::ListenerBudget>,
        workers: Workers,
        id: Uuid,
    }

    /// Run `test` against a fresh harness. The fixture is kept alive in this
    /// frame: moving its state out would otherwise drop its Coven home.
    fn with_rooms(test: impl FnOnce(&mut RoomHarness)) {
        let (fixture, host, device_home) = admitted_pair();
        let id = fixture.pairing.device_id;
        let relay = Relay::start_literal();
        let endpoints = vec![Arc::new(relay.configuration().parse().unwrap())];
        let mut rooms = RoomHarness {
            _device_home: device_home,
            _relay: relay,
            state: Arc::new(fixture.state),
            host,
            endpoints,
            budget: budget::ListenerBudget::new(routing::WINDOW as usize).unwrap(),
            workers: Workers::default(),
            id,
        };
        rooms.pass();
        assert_eq!(rooms.workers.workers.len(), routing::WINDOW as usize);
        test(&mut rooms);
        rooms.workers.cancel();
    }

    impl RoomHarness {
        fn pass(&mut self) {
            let legacy = BTreeMap::new();
            let stop = Stop::default();
            self.workers.tick(Tick {
                state: &self.state,
                signer: &self.host,
                endpoints: &self.endpoints,
                budget: &self.budget,
                ids: &[self.id],
                legacy: &legacy,
                stop: &stop,
            });
        }

        fn room(&self, counter: u64) -> (Arc<AtomicU8>, Arc<WorkerControl>) {
            let entry = self
                .workers
                .workers
                .get(&Key {
                    id: self.id,
                    generation: 0,
                    counter,
                    endpoint: 0,
                })
                .unwrap_or_else(|| panic!("room {counter} has no worker"));
            (
                Arc::clone(&entry.consumption),
                Arc::clone(&entry.worker.control),
            )
        }
    }

    fn cancelled(control: &WorkerControl) -> bool {
        control.0.lock().unwrap().0
    }

    #[test]
    fn a_live_winner_never_cancels_a_next_room_worker_past_the_host_proof() {
        for in_flight in [HOST_PROOF_SENT, PROOF_VERIFIED] {
            with_rooms(|rooms| {
                let (established, established_control) = rooms.room(0);
                let (next, next_control) = rooms.room(1);
                let (_, spare_control) = rooms.room(2);
                // Room 0 is the established channel. Its socket has just
                // closed, but the manager has not observed that yet, and the
                // device has reconnected on room 1 and may have consumed it.
                established.store(CONSUMED, Ordering::Release);
                next.store(in_flight, Ordering::Release);
                rooms.pass();
                assert!(
                    !cancelled(&next_control),
                    "state {in_flight}: the reconnect past the host proof was cancelled"
                );
                assert!(!cancelled(&established_control));
                assert!(
                    cancelled(&spare_control),
                    "an unproven spare room must still yield to the established channel"
                );
            });
        }
    }

    #[test]
    fn closing_the_winner_then_reconnecting_keeps_the_next_room_through_its_commit() {
        with_rooms(|rooms| {
            let (established, established_control) = rooms.room(0);
            let (next, next_control) = rooms.room(1);
            let (_, spare_control) = rooms.room(2);
            // 1. Room 0 was established, and its channel has now closed.
            established.store(CONSUMED, Ordering::Release);
            established.store(CLOSED, Ordering::Release);
            // 2. The device reconnects on room 1 and receives the host proof.
            next.store(HOST_PROOF_SENT, Ordering::Release);
            // 3. Manager passes during the reconnect's handshake. A closed channel
            //    is not established, so neither the reconnect nor the spare room
            //    the device could fall back to is cancelled.
            rooms.pass();
            assert!(
                !cancelled(&next_control),
                "closed winner cancelled the reconnect"
            );
            assert!(
                !cancelled(&spare_control),
                "closed winner cancelled a spare room"
            );
            next.store(PROOF_VERIFIED, Ordering::Release);
            rooms.pass();
            assert!(
                !cancelled(&next_control),
                "verified reconnect cancelled before commit"
            );
            // 4. The reconnect commits and becomes the established channel; only
            //    then do the closed and unproven rooms yield to it.
            next.store(CONSUMED, Ordering::Release);
            rooms.pass();
            assert!(
                !cancelled(&next_control),
                "the committed room was cancelled"
            );
            assert!(cancelled(&established_control));
            assert!(cancelled(&spare_control));
        });
    }

    #[test]
    fn routing_may_cancel_only_outside_the_handshake_window() {
        assert!(routing_may_cancel(WAITING_FOR_PROOF));
        assert!(!routing_may_cancel(HOST_PROOF_SENT));
        assert!(!routing_may_cancel(PROOF_VERIFIED));
        assert!(routing_may_cancel(CONSUMED));
        assert!(routing_may_cancel(CLOSED));
    }

    #[test]
    fn mark_closed_publishes_closed_on_every_exit() {
        let consumption = AtomicU8::new(CONSUMED);
        let result: Result<()> = (|| {
            let _closed = MarkClosed(&consumption);
            anyhow::bail!("transport failed")
        })();
        assert!(result.is_err());
        assert_eq!(consumption.load(Ordering::Acquire), CLOSED);
    }
}
