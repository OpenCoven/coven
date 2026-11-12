//! Repeated-record matrix: every device record position withheld, delayed and
//! repeated, the recorded first flight replayed across consecutive hostings,
//! and peer reorders at each point the protocol has an order to break.
//!
//! Each row runs its own daemon and relay, and the rows run concurrently. The
//! row that replays across three hostings takes about 80 seconds because the
//! manager's retry backoff grows between hostings, so the matrix is ignored by
//! default. Run it with:
//!
//! ```text
//! cargo test -p coven-cli --bin coven -- --ignored --exact \
//!   mobile_memory::gateway::transport_tests::v2_deadline::matrix::repeated_record_matrix
//! ```
use super::*;

#[derive(Clone, Copy, Debug)]
enum Ending {
    /// The daemon waited for a record that never came and left at 30 seconds.
    Deadline,
    /// The daemon left when the record arrived.
    Refused,
    /// The exchange completed and the daemon kept the channel.
    Completed,
}

/// One row driven by the production device with the adversary on the network.
struct Row {
    name: &'static str,
    script: &'static [Outbound],
    /// After the attempt, send the device's recorded record at this position
    /// again on the connection the adversary kept.
    repeat: Option<usize>,
    /// Host records the device was handed before its attempt ended.
    inbound: usize,
    device_completes: bool,
    ending: Ending,
    host_floors: [u64; 2],
    device: (u64, u64),
    /// Hostings expected for the window that starts at the relay floor.
    reopened: [usize; 4],
}

const F: Outbound = Outbound::Forward;
const LATE: Outbound = Outbound::At(LATE_RECORD);

const ROWS: &[Row] = &[
    // Withhold each device record in turn. Only the last two positions move a
    // counter: the device after an authentic host proof, the host after an
    // authentic device proof.
    Row {
        name: "withhold first flight",
        script: &[Outbound::Withhold],
        repeat: None,
        inbound: 0,
        device_completes: false,
        ending: Ending::Deadline,
        host_floors: [0, 0],
        device: (0, 0),
        reopened: [2; 4],
    },
    Row {
        name: "withhold nonce",
        script: &[F, Outbound::Withhold],
        repeat: None,
        inbound: 2,
        device_completes: false,
        ending: Ending::Deadline,
        host_floors: [0, 0],
        device: (0, 0),
        reopened: [2; 4],
    },
    Row {
        name: "withhold proof",
        script: &[F, F, Outbound::Withhold],
        repeat: None,
        inbound: 3,
        device_completes: false,
        ending: Ending::Deadline,
        host_floors: [0, 0],
        device: (1, 0),
        reopened: [2; 4],
    },
    Row {
        name: "withhold completion",
        script: &[F, F, F, Outbound::Withhold],
        repeat: None,
        inbound: 4,
        device_completes: false,
        ending: Ending::Deadline,
        host_floors: [0, 1],
        device: (1, 0),
        // The floor moved, so the window is rooms 1 to 4 and room 4 is new.
        reopened: [2, 2, 2, 1],
    },
    // One late record at each position still completes inside the budget.
    Row {
        name: "delay first flight",
        script: &[LATE],
        repeat: None,
        inbound: 4,
        device_completes: true,
        ending: Ending::Completed,
        host_floors: [0, 1],
        device: (1, 0),
        reopened: [0; 4],
    },
    Row {
        name: "delay nonce",
        script: &[F, LATE],
        repeat: None,
        inbound: 4,
        device_completes: true,
        ending: Ending::Completed,
        host_floors: [0, 1],
        device: (1, 0),
        reopened: [0; 4],
    },
    Row {
        name: "delay proof",
        script: &[F, F, LATE],
        repeat: None,
        inbound: 4,
        device_completes: true,
        ending: Ending::Completed,
        host_floors: [0, 1],
        device: (1, 0),
        reopened: [0; 4],
    },
    Row {
        name: "delay completion",
        script: &[F, F, F, LATE],
        repeat: None,
        inbound: 4,
        device_completes: true,
        ending: Ending::Completed,
        host_floors: [0, 1],
        device: (1, 0),
        reopened: [0; 4],
    },
    // Three late records, the last accepted 27 seconds in, and no completion.
    Row {
        name: "trickle three records",
        script: &[
            Outbound::At(Duration::from_secs(9)),
            Outbound::At(Duration::from_secs(18)),
            Outbound::At(Duration::from_secs(27)),
            Outbound::Withhold,
        ],
        repeat: None,
        inbound: 4,
        device_completes: false,
        ending: Ending::Deadline,
        host_floors: [0, 1],
        device: (1, 0),
        reopened: [2, 2, 2, 1],
    },
    // Send each device record a second time. The copy goes out once the device
    // has been handed everything the daemon sent in answer to the original, so
    // the relay cannot drop a queued answer when the daemon leaves. The daemon
    // reads the copy where it expects the next record and refuses it.
    Row {
        name: "repeat first flight",
        script: &[F, Outbound::Withhold],
        repeat: Some(0),
        inbound: 2,
        device_completes: false,
        ending: Ending::Refused,
        host_floors: [0, 0],
        device: (0, 0),
        reopened: [0; 4],
    },
    Row {
        name: "repeat nonce",
        script: &[F, F, Outbound::Withhold],
        repeat: Some(1),
        inbound: 3,
        device_completes: false,
        ending: Ending::Refused,
        host_floors: [0, 0],
        device: (1, 0),
        reopened: [0; 4],
    },
    Row {
        // The original is an authentic proof, so both counters move. The copy
        // arrives where the completion belongs and ends the exchange.
        name: "repeat proof",
        script: &[F, F, F, Outbound::Withhold],
        repeat: Some(2),
        inbound: 4,
        device_completes: false,
        ending: Ending::Refused,
        host_floors: [0, 1],
        device: (1, 0),
        reopened: [0; 4],
    },
    Row {
        // The exchange is complete. The copy arrives as a first application
        // record, which the established channel refuses.
        name: "repeat completion",
        script: &[],
        repeat: Some(3),
        inbound: 4,
        device_completes: true,
        ending: Ending::Refused,
        host_floors: [0, 1],
        device: (1, 0),
        reopened: [0; 4],
    },
];

