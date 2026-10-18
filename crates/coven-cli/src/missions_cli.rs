//! `coven missions`: submit, read, watch and cancel one familiar mission (#1313).
//!
//! Every command calls the running daemon over its owner-only socket or pipe
//! through `coven_client::DaemonClient`. None uses the in-process action path,
//! whose no-op session runtime would record a launch that never happened. The
//! commands print the daemon's own JSON and decide nothing about the mission:
//! the daemon validates the payload, binds the principal from the transport,
//! and projects the state.

use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use coven_client::{ClientError, DaemonClient, DaemonEndpoint};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ACTIONS_PATH: &str = "/api/v1/actions";
const ENVELOPE_ACTION: &str = "coven.automations.command.v1";
const GET_ACTION: &str = "coven.automations.mission.get.v1";
const SCHEMA_VERSION: &str = "coven.automations.v1";
/// Recorded as the command's origin. It grants nothing: the daemon takes the
/// mission's principal from the owner-only transport.
const ORIGIN_PRINCIPAL: &str = "principal:coven-cli";
/// A submit payload is at most 32 KiB of prompt plus small bounded fields.
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024;
/// Exit status when a command may have been applied and no answer proved it.
pub(crate) const EXIT_OUTCOME_UNKNOWN: i32 = 3;

#[derive(Subcommand, Debug)]
pub(crate) enum MissionsCommand {
    #[command(about = "Submit one mission from a JSON payload; `--json -` reads standard input")]
    Submit {
        /// The submit payload: a file path, or `-` for standard input.
        #[arg(long, value_name = "PATH|-")]
        json: PathBuf,
        /// Idempotency key, 8-200 characters of [A-Za-z0-9._:-]. Generated when absent.
        #[arg(long, value_name = "KEY")]
        adoption_key: Option<String>,
        /// Correlation id recorded with the command.
        #[arg(long, value_name = "ID")]
        correlation_id: Option<String>,
    },
    #[command(about = "Print one mission's current view")]
    Get {
        mission_id: String,
        /// Print the daemon's JSON instead of a summary line.
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Cancel one mission")]
    Cancel {
        mission_id: String,
        /// Why the mission is being cancelled, at most 500 characters.
        #[arg(long)]
        reason: Option<String>,
        /// Idempotency key for this cancel. Generated when absent.
        #[arg(long, value_name = "KEY")]
        adoption_key: Option<String>,
        /// Print the daemon's JSON instead of a summary line.
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Print each new view of one mission until it is terminal")]
    Watch {
        mission_id: String,
        /// Wait between reads, in milliseconds (100-60000).
        #[arg(long, default_value_t = 1_000, value_parser = clap::value_parser!(u64).range(100..=60_000))]
        interval_ms: u64,
        /// Print each view as one JSON line instead of a summary line.
        #[arg(long)]
        json: bool,
    },
}

/// The command printed its result, and its exit status says what kind it was.
#[derive(Debug)]
pub(crate) struct MissionsExit(pub(crate) i32);

impl fmt::Display for MissionsExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the mission command exited with status {}", self.0)
    }
}

impl std::error::Error for MissionsExit {}

pub(crate) fn run(command: MissionsCommand) -> Result<()> {
    let home = crate::paths::coven_home_dir()?;
    let mut daemon = Daemon::connect(&home)?;
    let stdout = &mut io::stdout().lock();
    match command {
        MissionsCommand::Submit {
            json,
            adoption_key,
            correlation_id,
        } => {
            let payload = read_payload(&json)?;
            let key = adoption_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let body = submit_request(payload, &key, correlation_id.as_deref())?;
            let mission_id = mission_id_for(&key)?;
            finish_command(
                daemon.send(&body),
                "mission.submit.v1",
                &key,
                Some(&mission_id),
                stdout,
            )
        }
        MissionsCommand::Cancel {
            mission_id,
            reason,
            adoption_key,
            json,
        } => {
            let key = adoption_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let body = cancel_request(&mission_id, reason.as_deref(), &key)?;
            let answer = daemon.send(&body);
            if json {
                finish_command(answer, "mission.cancel.v1", &key, None, stdout)
            } else {
                finish_summary(answer, "mission.cancel.v1", &key, stdout)
            }
        }
        MissionsCommand::Get { mission_id, json } => {
            let view = read_view(&mut daemon, &mission_id)?;
            print_view(&view, json, stdout)
        }
        MissionsCommand::Watch {
            mission_id,
            interval_ms,
            json,
        } => watch(
            || read_view(&mut daemon, &mission_id),
            Duration::from_millis(interval_ms),
            json,
            stdout,
        ),
    }
}

