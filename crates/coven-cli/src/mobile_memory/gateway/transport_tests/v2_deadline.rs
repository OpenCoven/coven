//! Daemon-level adversarial evidence for the v2 handshake deadline (#1683).
//!
//! Every case runs the real daemon (`serve_forever`) as a child process against
//! the production relay router. The daemon's relay manager opens the v2 counter
//! rooms itself; the test never calls the handshake's host side. The listener
//! limit equals one routing window, so the four counter rooms of the one paired
//! device use the whole installation budget and a room can be opened again only
//! with a permit that an earlier room returned.
//!
//! What the test can see from outside the daemon:
//!
//! - **When a room's challenge started.** The relay fixture records every byte
//!   it reads after TLS, including each `room=…&role=host` upgrade. A watcher
//!   polls that record. A hosting's `after` is the start of the last poll that
//!   did not contain the upgrade, so the daemon began its 30-second budget
//!   strictly later than `after`.
//! - **When the daemon left a room.** The relay closes the opposite peer as
//!   soon as the host's socket is gone, so the adversary's connection observes
//!   the departure, never earlier than it happened.
//! - **Counters.** The host floor is read from the daemon's durable routing
//!   state and the device counter from the device's durable state.
//!
//! No production deadline is shortened. The only test-chosen intervals are the
//! adversary's own delays ([`LATE_RECORD`], [`LATER_RECORD`]) and hang guards.
//! Everything runs on loopback, so there is no packet loss or reordering below
//! the record layer.
use super::*;
use crate::mobile_memory::transport::{
    routing::{self, DeviceState, Transport},
    storage::v2,
    test_support::Relay,
    websocket, Records,
};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

mod matrix;
mod peer;
use peer::{Insider, Outbound};

/// The production budget in `transport::HANDSHAKE_BUDGET`, restated as the
/// contract under test. It is compared with observations only and is never
/// passed into production code.
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(30);
/// Discriminating bound, not a promptness claim. A budget that restarted at
/// the adversary's late record would hold the room for at least
/// `LATE_RECORD + HANDSHAKE_BUDGET` = 42 seconds; this sits midway between
/// that and the 30-second contract.
const DEADLINE_SLACK: Duration = Duration::from_secs(6);
/// Test-only: how long after a hosting is observed the adversary lets its
/// first late record through.
const LATE_RECORD: Duration = Duration::from_secs(12);
/// Test-only: when the adversary lets a second late record through.
const LATER_RECORD: Duration = Duration::from_secs(20);
/// Discriminating bound between a rejection on arrival (well under a second
/// on loopback) and the 30-second budget running out.
const REJECTED_ON_ARRIVAL: Duration = Duration::from_secs(15);
/// The documented `daemon stop` contract (`daemon.rs`).
const STOP_BUDGET: Duration = Duration::from_secs(2);
/// Discriminating bound for the budget returning every permit: reopening
/// takes the manager's retry backoff (2 or 4 seconds here) plus one tick,
/// while a room opened with a permit that only a later expiry frees would
/// arrive a further 30 seconds on.
const REOPENED_TOGETHER: Duration = Duration::from_secs(15);
/// Hang guard only, far above anything load can produce.
const HANG_GUARD: Duration = Duration::from_secs(120);
/// Counter rooms the watcher follows; the cases never move past room 5.
const WATCHED_ROOMS: u64 = 8;

const CHILD_ENV: &str = "COVEN_TEST_V2_DEADLINE_DAEMON_CHILD";
const CHILD_TEST: &str = "mobile_memory::gateway::transport_tests::v2_deadline::daemon_child_entry";

/// Re-exec target that becomes the daemon. It is not a case: without the child
/// marker it does nothing.
#[test]
fn daemon_child_entry() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let home = crate::coven_home_dir().unwrap();
    crate::daemon::serve_forever(&home, Utc::now().to_rfc3339(), None, &[]).unwrap();
}

/// One line per observation on the real stderr, so hosted logs show what each
/// case measured even when it passes.
fn note(case: &str, message: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "v2-deadline {case}: {message}");
}

struct OwnedDaemon(Child);
impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// One appearance of the daemon as host in a counter room.
#[derive(Clone, Copy, Debug)]
struct Hosting {
    /// The daemon's upgrade reached the relay strictly after this instant.
    after: Instant,
    /// The watcher had seen the upgrade by this instant.
    by: Instant,
}