fn network_row(row: &Row) {
    let journey = Journey::start(row.name);
    let first = journey.hosting(0, 1);
    let (result, tap) = journey.device_through(0, row.script, false, first.by);
    assert_eq!(
        result.is_ok(),
        row.device_completes,
        "{}: device attempt ended with {result:?}",
        row.name
    );
    assert_eq!(
        peer::inbound(&tap),
        row.inbound,
        "{}: host records handed to the device",
        row.name
    );
    journey.assert_counters("after the attempt", row.host_floors, row.device);
    let mut kept = peer::kept_connection(&tap);
    match row.ending {
        Ending::Deadline => {
            let (departed, records) = peer::await_host_departure(&mut kept);
            assert!(records.is_empty(), "{}: unexpected host records", row.name);
            journey.assert_original_deadline(first, departed);
            journey.assert_window_reopened(row.host_floors[1], row.reopened, departed);
        }
        Ending::Refused => {
            let recorded = peer::outbound(&tap);
            let sent = Instant::now();
            kept.send(
                &recorded[row.repeat.expect("a refused row repeats a record")],
                Instant::now() + Duration::from_secs(30),
            )
            .unwrap();
            let (departed, records) = peer::await_host_departure(&mut kept);
            assert!(records.is_empty(), "{}: unexpected host records", row.name);
            journey.assert_rejected_on_arrival("the repeated record", sent, departed);
            journey.assert_counters("after the repeated record", row.host_floors, row.device);
        }
        Ending::Completed => {
            assert!(
                first.by.elapsed() >= LATE_RECORD,
                "{}: the late record was not late",
                row.name
            );
            assert!(
                first.after.elapsed() < HANDSHAKE_BUDGET,
                "{}: the exchange outlived the budget it completed in",
                row.name
            );
        }
    }
    drop(kept);
    journey.stop_idle();
    journey.assert_counters("after stop", row.host_floors, row.device);
}

/// The recorded first flight replayed into three consecutive hostings of the
/// same room. It is held back 12 seconds in the first and third, and followed
/// at once by the recorded nonce record in the second.
fn replay_across_hostings() {
    let journey = Journey::start("replay across hostings");
    let first = journey.hosting(0, 1);
    let (result, tap) = journey.device_through(0, &[F, F, Outbound::Withhold], false, first.by);
    assert!(result.is_err());
    let recorded = peer::outbound(&tap);
    // The adversary hangs up, so the first hosting ends without its deadline.
    drop(peer::kept_connection(&tap));
    let far = || Instant::now() + Duration::from_secs(30);

    for (nth, late) in [(2, true), (3, false), (4, true)] {
        let hosting = journey.hosting(0, nth);
        let mut replayer = journey.join(0);
        if late {
            sleep_until(hosting.by + LATE_RECORD);
        }
        let sent = Instant::now();
        replayer.send(&recorded[0], far()).unwrap();
        for _ in 0..2 {
            let answer = replayer.receive(far()).unwrap();
            assert_eq!(answer.len(), 48, "hosting {nth} refused the first flight");
        }
        if !late {
            replayer.send(&recorded[1], far()).unwrap();
        }
        let (departed, records) = peer::await_host_departure(&mut replayer);
        assert!(
            records.is_empty(),
            "hosting {nth} answered a replayed record"
        );
        if late {
            journey.assert_original_deadline(hosting, departed);
        } else {
            journey.assert_rejected_on_arrival("the replayed nonce record", sent, departed);
        }
        journey.assert_counters("after a replayed hosting", [0, 0], (1, 0));
    }
    journey.stop_idle();
    journey.assert_counters("after stop", [0, 0], (1, 0));
}