/// The running daemon, reached over its owner-only transport.
struct Daemon(DaemonClient);

impl Daemon {
    fn connect(home: &Path) -> Result<Self> {
        let endpoint = DaemonEndpoint::discover(home).map_err(|error| {
            anyhow!(
                "no running Coven daemon was found ({error}); start it with `coven daemon start`"
            )
        })?;
        Ok(Self(DaemonClient::new(endpoint)))
    }

    fn send(&mut self, body: &Value) -> Delivery {
        match self.0.request_json("POST", ACTIONS_PATH, Some(body)) {
            Ok(response) => Delivery::Answered {
                status: response.status,
                body: response.body,
            },
            Err(error) if error.request_was_definitely_not_sent() => Delivery::NotSent(error),
            Err(error) => Delivery::Unknown(error),
        }
    }
}

/// What happened to one request.
enum Delivery {
    Answered {
        status: u16,
        body: Vec<u8>,
    },
    /// The transport proved nothing was sent.
    NotSent(ClientError),
    /// The request may have reached the daemon, and no answer arrived.
    Unknown(ClientError),
}

fn read_payload(source: &Path) -> Result<Value> {
    let mut bytes = Vec::new();
    if source == Path::new("-") {
        io::stdin()
            .lock()
            .take(MAX_PAYLOAD_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("failed to read the mission payload from standard input")?;
    } else {
        std::fs::File::open(source)
            .with_context(|| format!("failed to open the mission payload {}", source.display()))?
            .take(MAX_PAYLOAD_BYTES + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read the mission payload {}", source.display()))?;
    }
    if bytes.len() as u64 > MAX_PAYLOAD_BYTES {
        bail!("the mission payload is over {MAX_PAYLOAD_BYTES} bytes");
    }
    let payload: Value =
        serde_json::from_slice(&bytes).context("the mission payload is not valid JSON")?;
    if !payload.is_object() {
        bail!("the mission payload must be a JSON object");
    }
    Ok(payload)
}

fn valid_adoption_key(key: &str) -> bool {
    (8..=200).contains(&key.len())
        && key.as_bytes()[0].is_ascii_alphanumeric()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

/// The id the daemon assigns to the submit with this adoption key:
/// `mission-` and the first 32 hex digits of SHA-256 over the key.
pub(crate) fn mission_id_for(adoption_key: &str) -> Result<String> {
    if !valid_adoption_key(adoption_key) {
        bail!("the adoption key must be 8-200 characters of [A-Za-z0-9._:-], starting with a letter or digit");
    }
    let digest = Sha256::digest(adoption_key.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("mission-{}", &hex[..32]))
}

fn origin(correlation_id: Option<&str>) -> Value {
    let mut origin = json!({ "principal": { "principalId": ORIGIN_PRINCIPAL }, "channel": "cli" });
    if let Some(correlation_id) = correlation_id {
        origin["correlationId"] = json!(correlation_id);
    }
    origin
}

fn submit_request(
    payload: Value,
    adoption_key: &str,
    correlation_id: Option<&str>,
) -> Result<Value> {
    mission_id_for(adoption_key)?;
    // The task statement is the human intent the envelope records; the daemon
    // validates it with the rest of the payload.
    let statement = payload
        .pointer("/task/statement")
        .and_then(Value::as_str)
        .context("the mission payload needs task.statement")?
        .to_owned();
    Ok(json!({
        "action": ENVELOPE_ACTION,
        "envelope": {
            "schemaVersion": SCHEMA_VERSION,
            "command": "mission.submit.v1",
            "adoptionKey": adoption_key,
            "origin": origin(correlation_id),
            "intent": { "statement": statement },
            "payload": payload,
        },
    }))
}

fn cancel_request(mission_id: &str, reason: Option<&str>, adoption_key: &str) -> Result<Value> {
    mission_id_for(adoption_key)?;
    let reason = reason.map(str::trim).filter(|reason| !reason.is_empty());
    let mut payload = json!({ "missionId": mission_id });
    if let Some(reason) = reason {
        payload["reason"] = json!(reason);
    }
    Ok(json!({
        "action": ENVELOPE_ACTION,
        "envelope": {
            "schemaVersion": SCHEMA_VERSION,
            "command": "mission.cancel.v1",
            "adoptionKey": adoption_key,
            "origin": origin(None),
            "intent": { "statement": reason.map_or_else(|| format!("Cancel {mission_id}."), str::to_owned) },
            "payload": payload,
        },
    }))
}

/// How a command answer reads.
#[derive(Debug, PartialEq)]
enum Answer {
    /// `committed` or `replayed`: the command response, as the daemon sent it.
    Applied(Value),
    /// A typed refusal, shaped as a command response; nothing was committed.
    Refused(Value),
}

/// Reads the daemon's answer to a command envelope, or `None` when the answer
/// does not prove what happened.
fn command_answer(status: u16, body: &[u8], command: &str, adoption_key: &str) -> Option<Answer> {
    let wrapper: Value = serde_json::from_slice(body).ok()?;
    if wrapper.get("action")?.as_str()? != ENVELOPE_ACTION {
        return None;
    }
    let refused_wrapper = wrapper.get("ok") == Some(&json!(false))
        && wrapper.get("accepted") == Some(&json!(false))
        && wrapper.get("status") == Some(&json!("rejected"));
    let response = match wrapper.get("result") {
        Some(Value::Object(_)) => wrapper["result"].clone(),
        // Refused before the envelope was read, such as by the owner-only gate.
        None | Some(Value::Null) if refused_wrapper && status >= 400 => {
            let error = wrapper.get("error")?.clone();
            error.get("code")?.as_str()?;
            return Some(Answer::Refused(json!({
                "schemaVersion": SCHEMA_VERSION,
                "command": command,
                "adoptionKey": adoption_key,
                "outcome": "rejected",
                "error": error,
            })));
        }
        _ => return None,
    };
    if response.get("command")?.as_str()? != command
        || response.get("adoptionKey")?.as_str()? != adoption_key
    {
        return None;
    }
    match response.get("outcome")?.as_str()? {
        "committed" | "replayed" if status == 200 && wrapper.get("ok") == Some(&json!(true)) => {
            response.get("result")?.as_object()?;
            Some(Answer::Applied(response))
        }
        "rejected" if status >= 400 && refused_wrapper => {
            response.get("error")?.get("code")?.as_str()?;
            Some(Answer::Refused(response))
        }
        _ => None,
    }
}

/// Prints a command answer as JSON and turns it into the exit status.
fn finish_command(
    sent: Delivery,
    command: &str,
    adoption_key: &str,
    mission_id: Option<&str>,
    out: &mut impl Write,
) -> Result<()> {
    let answer = match sent {
        Delivery::Answered { status, body } => command_answer(status, &body, command, adoption_key),
        Delivery::NotSent(error) => {
            return Err(anyhow!(error).context(format!("{command} was not sent; nothing changed")))
        }
        Delivery::Unknown(_) => None,
    };
    match answer {
        Some(Answer::Applied(response)) => {
            writeln!(out, "{}", serde_json::to_string(&response)?)?;
            Ok(())
        }
        Some(Answer::Refused(response)) => {
            writeln!(out, "{}", serde_json::to_string(&response)?)?;
            Err(MissionsExit(1).into())
        }
        None => {
            let mut unknown =
                json!({ "outcome": "unknown", "command": command, "adoptionKey": adoption_key });
            if let Some(mission_id) = mission_id {
                unknown["missionId"] = json!(mission_id);
            }
            writeln!(out, "{}", serde_json::to_string(&unknown)?)?;
            eprintln!(
                "coven: the daemon may have applied {command}; send it again with --adoption-key {adoption_key} to learn the outcome"
            );
            Err(MissionsExit(EXIT_OUTCOME_UNKNOWN).into())
        }
    }
}

fn finish_summary(
    sent: Delivery,
    command: &str,
    adoption_key: &str,
    out: &mut impl Write,
) -> Result<()> {
    let mut json_line = Vec::new();
    let result = finish_command(sent, command, adoption_key, None, &mut json_line);
    let response: Value = serde_json::from_slice(&json_line).unwrap_or(Value::Null);
    if let Some(outcome) = response
        .pointer("/result/cancellation/outcome")
        .and_then(Value::as_str)
    {
        let state = response
            .pointer("/result/mission/state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        writeln!(out, "cancellation {outcome}; mission {state}")?;
    } else if let Some(code) = response.pointer("/error/code").and_then(Value::as_str) {
        writeln!(out, "refused: {code}")?;
    } else {
        out.write_all(&json_line)?;
    }
    result
}

/// Reads one mission's view through the flat read action.
fn read_view(daemon: &mut Daemon, mission_id: &str) -> Result<Value> {
    let body = json!({ "action": GET_ACTION, "missionId": mission_id });
    match daemon.send(&body) {
        Delivery::Answered { status, body } => view_answer(status, &body, mission_id),
        Delivery::NotSent(error) | Delivery::Unknown(error) => {
            Err(anyhow!(error).context("failed to read the mission from the daemon"))
        }
    }
}

/// The view from a read answer, or an error naming the daemon's refusal code.
fn view_answer(status: u16, body: &[u8], mission_id: &str) -> Result<Value> {
    let wrapper: Value =
        serde_json::from_slice(body).context("the daemon's answer was not valid JSON")?;
    if wrapper.get("action").and_then(Value::as_str) != Some(GET_ACTION) {
        bail!("the daemon's answer was not a mission read");
    }
    if status != 200 || wrapper.get("ok") != Some(&json!(true)) {
        let code = wrapper
            .pointer("/error/code")
            .and_then(Value::as_str)
            .context("the daemon refused the read without a typed error")?;
        bail!(MissionRefused(code.to_owned()));
    }
    let view = wrapper.get("result").cloned().unwrap_or(Value::Null);
    if view.get("missionId").and_then(Value::as_str) != Some(mission_id)
        || view.get("revision").and_then(Value::as_u64).is_none()
        || view.get("terminal").and_then(Value::as_bool).is_none()
    {
        bail!("the daemon's answer was not this mission's view");
    }
    Ok(view)
}

/// The daemon refused a mission read with this typed code.
#[derive(Debug)]
struct MissionRefused(String);

impl fmt::Display for MissionRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the daemon refused the mission read: {}", self.0)
    }
}

impl std::error::Error for MissionRefused {}

fn print_view(view: &Value, json: bool, out: &mut impl Write) -> Result<()> {
    if json {
        writeln!(out, "{}", serde_json::to_string(view)?)?;
    } else {
        let field = |name: &str| view.get(name).and_then(Value::as_str).unwrap_or("?");
        let mut line = format!(
            "{} {} (revision {})",
            field("missionId"),
            field("state"),
            view.get("revision").and_then(Value::as_u64).unwrap_or(0)
        );
        if let Some(reason) = view.get("reason").and_then(Value::as_str) {
            line.push_str(&format!(": {reason}"));
        }
        writeln!(out, "{line}")?;
    }
    out.flush()?;
    Ok(())
}

/// Prints each view whose revision increased, and returns after the first
/// terminal one. A failed read ends the watch; run it again to resume.
fn watch(
    mut read: impl FnMut() -> Result<Value>,
    interval: Duration,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let mut shown = 0;
    loop {
        let view = read()?;
        let revision = view
            .get("revision")
            .and_then(Value::as_u64)
            .context("the daemon's view has no revision")?;
        if revision > shown {
            shown = revision;
            print_view(&view, json, out)?;
        }
        if view.get("terminal") == Some(&json!(true)) {
            return Ok(());
        }
        thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "adopt:mission:0001";

    fn payload() -> Value {
        json!({
            "familiar": { "rootId": format!("familiar:{}", "a".repeat(32)), "revisionId": format!("familiar-revision:{}:3", "a".repeat(32)) },
            "envelope": { "id": "r0-read.1", "descriptorDigest": { "algorithm": "sha256", "canonicalization": "jcs-rfc8785", "value": "b".repeat(64) } },
            "project": { "cwd": "/work/project" },
            "task": { "statement": "Diagnose the failing build", "prompt": "Read the last CI log." },
            "limits": { "timeoutMinutes": 10 },
            "validUntil": "2026-10-18T12:10:00.000Z",
        })
    }

    fn view(revision: u64, state: &str, terminal: bool) -> Value {
        json!({ "missionId": mission_id_for(KEY).unwrap(), "revision": revision, "state": state, "terminal": terminal })
    }

    #[test]
    fn mission_ids_match_the_sdk_derivation() {
        // The same vector the SDK client pins (OpenCoven/sdk#351).
        assert_eq!(
            mission_id_for(KEY).unwrap(),
            "mission-b22855c15769da95a4e6b688337a6fb2"
        );
        for bad in [
            "short",
            " adopt:mission:0001",
            "adopt/mission/0001",
            &"x".repeat(201),
        ] {
            assert!(mission_id_for(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn submit_wraps_the_payload_in_one_spec_envelope() {
        let request = submit_request(payload(), KEY, Some("chat-turn-7")).unwrap();
        assert_eq!(
            request,
            json!({
                "action": "coven.automations.command.v1",
                "envelope": {
                    "schemaVersion": "coven.automations.v1",
                    "command": "mission.submit.v1",
                    "adoptionKey": KEY,
                    "origin": { "principal": { "principalId": "principal:coven-cli" }, "channel": "cli", "correlationId": "chat-turn-7" },
                    "intent": { "statement": "Diagnose the failing build" },
                    "payload": payload(),
                },
            })
        );
        let mut missing = payload();
        missing["task"].as_object_mut().unwrap().remove("statement");
        assert!(submit_request(missing, KEY, None).is_err());
        assert!(submit_request(payload(), "bad key", None).is_err());
    }

    #[test]
    fn cancel_records_the_trimmed_reason_as_its_intent() {
        let id = mission_id_for(KEY).unwrap();
        let request =
            cancel_request(&id, Some("  User stopped it. "), "adopt:cancel:0001").unwrap();
        assert_eq!(
            request["envelope"]["intent"]["statement"],
            "User stopped it."
        );
        assert_eq!(
            request["envelope"]["payload"],
            json!({ "missionId": id, "reason": "User stopped it." })
        );
        let bare = cancel_request(&id, None, "adopt:cancel:0001").unwrap();
        assert_eq!(
            bare["envelope"]["intent"]["statement"],
            format!("Cancel {id}.")
        );
        assert_eq!(bare["envelope"]["payload"], json!({ "missionId": id }));
    }

    fn wrapper(status: &str, ok: bool, result: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({ "ok": ok, "accepted": ok, "action": ENVELOPE_ACTION, "status": status, "result": result })).unwrap()
    }

    #[test]
    fn command_answers_are_read_only_when_they_prove_the_outcome() {
        let committed = json!({ "schemaVersion": SCHEMA_VERSION, "command": "mission.submit.v1", "adoptionKey": KEY, "outcome": "committed", "result": { "missionId": "m" } });
        assert_eq!(
            command_answer(
                200,
                &wrapper("completed", true, committed.clone()),
                "mission.submit.v1",
                KEY
            ),
            Some(Answer::Applied(committed.clone()))
        );
        // Another key's answer, another command's, or a 200 that is not ok prove nothing.
        assert_eq!(
            command_answer(
                200,
                &wrapper("completed", true, committed.clone()),
                "mission.submit.v1",
                "adopt:other:0001"
            ),
            None
        );
        assert_eq!(
            command_answer(
                200,
                &wrapper("completed", true, committed.clone()),
                "mission.cancel.v1",
                KEY
            ),
            None
        );
        assert_eq!(
            command_answer(502, b"bad gateway", "mission.submit.v1", KEY),
            None
        );

        let refused = json!({ "schemaVersion": SCHEMA_VERSION, "command": "mission.submit.v1", "adoptionKey": KEY, "outcome": "rejected", "error": { "code": "CONCURRENCY_LIMIT", "httpStatus": 429 } });
        assert_eq!(
            command_answer(
                429,
                &wrapper("rejected", false, refused.clone()),
                "mission.submit.v1",
                KEY
            ),
            Some(Answer::Refused(refused))
        );
        let gate = serde_json::to_vec(&json!({ "ok": false, "accepted": false, "action": ENVELOPE_ACTION, "status": "rejected", "error": { "code": "AUTHORITY_REQUIRED", "httpStatus": 403 } })).unwrap();
        assert_eq!(
            command_answer(403, &gate, "mission.submit.v1", KEY),
            Some(Answer::Refused(
                json!({ "schemaVersion": SCHEMA_VERSION, "command": "mission.submit.v1", "adoptionKey": KEY, "outcome": "rejected", "error": { "code": "AUTHORITY_REQUIRED", "httpStatus": 403 } })
            ))
        );
    }

    #[test]
    fn an_unknown_outcome_prints_the_key_and_mission_id_and_exits_three() {
        let mut out = Vec::new();
        let error = finish_command(
            Delivery::Answered {
                status: 502,
                body: b"bad gateway".to_vec(),
            },
            "mission.submit.v1",
            KEY,
            Some("mission-b22855c15769da95a4e6b688337a6fb2"),
            &mut out,
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<MissionsExit>().map(|exit| exit.0),
            Some(EXIT_OUTCOME_UNKNOWN)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({ "outcome": "unknown", "command": "mission.submit.v1", "adoptionKey": KEY, "missionId": "mission-b22855c15769da95a4e6b688337a6fb2" })
        );
    }

    #[test]
    fn reads_return_this_missions_view_or_the_typed_refusal() {
        let id = mission_id_for(KEY).unwrap();
        let ok = serde_json::to_vec(&json!({ "ok": true, "accepted": true, "action": GET_ACTION, "status": "completed", "result": view(2, "running", false) })).unwrap();
        assert_eq!(
            view_answer(200, &ok, &id).unwrap(),
            view(2, "running", false)
        );
        assert!(view_answer(200, &ok, &format!("mission-{}", "0".repeat(32))).is_err());
        let missing = serde_json::to_vec(&json!({ "ok": false, "accepted": false, "action": GET_ACTION, "status": "rejected", "error": { "code": "NOT_FOUND", "httpStatus": 404 } })).unwrap();
        let error = view_answer(404, &missing, &id).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<MissionRefused>()
                .map(|refused| refused.0.as_str()),
            Some("NOT_FOUND")
        );
    }

    #[test]
    fn watch_prints_each_new_revision_once_and_stops_at_terminal() {
        let mut views = vec![
            view(1, "queued", false),
            view(1, "queued", false),
            view(2, "running", false),
            view(3, "unavailable", true),
        ]
        .into_iter();
        let mut out = Vec::new();
        watch(
            || views.next().context("read past the end"),
            Duration::ZERO,
            false,
            &mut out,
        )
        .unwrap();
        let id = mission_id_for(KEY).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("{id} queued (revision 1)\n{id} running (revision 2)\n{id} unavailable (revision 3)\n")
        );
        assert!(views.next().is_none());
    }

    #[test]
    fn oversized_or_non_object_payloads_are_refused_before_sending() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.json");
        std::fs::write(&big, format!("{{\"task\":\"{}\"}}", "x".repeat(70_000))).unwrap();
        assert!(read_payload(&big).is_err());
        let array = dir.path().join("array.json");
        std::fs::write(&array, "[1]").unwrap();
        assert!(read_payload(&array).is_err());
        let good = dir.path().join("good.json");
        std::fs::write(&good, serde_json::to_vec(&payload()).unwrap()).unwrap();
        assert_eq!(read_payload(&good).unwrap(), payload());
    }
}