struct Watch {
    hostings: Arc<Mutex<Vec<Vec<Hosting>>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Watch {
    fn start(relay: Arc<Relay>, rooms: Vec<[u8; 32]>) -> Self {
        let needles: Vec<Vec<u8>> = rooms
            .iter()
            .map(|room| format!("room={}&role=host", URL_SAFE_NO_PAD.encode(room)).into_bytes())
            .collect();
        let hostings = Arc::new(Mutex::new(vec![Vec::new(); needles.len()]));
        let stop = Arc::new(AtomicBool::new(false));
        let (observed, stopping) = (Arc::clone(&hostings), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            let mut absent = Instant::now();
            while !stopping.load(Ordering::Acquire) {
                let started = Instant::now();
                let captured = relay.captured();
                let by = Instant::now();
                for (room, needle) in needles.iter().enumerate() {
                    let count = captured
                        .windows(needle.len())
                        .filter(|part| part == needle)
                        .count();
                    let mut observed = observed.lock().unwrap();
                    while observed[room].len() < count {
                        observed[room].push(Hosting { after: absent, by });
                    }
                }
                absent = started;
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Self {
            hostings,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Journey {
    case: &'static str,
    started: Instant,
    // Field order is drop order: stop observing and the daemon before the
    // relay and the homes they use go away.
    watch: Watch,
    daemon: Mutex<Option<OwnedDaemon>>,
    relay: Arc<Relay>,
    pairing: Pairing,
    host: SigningKey,
    device_home: tempfile::TempDir,
    fixture: Fixture,
}

impl Journey {
    fn start(case: &'static str) -> Self {
        let started = Instant::now();
        // Keep the daemon's Unix socket path below the macOS pathname limit.
        let fixture = Fixture::with_home(
            tempfile::Builder::new()
                .prefix("coven-v2-deadline-")
                .tempdir_in("/tmp")
                .unwrap(),
        );
        let home = fixture.state.coven_home.clone();
        let relay = Arc::new(Relay::start());
        storage::RelayConfig {
            listener_limit: usize::try_from(routing::WINDOW).unwrap(),
            version: 1,
            enabled: true,
            endpoints: vec![relay.configuration()],
        }
        .save(&home)
        .unwrap();
        let identity = super::super::super::identity::load_existing_host_identity(&home).unwrap();
        let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
        let id = fixture.pairing.device_id;
        let handover = v2::prepare_direct(&home, &fixture.state.registry, &host, id).unwrap();
        let device_home = tempfile::tempdir().unwrap();
        let acknowledgment = v2::device::adopt_direct(
            device_home.path(),
            handover,
            host.verifying_key(),
            id,
            fixture.device.verifying_key(),
        )
        .unwrap();
        v2::acknowledge_direct(&home, &fixture.state.registry, &host, id, &acknowledgment).unwrap();
        let snapshot = v2::load(
            &home,
            &fixture.state.registry,
            host.verifying_key(),
            id,
            Transport::Relay,
        )
        .unwrap()
        .unwrap();
        assert_eq!(snapshot.floors, [0, 0]);
        let rooms = (0..WATCHED_ROOMS)
            .map(|counter| snapshot.rendezvous(Transport::Relay, counter).unwrap().0)
            .collect();
        let watch = Watch::start(Arc::clone(&relay), rooms);

        let user = home.join("isolated-user");
        std::fs::create_dir(&user).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_ENV, "1")
            .env("COVEN_HOME", &home)
            .env("HOME", &user)
            .env("TMPDIR", &home)
            .current_dir(&home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let daemon = OwnedDaemon(command.spawn().unwrap());
        Self {
            case,
            started,
            watch,
            daemon: Mutex::new(Some(daemon)),
            relay,
            pairing: snapshot.material.pairing.clone(),
            host,
            device_home,
            fixture,
        }
    }

    fn home(&self) -> &Path {
        &self.fixture.state.coven_home
    }

    fn id(&self) -> Uuid {
        self.fixture.pairing.device_id
    }

    fn assert_daemon_running(&self, waiting_for: &str) {
        let mut daemon = self.daemon.lock().unwrap();
        let daemon = daemon.as_mut().expect("daemon already stopped");
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "{}: daemon exited while waiting for {waiting_for}",
            self.case
        );
    }

    /// The `nth` time (from 1) the daemon opened counter room `counter`.
    fn hosting(&self, counter: u64, nth: usize) -> Hosting {
        self.hosting_within(counter, nth, HANG_GUARD)
            .unwrap_or_else(|| {
                panic!(
                    "{}: daemon did not open room {counter} for hosting {nth}",
                    self.case
                )
            })
    }

    fn hosting_within(&self, counter: u64, nth: usize, limit: Duration) -> Option<Hosting> {
        let started = Instant::now();
        loop {
            let seen = self.watch.hostings.lock().unwrap()[usize::try_from(counter).unwrap()]
                .get(nth - 1)
                .copied();
            if seen.is_some() {
                return seen;
            }
            self.assert_daemon_running("a counter room");
            if started.elapsed() >= limit {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn snapshot(&self) -> v2::Snapshot {
        v2::load(
            self.home(),
            &self.fixture.state.registry,
            self.host.verifying_key(),
            self.id(),
            Transport::Relay,
        )
        .unwrap()
        .unwrap()
    }

    /// `[direct, relay]` floors from the daemon's durable routing state.
    fn host_floors(&self) -> [u64; 2] {
        self.snapshot().floors
    }

    fn device_attempt(&self) -> v2::device::Attempt {
        v2::device::begin(
            self.device_home.path(),
            self.host.verifying_key(),
            self.id(),
            self.fixture.device.verifying_key(),
            Transport::Relay,
        )
        .unwrap()
    }

    /// `(next, floor)` from the device's durable routing state.
    fn device_counters(&self) -> (u64, u64) {
        let DeviceState { next, floor, .. } = self.device_attempt().state();
        (next, floor)
    }

    fn room(&self, counter: u64) -> [u8; 32] {
        self.snapshot()
            .rendezvous(Transport::Relay, counter)
            .unwrap()
            .0
    }

    /// Joins a counter room with routing material alone, as a broker or any
    /// holder of the room credential could.
    fn join(&self, counter: u64) -> websocket::WebSocketRecords {
        let (room, credential) = self
            .snapshot()
            .rendezvous(Transport::Relay, counter)
            .unwrap();
        self.relay
            .connect(websocket::Role::Client, &room, &credential)
            .unwrap()
    }

    /// Runs the production device implementation against the room it selects
    /// itself, with the adversary between it and the relay.
    fn device_through(
        &self,
        expected_room: u64,
        script: &[Outbound],
        swap_first_flight: bool,
        origin: Instant,
    ) -> (Result<()>, peer::Shared) {
        let mut attempt = self.device_attempt();
        assert_eq!(
            attempt.state().next,
            expected_room,
            "{}: device chose an unexpected room",
            self.case
        );
        let (room, credential) = attempt.rendezvous().unwrap();
        assert_eq!(room, self.room(expected_room));
        let connection = self
            .relay
            .connect(websocket::Role::Client, &room, &credential)
            .unwrap();
        let (hostile, shared) = peer::hostile(connection, script, swap_first_flight, origin);
        let result = attempt
            .connect(hostile, &self.fixture.device)
            .map(|_channel| ());
        (result, shared)
    }

    /// (1): the daemon left the room when the budget that began with this
    /// hosting ran out, whatever the peer sent in between.
    fn assert_original_deadline(&self, hosting: Hosting, departed: Instant) {
        let held = departed.duration_since(hosting.after);
        note(
            self.case,
            format_args!("daemon left the room {held:?} after its challenge began"),
        );
        assert!(
            held >= HANDSHAKE_BUDGET,
            "{}: daemon left the room after {held:?}, before its 30-second budget",
            self.case
        );
        assert!(
            held < HANDSHAKE_BUDGET + DEADLINE_SLACK,
            "{}: daemon held the room for {held:?}, past its original 30-second budget",
            self.case
        );
    }

    /// The daemon refused the record when it arrived, not when time ran out.
    fn assert_rejected_on_arrival(&self, what: &str, sent: Instant, departed: Instant) {
        let waited = departed.duration_since(sent);
        note(
            self.case,
            format_args!("daemon left the room {waited:?} after {what}"),
        );
        assert!(
            waited < REJECTED_ON_ARRIVAL,
            "{}: daemon kept the room for {waited:?} after {what}",
            self.case
        );
    }

    /// (2): after the daemon left the adversary's room at `departed`, it holds
    /// all four rooms of the window at once again. The budget is exactly four
    /// permits, so that needs every permit back, including the one the
    /// adversary's room held. `nth` names the expected hosting of each room
    /// from `floor` upward; the adversary's room is the first.
    fn assert_window_reopened(&self, floor: u64, nth: [usize; 4], departed: Instant) {
        let mut hostings = Vec::new();
        for (offset, nth) in nth.into_iter().enumerate() {
            let counter = floor + u64::try_from(offset).unwrap();
            let remaining = REOPENED_TOGETHER.saturating_sub(departed.elapsed());
            let Some(hosting) = self.hosting_within(counter, nth, remaining) else {
                panic!(
                    "{}: room {counter} was not reopened within {REOPENED_TOGETHER:?} of the daemon \
                     leaving; the listener budget did not get its permit back",
                    self.case
                );
            };
            hostings.push(hosting);
        }
        assert!(
            hostings[0].after >= departed,
            "{}: the adversary's room was reopened before the daemon left it",
            self.case
        );
        // Nothing cancels an idle room here, so each hosting lasts its full
        // budget and four that began within one budget are held together.
        let earliest = hostings.iter().map(|hosting| hosting.after).min().unwrap();
        let latest = hostings.iter().map(|hosting| hosting.by).max().unwrap();
        let spread = latest.duration_since(earliest);
        assert!(
            spread < HANDSHAKE_BUDGET - DEADLINE_SLACK,
            "{}: the four rooms were opened {spread:?} apart, not held together",
            self.case
        );
        note(
            self.case,
            format_args!(
                "all four rooms held again {:?} after the daemon left",
                latest.duration_since(departed)
            ),
        );
    }

    /// (4): `daemon stop` keeps its two-second contract and the daemon exits
    /// gracefully while `held` is an unauthenticated peer inside a challenge.
    /// The peer's room must close because of the stop, not its own budget.
    fn stop_while_held(&self, held: &mut websocket::WebSocketRecords, hosting: Hosting) {
        self.assert_daemon_running("stop");
        let started = Instant::now();
        assert!(
            started.duration_since(hosting.by) < REJECTED_ON_ARRIVAL,
            "{}: the held challenge is too old to tell a stop from its deadline",
            self.case
        );
        assert!(crate::daemon::stop_background_server(self.home()).unwrap());
        let elapsed = started.elapsed();
        assert!(
            elapsed < STOP_BUDGET,
            "{}: daemon stop exceeded its two-second contract: {elapsed:?}",
            self.case
        );
        let (departed, records) = peer::await_host_departure(held);
        assert!(
            records.is_empty(),
            "{}: daemon sent {} records while stopping",
            self.case,
            records.len()
        );
        let closed = departed.duration_since(started);
        assert!(
            closed < REJECTED_ON_ARRIVAL,
            "{}: the held room closed {closed:?} after stop, at its own deadline",
            self.case
        );
        note(
            self.case,
            format_args!("daemon stop returned in {elapsed:?}; held room closed {closed:?} after"),
        );
        self.reap();
    }

    /// `daemon stop` with no peer inside a challenge.
    fn stop_idle(&self) {
        self.assert_daemon_running("stop");
        let started = Instant::now();
        assert!(crate::daemon::stop_background_server(self.home()).unwrap());
        let elapsed = started.elapsed();
        assert!(
            elapsed < STOP_BUDGET,
            "{}: daemon stop exceeded its two-second contract: {elapsed:?}",
            self.case
        );
        self.reap();
    }

    /// Requires a graceful exit, so a forced kill cannot pass for cancellation.
    fn reap(&self) {
        let mut daemon = self.daemon.lock().unwrap().take().expect("daemon running");
        note(
            self.case,
            format_args!("case took {:?} up to daemon stop", self.started.elapsed()),
        );
        let limit = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = daemon.0.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "{}: daemon did not stop gracefully: {status}",
                    self.case
                );
                return;
            }
            assert!(
                Instant::now() < limit,
                "{}: daemon stayed alive after stop",
                self.case
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_counters(&self, when: &str, host_floors: [u64; 2], device: (u64, u64)) {
        assert_eq!(
            self.host_floors(),
            host_floors,
            "{}: host floors {when}",
            self.case
        );
        assert_eq!(
            self.device_counters(),
            device,
            "{}: device (next, floor) {when}",
            self.case
        );
    }
}

fn sleep_until(instant: Instant) {
    if let Some(remaining) = instant.checked_duration_since(Instant::now()) {
        std::thread::sleep(remaining);
    }
}

/// Control for the observations the adversarial cases rely on. A genuine
/// exchange moves the host floor and the device counter past exactly the room
/// it proved, so an unmoved counter elsewhere is a real observation. The
/// independent device implementation is admitted when it follows the protocol,
/// so its refusals elsewhere are not encoding mistakes.
#[test]
fn authenticated_exchanges_are_what_move_the_host_floor_and_the_device_counter() {
    let journey = Journey::start("control");
    journey.hosting(0, 1);
    journey.assert_counters("before any exchange", [0, 0], (0, 0));

    let (result, _) = journey.device_through(0, &[], false, Instant::now());
    result.expect("genuine device was refused");
    journey.assert_counters("after the genuine exchange", [0, 1], (1, 0));

    // The window is now rooms 1..5. Room 4 is the one room the daemon cannot
    // have opened before the floor moved, so its first hosting is new whether
    // or not the manager cancelled rooms 1 to 3 while the genuine channel was up.
    let hosting = journey.hosting(4, 1);
    let mut insider = Insider::open(journey.join(4), &journey.pairing, &journey.room(4)).unwrap();
    let (proof, signature) = insider.challenge(&journey.fixture.device).unwrap();
    assert_eq!(
        (proof.generation, proof.host_floor, proof.counter),
        (journey.snapshot().generation, 1, 4)
    );
    insider.send(&signature).unwrap();
    assert_eq!(insider.receive().unwrap(), b"host-finished-v2");
    insider.send(b"device-finished-v2").unwrap();
    // The floor is durable before the daemon sends its completion record, and
    // proving room 4 retires rooms 1 to 3 with it.
    assert_eq!(journey.host_floors(), [0, 5]);
    note(
        journey.case,
        format_args!(
            "independent device admitted {:?} after room 4 opened",
            hosting.by.elapsed()
        ),
    );

    journey.stop_idle();
    // The independent device keeps no durable state, so the device file still
    // records only the genuine exchange.
    journey.assert_counters("after stop", [0, 5], (1, 0));
}

/// Withhold: the device's possession proof never reaches the daemon.
#[test]
fn withheld_possession_proof_expires_at_the_original_deadline_without_a_host_counter_move() {
    let journey = Journey::start("withhold");
    let script = [Outbound::Forward, Outbound::Forward, Outbound::Withhold];

    let first = journey.hosting(0, 1);
    let (result, tap) = journey.device_through(0, &script, false, first.by);
    assert!(
        result.is_err(),
        "withhold: device completed without its proof"
    );
    assert_eq!(peer::outbound(&tap).len(), 3);
    assert_eq!(peer::inbound(&tap), 3, "withhold: host proof never arrived");
    // The device verified an authentic host proof and durably advanced before
    // it offered its own. The host saw no device proof, so its floor stays.
    journey.assert_counters("while the proof is withheld", [0, 0], (1, 0));
    let mut kept = peer::kept_connection(&tap);
    let (departed, records) = peer::await_host_departure(&mut kept);
    assert!(records.is_empty());
    journey.assert_original_deadline(first, departed);
    journey.assert_counters("after the deadline", [0, 0], (1, 0));
    journey.assert_window_reopened(0, [2; 4], departed);

    let second = journey.hosting(1, 2);
    let (result, tap) = journey.device_through(1, &script, false, second.by);
    assert!(result.is_err());
    assert_eq!(peer::inbound(&tap), 3);
    let mut kept = peer::kept_connection(&tap);
    journey.stop_while_held(&mut kept, second);
    journey.assert_counters("after stop", [0, 0], (2, 0));
}

/// Delay: the handshake records trickle in late and the proof never follows.
/// The daemon accepts a record 20 seconds into its budget and still leaves at
/// 30 seconds, not 30 seconds after that record.
#[test]
fn delayed_handshake_records_do_not_extend_the_original_deadline() {
    let journey = Journey::start("delay");

    let first = journey.hosting(0, 1);
    let (result, tap) = journey.device_through(
        0,
        &[
            Outbound::At(LATE_RECORD),
            Outbound::At(LATER_RECORD),
            Outbound::Withhold,
        ],
        false,
        first.by,
    );
    assert!(result.is_err());
    // Three host records prove the daemon accepted both late records: its
    // Noise reply and nonce answer the first, its proof answers the second.
    assert_eq!(
        peer::inbound(&tap),
        3,
        "delay: daemon refused a late record"
    );
    assert!(first.by.elapsed() >= LATER_RECORD);
    journey.assert_counters("after the late records", [0, 0], (1, 0));
    let mut kept = peer::kept_connection(&tap);
    let (departed, records) = peer::await_host_departure(&mut kept);
    assert!(records.is_empty());
    journey.assert_original_deadline(first, departed);
    // The authentic proof the adversary held arrives after the deadline.
    let _ = kept.send(
        &peer::outbound(&tap)[2],
        Instant::now() + Duration::from_secs(5),
    );
    journey.assert_window_reopened(0, [2; 4], departed);
    journey.assert_counters("after the late proof", [0, 0], (1, 0));

    // Stop lands while the adversary is still holding the first record back.
    let second = journey.hosting(1, 2);
    std::thread::scope(|scope| {
        let (announce, announced) = std::sync::mpsc::channel();
        let journey = &journey;
        let device = scope.spawn(move || {
            let mut attempt = journey.device_attempt();
            let (room, credential) = attempt.rendezvous().unwrap();
            let connection = journey
                .relay
                .connect(websocket::Role::Client, &room, &credential)
                .unwrap();
            let (hostile, shared) =
                peer::hostile(connection, &[Outbound::At(HANG_GUARD)], false, second.by);
            announce.send(Arc::clone(&shared)).unwrap();
            let result = attempt
                .connect(hostile, &journey.fixture.device)
                .map(|_| ());
            (result, shared)
        });
        let shared = announced.recv_timeout(HANG_GUARD).unwrap();
        peer::await_delaying(&shared, 0);
        let started = Instant::now();
        assert!(crate::daemon::stop_background_server(journey.home()).unwrap());
        let elapsed = started.elapsed();
        assert!(
            elapsed < STOP_BUDGET,
            "delay: daemon stop exceeded its two-second contract: {elapsed:?}"
        );
        peer::release(&shared);
        let (result, shared) = device.join().unwrap();
        assert!(result.is_err(), "delay: device completed after stop");
        let (departed, records) = peer::await_host_departure(&mut peer::kept_connection(&shared));
        assert!(records.is_empty());
        let closed = departed.duration_since(started);
        assert!(
            closed < REJECTED_ON_ARRIVAL,
            "delay: the held room closed {closed:?} after stop, at its own deadline"
        );
        note(
            journey.case,
            format_args!("daemon stop returned in {elapsed:?}; held room closed {closed:?} after"),
        );
    });
    journey.reap();
    journey.assert_counters("after stop", [0, 0], (1, 0));
}

/// Replay: records the daemon has already seen are sent again.
#[test]
fn replayed_handshake_records_are_refused_or_expire_at_the_original_deadline() {
    let journey = Journey::start("replay");

    // In-session: the adversary repeats the device's nonce record where the
    // daemon expects the proof it is holding back.
    let first = journey.hosting(0, 1);
    let (result, tap) = journey.device_through(
        0,
        &[Outbound::Forward, Outbound::Forward, Outbound::Withhold],
        false,
        first.by,
    );
    assert!(result.is_err());
    assert_eq!(peer::inbound(&tap), 3);
    let recorded = peer::outbound(&tap);
    let mut kept = peer::kept_connection(&tap);
    let sent = Instant::now();
    kept.send(&recorded[1], Instant::now() + Duration::from_secs(30))
        .unwrap();
    let (departed, records) = peer::await_host_departure(&mut kept);
    assert!(
        records.is_empty(),
        "replay: daemon answered a repeated record"
    );
    journey.assert_rejected_on_arrival("the repeated nonce record", sent, departed);
    journey.assert_counters("after the repeated record", [0, 0], (1, 0));

    // Across hostings: the recorded first flight is a valid Noise message for
    // this room, so a later hosting accepts it and answers. Sent 12 seconds
    // into that hosting's budget, it buys no extra time.
    let second = journey.hosting(0, 2);
    let mut replayer = journey.join(0);
    sleep_until(second.by + LATE_RECORD);
    replayer
        .send(&recorded[0], Instant::now() + Duration::from_secs(30))
        .unwrap();
    for record in ["Noise reply", "host nonce"] {
        let answer = replayer
            .receive(Instant::now() + Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("replay: daemon refused the replayed first flight"));
        assert_eq!(answer.len(), 48, "replay: unexpected {record}");
    }
    let (departed, records) = peer::await_host_departure(&mut replayer);
    assert!(records.is_empty());
    journey.assert_original_deadline(second, departed);
    journey.assert_counters("after the replayed first flight", [0, 0], (1, 0));
    journey.assert_window_reopened(0, [3, 2, 2, 2], departed);

    let third = journey.hosting(0, 3);
    let mut replayer = journey.join(0);
    replayer
        .send(&recorded[0], Instant::now() + Duration::from_secs(30))
        .unwrap();
    for _ in 0..2 {
        replayer
            .receive(Instant::now() + Duration::from_secs(10))
            .unwrap();
    }
    journey.stop_while_held(&mut replayer, third);
    journey.assert_counters("after stop", [0, 0], (1, 0));
}

/// Reorder: records arrive in an order the protocol does not allow.
#[test]
fn reordered_handshake_and_proof_records_are_refused_or_expire_at_the_original_deadline() {
    let journey = Journey::start("reorder");

    // On the network: the daemon's two handshake records reach the device in
    // the wrong order, 12 seconds into the budget. The device aborts and the
    // daemon is left waiting for a nonce that will not come.
    let first = journey.hosting(0, 1);
    let (result, tap) = journey.device_through(0, &[Outbound::At(LATE_RECORD)], true, first.by);
    assert!(
        result.is_err(),
        "reorder: device accepted reordered records"
    );
    assert_eq!(peer::outbound(&tap).len(), 1);
    assert_eq!(peer::inbound(&tap), 2);
    journey.assert_counters("after the reordered first flight", [0, 0], (0, 0));
    let mut kept = peer::kept_connection(&tap);
    let (departed, records) = peer::await_host_departure(&mut kept);
    assert!(records.is_empty());
    journey.assert_original_deadline(first, departed);
    journey.assert_window_reopened(0, [2; 4], departed);

    // From the peer: a device holding the genuine key sends its completion
    // before its proof. Both records are valid ciphertext and the proof is
    // authentic for this exchange; only the order is wrong.
    journey.hosting(0, 2);
    let mut insider = Insider::open(journey.join(0), &journey.pairing, &journey.room(0)).unwrap();
    let (proof, signature) = insider.challenge(&journey.fixture.device).unwrap();
    assert_eq!((proof.host_floor, proof.counter), (0, 0));
    let sent = Instant::now();
    insider.send(b"device-finished-v2").unwrap();
    let _ = insider.send(&signature);
    let (departed, records) = peer::await_host_departure(&mut insider.connection);
    assert!(
        records.is_empty(),
        "reorder: daemon answered a completion sent before the proof"
    );
    journey.assert_rejected_on_arrival("the completion sent before the proof", sent, departed);
    journey.assert_counters("after the reordered proof", [0, 0], (0, 0));

    // Stop lands while the same peer holds an authentic proof it has not sent.
    let held = journey.hosting(1, 2);
    let mut insider = Insider::open(journey.join(1), &journey.pairing, &journey.room(1)).unwrap();
    let (proof, _unsent) = insider.challenge(&journey.fixture.device).unwrap();
    assert_eq!((proof.host_floor, proof.counter), (0, 1));
    journey.stop_while_held(&mut insider.connection, held);
    journey.assert_counters("after stop", [0, 0], (0, 0));
}