/// A peer with the genuine device key breaks the order at each point where the
/// protocol has one, using three rooms of the same window.
fn reorder_from_the_peer() {
    let journey = Journey::start("reorder from the peer");
    let device = &journey.fixture.device;
    let open = |room: u64| {
        let hosting = journey.hosting(room, 1);
        let insider =
            Insider::open(journey.join(room), &journey.pairing, &journey.room(room)).unwrap();
        (hosting, insider)
    };

    // The completion where the nonce belongs.
    let (_, mut insider) = open(0);
    assert_eq!(insider.receive().unwrap().len(), 32);
    let sent = Instant::now();
    insider.send(b"device-finished-v2").unwrap();
    let (departed, records) = peer::await_host_departure(&mut insider.connection);
    assert!(records.is_empty());
    journey.assert_rejected_on_arrival("the completion sent as a nonce", sent, departed);

    // An authentic proof and the completion, encrypted in protocol order and
    // delivered the other way round.
    let (_, mut insider) = open(1);
    let (_, signature) = insider.challenge(device).unwrap();
    let proof = insider.seal(&signature).unwrap();
    let completion = insider.seal(b"device-finished-v2").unwrap();
    let sent = Instant::now();
    insider.deliver(&completion).unwrap();
    let _ = insider.deliver(&proof);
    let (departed, records) = peer::await_host_departure(&mut insider.connection);
    assert!(records.is_empty());
    journey.assert_rejected_on_arrival("the swapped ciphertext records", sent, departed);

    // The completion before the proof, 12 seconds into the budget: refused
    // when it arrives, with 18 seconds of the budget unused.
    let (hosting, mut insider) = open(2);
    let (_, signature) = insider.challenge(device).unwrap();
    sleep_until(hosting.by + LATE_RECORD);
    let sent = Instant::now();
    insider.send(b"device-finished-v2").unwrap();
    let _ = insider.send(&signature);
    let (departed, records) = peer::await_host_departure(&mut insider.connection);
    assert!(records.is_empty());
    journey.assert_rejected_on_arrival("the late completion sent before the proof", sent, departed);
    assert!(departed.duration_since(hosting.after) < HANDSHAKE_BUDGET);

    journey.assert_counters("after the reorders", [0, 0], (0, 0));
    journey.stop_idle();
    journey.assert_counters("after stop", [0, 0], (0, 0));
}

/// The daemon's two handshake records swapped on the way to the device with no
/// added delay: the device aborts and the daemon waits out its budget.
fn reorder_on_the_network() {
    let journey = Journey::start("reorder on the network");
    let first = journey.hosting(0, 1);
    let (result, tap) = journey.device_through(0, &[], true, first.by);
    assert!(result.is_err());
    assert_eq!(peer::inbound(&tap), 2);
    let mut kept = peer::kept_connection(&tap);
    let (departed, records) = peer::await_host_departure(&mut kept);
    assert!(records.is_empty());
    journey.assert_original_deadline(first, departed);
    journey.assert_window_reopened(0, [2; 4], departed);
    journey.stop_idle();
    journey.assert_counters("after stop", [0, 0], (0, 0));
}

#[test]
#[ignore = "runs 16 daemons for about 80 seconds; see the module documentation"]
fn repeated_record_matrix() {
    let failures: Vec<String> = std::thread::scope(|scope| {
        let mut rows: Vec<(&str, std::thread::ScopedJoinHandle<'_, ()>)> = ROWS
            .iter()
            .map(|row| (row.name, scope.spawn(move || network_row(row))))
            .collect();
        rows.push((
            "replay across hostings",
            scope.spawn(replay_across_hostings),
        ));
        rows.push(("reorder from the peer", scope.spawn(reorder_from_the_peer)));
        rows.push((
            "reorder on the network",
            scope.spawn(reorder_on_the_network),
        ));
        note("matrix", format_args!("{} rows started", rows.len()));
        rows.into_iter()
            .filter_map(|(name, row)| row.join().is_err().then(|| name.to_string()))
            .collect()
    });
    assert!(failures.is_empty(), "failed rows: {failures:?}");
}
