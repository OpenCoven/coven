//! Disposable command-transaction crash fixtures. No provider or runtime launches.
use std::io::Write;

use rusqlite::{functions::FunctionFlags, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::automations::command_adoption::{
    execute_definition_command, DefinitionCommand, DefinitionCommandOutcome,
    DefinitionCommandResponse,
};
use crate::automations::contract::types::AdoptionKey;
use crate::automations::definition::RoutineDefinition;
use crate::automations::owner_grants::CommandAuthority;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const FIRST_AT: &str = "2026-08-30T09:00:00.000Z";
const SCRATCH_ENV: &str = "COVEN_AUTOMATIONS_CONFORMANCE_SCRATCH";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterDefinition,
    AfterEvent,
    AfterAdoption,
    None,
}
impl Boundary {
    fn table(self) -> Option<&'static str> {
        match self {
            Self::AfterDefinition => Some("automation_definitions"),
            Self::AfterEvent => Some("automation_events"),
            Self::AfterAdoption => Some("automation_command_adoptions"),
            Self::None => None,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    boundary: Boundary,
    adoption_key: String,
    definition: Value,
    prompt_bytes: usize,
    expected: Expected,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    baseline_preserved: bool,
    integrity_ok: bool,
    final_rows: u64,
    replay_preserved: bool,
}
#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Committed,
}

fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-adoption","name":"Conformance adoption",
        "status":"PAUSED","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc",
        "misfire":"latest","overlap":"forbid","timeoutMinutes":30,
        "runtime":"coven-code","prompt":"Run the native adoption conformance probe."})
}

fn validate(case: &Case) -> Result<(), &'static str> {
    RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
    if !super::valid_case_id(&case.case_id)
        || AdoptionKey::new(case.adoption_key.clone()).is_err()
        || case.definition != synthetic_definition()
        || case.prompt_bytes != 131072
        || !case.expected.baseline_preserved
        || !case.expected.integrity_ok
        || !case.expected.replay_preserved
        || case.expected.final_rows != 1
        || (case.expected.initial_outcome == InitialOutcome::Committed)
            != (case.boundary == Boundary::None)
    {
        return Err(INVALID);
    }
    Ok(())
}

// Values stay private. Debug encoding preserves SQLite's value type and exact
// bytes, including blobs, rather than coercing unlike values into JSON scalars.
type Snapshot = Vec<Vec<Vec<String>>>;
fn snapshot(conn: &Connection) -> Result<Snapshot, &'static str> {
    let mut tables = Vec::new();
    for table in [
        "automation_definitions",
        "automation_rich_definition_revisions",
        "automation_owner_principal",
        "automation_owner_grants",
        "automation_command_adoptions",
        "automation_command_reservations",
        "automation_events",
        "automation_event_stream_heads",
    ] {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .map_err(|_| FAILED)?;
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|column| {
                        row.get::<_, rusqlite::types::Value>(column)
                            .map(|value| format!("{value:?}"))
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|_| FAILED)?;
        tables.push(
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| FAILED)?,
        );
    }
    Ok(tables)
}

fn definition(case: &Case) -> Value {
    let mut value = case.definition.clone();
    value["prompt"] = json!("x".repeat(case.prompt_bytes));
    value
}
fn execute(conn: &Connection, case: &Case, at: &str) -> anyhow::Result<DefinitionCommandResponse> {
    execute_definition_command(
        conn,
        &case.adoption_key,
        DefinitionCommand::Create {
            definition: definition(case),
        },
        at,
        CommandAuthority::OwnerLocal,
    )
}
fn projection(response: &DefinitionCommandResponse) -> Result<Value, &'static str> {
    if response.error.is_some()
        || response.result.is_none()
        || response.event_ref.is_none()
        || response.revision != Some(1)
    {
        return Err(FAILED);
    }
    Ok(
        json!({"revision": response.revision, "result": response.result,
        "eventRef": response.event_ref, "firstCommittedAt": FIRST_AT}),
    )
}
fn announce(message: &Value) -> Result<(), &'static str> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, message).map_err(|_| FAILED)?;
    stdout.write_all(b"\n").map_err(|_| FAILED)?;
    stdout.flush().map_err(|_| FAILED)
}

/// Hidden worker: its input cannot choose a database. Every invocation creates
/// a fresh random child directory under the caller's conformance scratch root.
/// The parent owns that root and removes it after killing/reaping this worker.
pub(super) fn worker(input: &Value) -> Result<(), &'static str> {
    let case: Case = serde_json::from_value(input.clone()).map_err(|_| INVALID)?;
    validate(&case)?;
    let scratch = std::env::var_os(SCRATCH_ENV).ok_or(FAILED)?;
    let temporary = tempfile::Builder::new()
        .prefix("command-crash-")
        .tempdir_in(scratch)
        .map_err(|_| FAILED)?;
    crate::daemon::ensure_private_coven_home(temporary.path()).map_err(|_| FAILED)?;
    // A killed process cannot run TempDir::drop; control exits retain the same
    // parent-owned files so the parent can inspect both cases identically.
    let home = temporary.keep();
    let relative_home = home
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(FAILED)?
        .to_owned();
    let conn = crate::store::open_store(&home.join("store.sqlite")).map_err(|_| FAILED)?;
    let mut seed = case.definition.clone();
    seed["id"] = json!("crash-preexisting");
    let seed_response = execute_definition_command(
        &conn,
        "adopt:crash:baseline",
        DefinitionCommand::Create {
            definition: seed.clone(),
        },
        FIRST_AT,
        CommandAuthority::OwnerLocal,
    )
    .map_err(|_| FAILED)?;
    if seed_response.outcome != DefinitionCommandOutcome::Committed {
        return Err(FAILED);
    }
    let stored = crate::automations::store::get_definition(&conn, "crash-preexisting")
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let persisted: RoutineDefinition =
        serde_json::from_str(&stored.definition_json).map_err(|_| FAILED)?;
    if persisted != RoutineDefinition::from_json(&seed).map_err(|_| INVALID)? {
        return Err(FAILED);
    }
    crate::automations::owner_grants::owner_principal_id(&conn, FIRST_AT).map_err(|_| FAILED)?;
    let baseline = snapshot(&conn)?;
    std::fs::write(
        home.join("baseline.json"),
        serde_json::to_vec(&baseline).map_err(|_| FAILED)?,
    )
    .map_err(|_| FAILED)?;
    // Fixture-only cache pressure makes uncommitted WAL spill observable. No
    // production quota, connection default, or durability deadline changes.
    let checkpoint: (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|_| FAILED)?;
    if checkpoint != (0, 0, 0) {
        return Err(FAILED);
    }
    conn.execute_batch("PRAGMA cache_size=1; PRAGMA cache_spill=ON; PRAGMA wal_autocheckpoint=0; PRAGMA synchronous=FULL;").map_err(|_| FAILED)?;
    if let Some(table) = case.boundary.table() {
        let message =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        conn.create_scalar_function(
            "coven_crash_fixture_pause",
            0,
            FunctionFlags::SQLITE_UTF8,
            move |_| -> rusqlite::Result<i64> {
                announce(&message).map_err(|_| rusqlite::Error::InvalidQuery)?;
                loop {
                    std::thread::park();
                }
            },
        )
        .map_err(|_| FAILED)?;
        conn.execute_batch(&format!("CREATE TEMP TRIGGER coven_crash_fixture AFTER INSERT ON {table} BEGIN SELECT coven_crash_fixture_pause(); END;")).map_err(|_| FAILED)?;
    }
    let committed = execute(&conn, &case, FIRST_AT).map_err(|_| FAILED)?;
    if case.boundary != Boundary::None || committed.outcome != DefinitionCommandOutcome::Committed {
        return Err(FAILED);
    }
    std::fs::write(
        home.join("committed.json"),
        serde_json::to_vec(&projection(&committed)?).map_err(|_| FAILED)?,
    )
    .map_err(|_| FAILED)?;
    drop(conn);
    announce(&json!({"state":"committed", "boundary":case.boundary, "relativeHome":relative_home}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VectorSet {
    schema_version: String,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Handshake {
    state: String,
    boundary: Boundary,
    relative_home: String,
}

// These are fixture hang guards, not claims about production recovery speed.
const CHILD_GUARD: std::time::Duration = std::time::Duration::from_secs(10);
struct WorkerProcess {
    child: std::process::Child,
    reader: Option<std::thread::JoinHandle<()>>,
    writer: Option<std::thread::JoinHandle<()>>,
    reaped: bool,
}
impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.cleanup();
    }
}
impl WorkerProcess {
    fn cleanup(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            self.reaped = self.child.wait().is_ok();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
    fn finish(&mut self, terminate: bool) -> Result<std::process::ExitStatus, &'static str> {
        if terminate {
            // The handshake callback cannot return normally. Require it to be
            // alive when the parent issues the abrupt OS termination request.
            if self.child.try_wait().map_err(|_| FAILED)?.is_some() {
                return Err(FAILED);
            }
            self.child.kill().map_err(|_| FAILED)?;
            let status = self.child.wait().map_err(|_| FAILED)?;
            self.reaped = true;
            return Ok(status);
        }
        let deadline = std::time::Instant::now() + CHILD_GUARD;
        loop {
            if let Some(status) = self.child.try_wait().map_err(|_| FAILED)? {
                self.reaped = true;
                return Ok(status);
            }
            if std::time::Instant::now() >= deadline {
                return Err(FAILED);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
type HandshakeReceiver = std::sync::mpsc::Receiver<Result<Handshake, &'static str>>;
fn start_worker(
    case: &Case,
    scratch: &std::path::Path,
) -> Result<(WorkerProcess, HandshakeReceiver), &'static str> {
    use std::io::{BufRead, Read};
    use std::process::{Command, Stdio};
    let mut command = Command::new(std::env::current_exe().map_err(|_| FAILED)?);
    command
        .args(["automations", "conformance", "command-crash-worker"])
        .env_clear()
        .env(SCRATCH_ENV, scratch)
        .env("COVEN_HOME", scratch.join("must-not-create"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Windows uses this OS location for native services; no provider settings
    // or caller-selected store paths are inherited by the worker.
    for key in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let child = command.spawn().map_err(|_| FAILED)?;
    let mut process = WorkerProcess {
        child,
        reader: None,
        writer: None,
        reaped: false,
    };
    let stdout = process.child.stdout.take().ok_or(FAILED)?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    process.reader = Some(std::thread::spawn(move || {
        let mut line = Vec::new();
        let result = std::io::BufReader::new(stdout)
            .take(2049)
            .read_until(b'\n', &mut line)
            .map_err(|_| FAILED)
            .and_then(|_| {
                if line.len() > 2048 || !line.ends_with(b"\n") {
                    return Err(FAILED);
                }
                serde_json::from_slice(&line).map_err(|_| FAILED)
            });
        let _ = sender.send(result);
    }));
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let mut stdin = process.child.stdin.take().ok_or(FAILED)?;
    // Input delivery shares the handshake guard. A stalled child must not
    // block the parent before it can kill/reap and close both pipes.
    process.writer = Some(std::thread::spawn(move || {
        let _ = stdin.write_all(&payload);
    }));
    Ok((process, receiver))
}
fn read_private_json<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
) -> Result<T, &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| FAILED)?;
    if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 {
        return Err(FAILED);
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|_| FAILED)?).map_err(|_| FAILED)
}
fn uncommitted_wal(home: &std::path::Path) -> Result<bool, &'static str> {
    let path = home.join("store.sqlite-wal");
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| FAILED)?;
    if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
        return Err(FAILED);
    }
    let bytes = std::fs::read(path).map_err(|_| FAILED)?;
    if bytes.len() < 32 {
        return Ok(false);
    }
    // SQLite WAL format from the locked libsqlite3-sys amalgamation:
    // header and stored checksums are big-endian; the magic selects the
    // checksum input byte order. Carry the checksum through every frame.
    let word = |value: &[u8]| u32::from_be_bytes(value.try_into().unwrap());
    let magic = word(&bytes[0..4]);
    let page_size = word(&bytes[8..12]) as usize;
    if ![0x377f0682, 0x377f0683].contains(&magic)
        || word(&bytes[4..8]) != 3007000
        || !(512..=65536).contains(&page_size)
        || !page_size.is_power_of_two()
    {
        return Ok(false);
    }
    let frame_size = 24 + page_size;
    let frames = &bytes[32..];
    if frames.is_empty() || !frames.len().is_multiple_of(frame_size) {
        return Ok(false);
    }
    let mut checksum = [0_u32; 2];
    let mut accumulate = |input: &[u8]| {
        for pair in input.as_chunks::<8>().0 {
            let decode = |value: &[u8]| {
                let bytes = value.try_into().unwrap();
                if magic == 0x377f0682 {
                    u32::from_le_bytes(bytes)
                } else {
                    u32::from_be_bytes(bytes)
                }
            };
            checksum[0] = checksum[0]
                .wrapping_add(decode(&pair[..4]))
                .wrapping_add(checksum[1]);
            checksum[1] = checksum[1]
                .wrapping_add(decode(&pair[4..]))
                .wrapping_add(checksum[0]);
        }
        checksum
    };
    if accumulate(&bytes[..24]) != [word(&bytes[24..28]), word(&bytes[28..32])] {
        return Ok(false);
    }
    for frame in frames.chunks_exact(frame_size) {
        // No commit is allowed: the worker truncated the WAL before its only
        // subsequent transaction. Reject stale frames and corrupted content.
        if word(&frame[..4]) == 0 || word(&frame[4..8]) != 0 || frame[8..16] != bytes[16..24] {
            return Ok(false);
        }
        accumulate(&frame[..8]);
        if accumulate(&frame[24..]) != [word(&frame[16..20]), word(&frame[20..24])] {
            return Ok(false);
        }
    }
    Ok(true)
}
fn preserves(before: &Snapshot, after: &Snapshot) -> bool {
    before.len() == after.len()
        && before
            .iter()
            .zip(after)
            .all(|(before, after)| before.iter().all(|row| after.contains(row)))
}
fn one_commit(before: &Snapshot, after: &Snapshot) -> bool {
    preserves(before, after)
        && before
            .iter()
            .zip(after)
            .enumerate()
            .all(|(index, (before, after))| {
                after.len() == before.len() + usize::from([0, 4, 6, 7].contains(&index))
            })
}
fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let (mut process, receiver) = start_worker(case, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    let mut components = std::path::Path::new(&ready.relative_home).components();
    if ready.boundary != case.boundary
        || ready.state != if crash { "paused" } else { "committed" }
        || !ready.relative_home.starts_with("command-crash-")
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(FAILED);
    }
    let home = root
        .path()
        .join(&ready.relative_home)
        .canonicalize()
        .map_err(|_| FAILED)?;
    if !home.starts_with(root.path().canonicalize().map_err(|_| FAILED)?) {
        return Err(FAILED);
    }
    let baseline: Snapshot = read_private_json(&home.join("baseline.json"))?;
    if baseline.len() != 8
        || baseline[0].len() != 1
        || baseline[4].len() != 1
        || baseline[6].len() != 1
    {
        return Err(FAILED);
    }
    let wal_observed = if crash {
        uncommitted_wal(&home)?
    } else {
        false
    };
    if crash && !wal_observed {
        return Err(FAILED);
    }
    let status = process.finish(crash)?;
    #[cfg(unix)]
    let terminated = {
        use std::os::unix::process::ExitStatusExt;
        status.signal() == Some(9)
    };
    #[cfg(not(unix))]
    let terminated = !status.success();
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let path = home.join("store.sqlite");
    if !std::fs::symlink_metadata(&path)
        .map_err(|_| FAILED)?
        .is_file()
    {
        return Err(FAILED);
    }
    // Observe raw recovery before the production initializer can repair schema
    // or metadata. Never create a replacement database if the worker lost it.
    let raw = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| FAILED)?;
    let integrity: String = raw
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| FAILED)?;
    let initial = snapshot(&raw)?;
    let baseline_preserved = if crash {
        initial == baseline
    } else {
        one_commit(&baseline, &initial)
    };
    let mut passed = integrity == "ok" && baseline_preserved;
    drop(raw);
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    passed &= snapshot(&conn)? == initial;
    let retried = execute(&conn, case, FIRST_AT).map_err(|_| FAILED)?;
    passed &= retried.outcome
        == if crash {
            DefinitionCommandOutcome::Committed
        } else {
            DefinitionCommandOutcome::Replayed
        };
    let committed = if crash {
        projection(&retried)?
    } else {
        read_private_json::<Value>(&home.join("committed.json"))?
    };
    passed &= projection(&retried)? == committed;
    let reference = &committed["eventRef"];
    let (kind, id) = reference["stream"]
        .as_str()
        .and_then(|stream| stream.split_once('/'))
        .ok_or(FAILED)?;
    let sequence = reference["sequence"].as_i64().ok_or(FAILED)?;
    let event_count: i64 = conn.query_row("SELECT COUNT(*) FROM automation_events WHERE stream_kind=?1 AND stream_id=?2 AND sequence=?3", rusqlite::params![kind, id, sequence], |row| row.get(0)).map_err(|_| FAILED)?;
    passed &=
        kind == "automation" && Some(id) == case.definition["id"].as_str() && event_count == 1;
    let final_state = snapshot(&conn)?;
    passed &= one_commit(&baseline, &final_state);
    let stored = crate::automations::store::get_definition(&conn, "conformance-adoption")
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let persisted: RoutineDefinition =
        serde_json::from_str(&stored.definition_json).map_err(|_| FAILED)?;
    passed &= persisted == RoutineDefinition::from_json(&definition(case)).map_err(|_| INVALID)?;
    drop(conn);
    let reopened = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let replay = execute(&reopened, case, "2026-08-30T09:01:00.000Z").map_err(|_| FAILED)?;
    let replay_preserved = replay.outcome == DefinitionCommandOutcome::Replayed
        && replay.replay_first_committed_at.as_deref() == Some(FIRST_AT)
        && projection(&replay)? == committed;
    let unchanged = snapshot(&reopened)? == final_state;
    passed &= replay_preserved && unchanged;
    Ok((
        passed,
        json!({"boundary":case.boundary, "initialOutcome":if crash {"terminated"} else {"committed"},
        "abruptlyTerminated":terminated, "uncommittedWalFramesObserved":wal_observed,
        "baselinePreserved":baseline_preserved, "integrityOk":integrity == "ok",
        "finalAddedDefinitions":final_state[0].len().checked_sub(baseline[0].len()),
        "finalAddedAdoptions":final_state[4].len().checked_sub(baseline[4].len()),
        "finalAddedEvents":final_state[6].len().checked_sub(baseline[6].len()),
        "preexistingPreserved":preserves(&baseline,&final_state),
        "replayPreserved":replay_preserved, "replayWritesUnchanged":unchanged}),
    ))
}

pub(super) fn evaluate(vector: &Value) -> Result<(bool, Vec<Value>), &'static str> {
    let vectors: VectorSet = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    if vectors.schema_version != "coven.automations.command-process-crash-recovery-vectors.v1"
        || vectors.cases.len() != 4
    {
        return Err(INVALID);
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut boundaries = std::collections::BTreeSet::new();
    for case in &vectors.cases {
        validate(case)?;
        if !ids.insert(&case.case_id) || !boundaries.insert(case.boundary) {
            return Err(INVALID);
        }
    }
    let mut passed = true;
    let mut observations = Vec::new();
    for case in &vectors.cases {
        let (matches, observation) = case_matches(case)?;
        passed &= matches;
        observations.push(observation);
    }
    Ok((passed, observations))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::sync_channel;

    #[test]
    fn wal_observation_rejects_corrupted_sqlite_frames() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("store.sqlite");
        let conn = Connection::open(db).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE probe(value BLOB); PRAGMA wal_checkpoint(TRUNCATE); PRAGMA cache_size=1; PRAGMA cache_spill=ON; BEGIN IMMEDIATE; INSERT INTO probe VALUES (zeroblob(131072));").unwrap();
        let path = root.path().join("store.sqlite-wal");
        let original = std::fs::read(&path).unwrap();
        assert!(uncommitted_wal(root.path()).unwrap());
        // Mutate actual SQLite output, independently of the observer's checksum
        // implementation: header checksum, frame salt, checksum, page, and commit.
        for offset in [24, 40, 48, 56, 36] {
            let mut corrupted = original.clone();
            corrupted[offset] ^= 1;
            std::fs::write(&path, corrupted).unwrap();
            assert!(
                !uncommitted_wal(root.path()).unwrap(),
                "accepted corruption at {offset}"
            );
        }
        std::fs::write(&path, &original[..original.len() - 1]).unwrap();
        assert!(!uncommitted_wal(root.path()).unwrap());
        std::fs::write(path, original).unwrap();
        conn.execute_batch("ROLLBACK").unwrap();
    }

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../../../conformance/automations/runner/command-process-crash-recovery.vectors.json")).unwrap()
    }
    #[test]
    fn crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        let mut duplicate = original.clone();
        duplicate["cases"][1]["boundary"] = duplicate["cases"][0]["boundary"].clone();
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }
    #[test]
    fn crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (field, value) in [
            ("baselinePreserved", json!(false)),
            ("integrityOk", json!(false)),
            ("replayPreserved", json!(false)),
            ("finalRows", json!(2)),
            ("initialOutcome", json!("committed")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        for (field, value) in [
            ("databasePath", json!("private-input")),
            ("promptBytes", json!(1)),
            ("boundary", json!("after_external_dispatch")),
            ("definition", json!({})),
            ("adoptionKey", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] = json!("unknown");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    const STALL_MARKER_ENV: &str = "COVEN_TEST_COMMAND_CRASH_STALL_MARKER";
    // A subprocess-only test fixture. Normal workspace execution returns
    // immediately. The selected child publishes readiness and never reads stdin.
    #[test]
    fn stalled_input_child() {
        if let Some(marker) = std::env::var_os(STALL_MARKER_ENV) {
            std::fs::write(marker, b"ready").unwrap();
            loop {
                std::thread::park();
            }
        }
    }
    #[test]
    fn cleanup_reaps_a_stalled_child_before_joining_pipe_threads() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("ready");
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "automations::conformance_target::command_process_crash_recovery::tests::stalled_input_child", "--nocapture"])
            .env(STALL_MARKER_ENV, &marker)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().unwrap();
        let mut process = WorkerProcess {
            child,
            reader: None,
            writer: None,
            reaped: false,
        };
        let deadline = std::time::Instant::now() + CHILD_GUARD;
        while std::fs::read(&marker).ok().as_deref() != Some(b"ready") {
            assert!(
                std::time::Instant::now() < deadline,
                "child did not announce readiness"
            );
            assert!(
                process.child.try_wait().unwrap().is_none(),
                "child exited before readiness"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut stdout = process.child.stdout.take().unwrap();
        process.reader = Some(std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
        }));
        let mut stdin = process.child.stdin.take().unwrap();
        let (started, starting) = sync_channel(1);
        let (finished, result) = sync_channel(1);
        process.writer = Some(std::thread::spawn(move || {
            started.send(()).unwrap();
            let failed = stdin.write_all(&vec![b'x'; 2 * 1024 * 1024]).is_err();
            let _ = finished.send(failed);
        }));
        starting.recv_timeout(CHILD_GUARD).unwrap();
        process.cleanup();
        assert!(process.reaped);
        assert!(!process.child.try_wait().unwrap().unwrap().success());
        assert!(process.writer.is_none() && process.reader.is_none());
        // The producer has joined; a successful full write would invalidate
        // this stalled-reader fixture rather than proving cleanup under pressure.
        assert!(result.recv_timeout(CHILD_GUARD).unwrap());
        drop(process);
        root.close().unwrap();
    }
}
