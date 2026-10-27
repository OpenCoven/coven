//! Launching a trusted Runtime Authority attempt under exactly the scopes its
//! signed Threads decision grants (coven#857 slice 6, #1603).
//!
//! The trusted adapter pins the scoped read envelope (`r0-read.2`), whose
//! only file tools are the daemon's scoped read service. This module is the
//! launch half: from the consumer projection a launch carries, it finds the
//! execution binding the attempt pinned, authenticates it, follows it to the
//! recorded Threads decision, and takes the scopes from that decision's signed
//! request. Scoped read admission then builds the reader over the routine's
//! project folder from those scopes and nothing else.
//!
//! Nothing the launch is handed can widen the grant. A changed definition, a
//! different launch folder, an unpinned or altered binding, or a decision that
//! no longer verifies or was never consumed by this dispatch refuses the
//! launch before the harness starts.

use std::fmt;
use std::path::Path;
use std::time::Instant;

use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

use super::authority_projection::AutomationAuthorityConsumerProjection;
use super::contract::authority::{
    AuthorityEvidenceVerifier, AuthorityProfileErrorCode, AuthorityValidationPhase,
    AutomationAuthorityExtension,
};
use super::runtime_envelope::{self, RuntimeEnvelope};
use super::scoped_read::admission::{self, AdmissionError, Prepared};
use super::scoped_read::service::Gate;
use super::store_gate::{self, Held, StoreGate};
use super::threads_decisions;
use super::trusted_authority::{opaque_id, TrustedAuthority};

/// One launch of a trusted attempt.
pub(crate) struct ScopedLaunch<'a> {
    pub coven_home: &'a Path,
    /// The projection the runner handed the runtime.
    pub projection: &'a AutomationAuthorityConsumerProjection,
    pub harness: &'a str,
    /// The session the runner launches the attempt as.
    pub session_id: &'a str,
    /// The routine's project folder, the launch's working directory as the
    /// runner built it.
    pub project: &'a Path,
    /// The resolved harness executable.
    pub program: &'a Path,
    /// The `coven` executable the harness launches as the relay.
    pub relay: &'a Path,
    pub owner_home: Option<&'a Path>,
    pub deadline: Option<Instant>,
}

/// Why a trusted launch was refused before the harness started.
#[derive(Debug)]
pub(crate) enum LaunchRefusal {
    /// The projection pins no envelope served by the scoped read service.
    NotScoped,
    /// No attempt pins this binding, or the pinned binding does not verify or
    /// is not the projection's.
    BindingMismatch(String),
    /// The launch folder is not the project the binding names.
    ProjectMismatch,
    /// The binding's Threads decision is unknown, no longer verifies, is
    /// outside its validity, or was not consumed by this dispatch.
    DecisionUntrusted(String),
    /// The attempt's authority was withdrawn since dispatch: the attempt is
    /// no longer live, a stop or cancellation is pending, the familiar
    /// revision was revoked or retired, or the definition revision is no
    /// longer current and active. Carries the profile code that fits.
    Withdrawn(AuthorityProfileErrorCode, String),
    /// Scoped read admission refused the launch.
    Admission(AdmissionError),
    /// The store could not be read.
    Unavailable(String),
}

impl fmt::Display for LaunchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotScoped => write!(
                f,
                "the attempt's runtime envelope is not served by the scoped read service"
            ),
            Self::BindingMismatch(why) => {
                write!(f, "the attempt's execution binding does not match: {why}")
            }
            Self::ProjectMismatch => write!(
                f,
                "the launch folder is not the project the attempt was authorized for"
            ),
            Self::DecisionUntrusted(why) => {
                write!(f, "the attempt's Threads decision is not trusted: {why}")
            }
            Self::Withdrawn(code, why) => {
                write!(
                    f,
                    "the attempt's authority was withdrawn ({}): {why}",
                    code.as_str()
                )
            }
            Self::Admission(error) => write!(f, "{error}"),
            Self::Unavailable(why) => write!(f, "Runtime Authority state is unreadable: {why}"),
        }
    }
}

impl std::error::Error for LaunchRefusal {}

/// What a trusted launch was verified to run under.
#[derive(Debug)]
pub(crate) struct Authorized {
    pub envelope: &'static RuntimeEnvelope,
    /// The scopes the signed decision grants.
    pub scopes: Vec<Value>,
    /// What the gate holds for the attempt.
    pub held: Held,
}

/// The envelope a trusted launch runs under, and the scopes its signed
/// decision grants, at `now`.
///
/// 1. The projection must pin an envelope served by the scoped read service,
///    on the launch's harness.
/// 2. The attempt it names must have pinned an execution binding that the
///    daemon's `dispatch-authority` keys authenticate and whose projection is
///    exactly the one the launch carries.
/// 3. The launch folder must be the project that binding names.
/// 4. The binding's Threads decision must still verify, be inside its
///    validity window, match the binding's request and decision digests and
///    this attempt, and have been consumed by the dispatch.
/// 5. P, the predicate the gate evaluates for every operation
///    ([`store_gate::authorized`]), must hold.
///
/// The scopes are then the decision's signed request's, never the stored
/// definition's: a definition changed after the decision cannot widen them.
pub(crate) fn launch_scopes(
    conn: &Connection,
    launch: &ScopedLaunch<'_>,
    now: DateTime<Utc>,
) -> Result<Authorized, LaunchRefusal> {
    let unavailable = |error: anyhow::Error| LaunchRefusal::Unavailable(format!("{error:#}"));
    let projection = launch.projection;

    // 1. The scoped read envelope.
    let envelope =
        runtime_envelope::by_descriptor_digest(projection.runtime.descriptor_digest.value.as_str())
            .filter(|envelope| {
                envelope.service.is_some()
                    && envelope.harness == launch.harness
                    && envelope.descriptor_version == projection.runtime.descriptor_version.as_str()
            })
            .ok_or(LaunchRefusal::NotScoped)?;

    // 2. The binding the attempt pinned.
    let pinned: Option<Option<String>> = conn
        .query_row(
            "SELECT authority_extension_json FROM automation_attempts WHERE id = ?1",
            [projection.attempt_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read the attempt's pinned authority")
        .map_err(unavailable)?;
    let Some(Some(pinned)) = pinned else {
        return Err(LaunchRefusal::BindingMismatch(
            "the attempt pinned no execution binding".to_owned(),
        ));
    };
    let extension: AutomationAuthorityExtension = serde_json::from_str(&pinned)
        .map_err(|error| LaunchRefusal::BindingMismatch(error.to_string()))?;
    TrustedAuthority::new(conn, launch.coven_home, now)
        .verify(&extension, AuthorityValidationPhase::PreDispatch)
        .map_err(|error| LaunchRefusal::BindingMismatch(error.to_string()))?;
    if AutomationAuthorityConsumerProjection::from_validated(&extension) != *projection {
        return Err(LaunchRefusal::BindingMismatch(
            "the launch's projection is not the pinned binding's".to_owned(),
        ));
    }
    let binding = serde_json::to_value(&extension.execution_binding)
        .context("an execution binding serializes")
        .map_err(unavailable)?;

    // 3. The project.
    let project = launch
        .project
        .to_str()
        .ok_or(LaunchRefusal::ProjectMismatch)?;
    if binding["contextProjection"]["projectId"] != json!(opaque_id("project", project)) {
        return Err(LaunchRefusal::ProjectMismatch);
    }

    // 4. The decision.
    let untrusted = |why: String| LaunchRefusal::DecisionUntrusted(why);
    let decision_id = binding["threads"]["decisionId"]
        .as_str()
        .ok_or_else(|| untrusted("the binding names no decision".to_owned()))?;
    let decided = match threads_decisions::valid_at(conn, decision_id, now).map_err(unavailable)? {
        Ok(decided) => decided,
        Err(refusal) => return Err(untrusted(format!("{refusal:?}"))),
    };
    let prefixed = |digest: &Value| digest["value"].as_str().map(|hex| format!("sha256:{hex}"));
    let request = &decided.request;
    let matches = prefixed(&binding["threads"]["decisionDigest"]).as_deref()
        == Some(decided.decision_digest.as_str())
        && prefixed(&binding["authorization"]["requestDigest"]).as_deref()
            == Some(decided.request_digest.as_str())
        && decided.outcome == "permit"
        && request["execution"]["occurrence_id"] == binding["base"]["occurrenceId"]
        && request["execution"]["run_id"] == binding["base"]["runId"]
        && request["execution"]["attempt"] == binding["base"]["attemptNumber"]
        && request["replay"]["adoption_key"] == binding["base"]["adoptionKey"]
        && request["context"]["runtime"]["descriptor_digest"]
            == json!(format!("sha256:{}", envelope.descriptor_digest()));
    if !matches {
        return Err(untrusted(
            "the decision is not the one this attempt's binding names".to_owned(),
        ));
    }
    if !threads_decisions::consumed(conn, decision_id).map_err(unavailable)? {
        return Err(untrusted(
            "the decision was not consumed by a dispatch".to_owned(),
        ));
    }
    let scopes = request["scopes"]
        .as_array()
        .cloned()
        .ok_or_else(|| untrusted("the signed request carries no scopes".to_owned()))?;

    // 5. P.
    let held = Held::new(
        &pinned,
        &binding,
        launch.session_id,
        &decided.decision,
        request,
    )
    .map_err(unavailable)?;
    if let Err(withdrawal) = store_gate::authorized(conn, &held, now).map_err(unavailable)? {
        return Err(LaunchRefusal::Withdrawn(
            withdrawal.code,
            withdrawal.why.to_owned(),
        ));
    }
    Ok(Authorized {
        envelope,
        scopes,
        held,
    })
}

/// Admits and prepares one trusted launch under its decision's scopes. The
/// service's operations run under a [`StoreGate`] that holds what the launch
/// verified: no other gate can be supplied.
pub(crate) fn prepare(
    conn: &Connection,
    launch: &ScopedLaunch<'_>,
    now: DateTime<Utc>,
) -> Result<Prepared, LaunchRefusal> {
    prepare_gated(conn, launch, now, |authorized| {
        let service = authorized
            .envelope
            .service
            .as_ref()
            .context("a scoped envelope binds its service")?;
        Ok(Box::new(StoreGate::open(
            launch.coven_home,
            authorized.held.clone(),
            service.operation_time,
            launch.deadline,
        )?))
    })
}

/// [`prepare`] with the gate `gate` builds, so tests can pause it.
#[cfg(test)]
pub(crate) fn prepare_with_gate(
    conn: &Connection,
    launch: &ScopedLaunch<'_>,
    now: DateTime<Utc>,
    gate: impl FnOnce(&Authorized) -> anyhow::Result<Box<dyn Gate>>,
) -> Result<Prepared, LaunchRefusal> {
    prepare_gated(conn, launch, now, gate)
}

fn prepare_gated(
    conn: &Connection,
    launch: &ScopedLaunch<'_>,
    now: DateTime<Utc>,
    gate: impl FnOnce(&Authorized) -> anyhow::Result<Box<dyn Gate>>,
) -> Result<Prepared, LaunchRefusal> {
    let authorized = launch_scopes(conn, launch, now)?;
    let gate =
        gate(&authorized).map_err(|error| LaunchRefusal::Unavailable(format!("{error:#}")))?;
    admission::prepare(
        admission::Request {
            coven_home: launch.coven_home,
            envelope: authorized.envelope,
            program: launch.program,
            relay: launch.relay,
            scopes: &authorized.scopes,
            project: launch.project,
            owner_home: launch.owner_home,
            deadline: launch.deadline,
        },
        gate,
    )
    .map_err(LaunchRefusal::Admission)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A home with `sage` registered, a saved owner key, a stand-in claude
    //! 2.1.305, and a project folder outside `COVEN_HOME` holding a granted
    //! note and a sibling the grant does not name.

    use std::fs;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use chrono::{DurationRound, TimeDelta, Utc};
    use serde_json::{json, Value};

    use crate::api::{NoopSessionRuntime, SessionLaunch, SessionRuntime};
    use crate::automations::authority_projection::AutomationAuthorityConsumerProjection;
    use crate::automations::owner_grants::CommandAuthority;
    use crate::automations::scoped_read::admission::Prepared;
    use crate::automations::scoped_read::channel::{relay, CHANNEL_ENV, TOKEN_ENV};
    use crate::automations::scoped_read::credential;
    use crate::familiar_ledger::tests::Home;

    pub(crate) const GRANTED: &str = "granted notes";
    pub(crate) const SIBLING: &str = "ungranted sibling";

    pub(crate) struct Fixture {
        pub home: Home,
        pub project: PathBuf,
        pub outside: PathBuf,
        pub program: PathBuf,
        _scratch: tempfile::TempDir,
    }

    /// A filesystem scope on the project root.
    pub(crate) fn scope(path: &str, recursive: bool) -> Value {
        json!({ "kind": "filesystem", "root": "project", "path": path,
                "access": "read", "recursive": recursive })
    }

    /// An R0 read authority over `scopes`.
    pub(crate) fn r0(scopes: Vec<Value>) -> Value {
        json!({
            "actionType": "analysis.read", "riskClass": "R0",
            "capabilities": ["analysis.read"], "scopes": scopes, "proposalSafe": true
        })
    }

    impl Fixture {
        pub(crate) fn new() -> Self {
            let short = |prefix: &str| {
                tempfile::Builder::new()
                    .prefix(prefix)
                    .tempdir_in("/tmp")
                    .expect("scratch directory")
            };
            let home = Home::in_dir(short("cth"));
            let scratch = short("ctp");
            let project = scratch.path().join("project");
            let outside = scratch.path().join("outside");
            fs::create_dir_all(project.join("notes")).unwrap();
            fs::create_dir_all(&outside).unwrap();
            fs::write(project.join("notes/today.md"), GRANTED).unwrap();
            fs::write(project.join("note.txt"), SIBLING).unwrap();
            fs::write(outside.join("secret.md"), "outside the project").unwrap();
            credential::store(home.path(), "sk-ant-test-capture-only").unwrap();
            let program = scratch.path().join("bin/claude");
            fs::create_dir_all(program.parent().unwrap()).unwrap();
            fs::write(&program, "#!/bin/sh\necho '2.1.305 (Claude Code)'\n").unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            home.register("adopt:ledger:register");
            Self {
                home,
                project,
                outside,
                program,
                _scratch: scratch,
            }
        }

        /// Creates routine `id` over the project, as the owner, so its
        /// creation is the owner grant.
        pub(crate) fn create(&self, id: &str, authority: Value, status: &str) {
            let routine = json!({
                "schemaVersion": 1, "id": id, "name": id, "status": status,
                "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
                "overlap": "forbid", "timeoutMinutes": 30, "runtime": "claude",
                "familiarId": "sage", "cwd": self.project_text(),
                "prompt": "Summarise the notes.", "authority": authority
            });
            let (status, response) = crate::control_plane::route_action_with_authority(
                json!({ "action": "coven.automations.definition.create.v1",
                        "adoptionKey": format!("adopt:{id}:create"), "definition": routine }),
                &self.home.conn,
                &NoopSessionRuntime,
                CommandAuthority::OwnerLocal,
            );
            assert!(status == 200 && response.accepted, "{response:?}");
        }

        pub(crate) fn project_text(&self) -> String {
            self.project.to_str().unwrap().to_owned()
        }

        /// Plans the latest due slot of `id`'s schedule and claims it for the
        /// daemon, as the scheduler does: a real `scheduled` occurrence.
        pub(crate) fn schedule(&self, id: &str) -> String {
            let conn = &self.home.conn;
            let now = Utc::now()
                .duration_trunc(TimeDelta::milliseconds(1))
                .unwrap();
            // The routine was created a moment ago, so no slot of its daily
            // schedule is due yet: date its revision two days back, as if it
            // had been active since, so the latest slot is.
            let since =
                (now - TimeDelta::days(2)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            conn.execute(
                "UPDATE automation_definitions SET created_at = ?2, updated_at = ?2 WHERE id = ?1",
                rusqlite::params![id, since],
            )
            .unwrap();
            let definition: crate::automations::definition::RoutineDefinition =
                serde_json::from_str(
                    &conn
                        .query_row(
                            "SELECT definition_json FROM automation_definitions WHERE id = ?1",
                            [id],
                            |row| row.get::<_, String>(0),
                        )
                        .unwrap(),
                )
                .unwrap();
            assert!(matches!(
                crate::automations::occurrences::plan_latest_due_occurrence(
                    conn,
                    &definition,
                    now - TimeDelta::days(2),
                    now,
                )
                .unwrap(),
                crate::automations::occurrences::PlanOutcome::Planned(_)
            ));
            let occurrence =
                crate::automations::occurrences::claim_due_occurrence(conn, id, "daemon", 60, now)
                    .unwrap()
                    .expect("the planned slot is claimed");
            let kind: String = conn
                .query_row(
                    "SELECT kind FROM automation_occurrences WHERE id = ?1",
                    [&occurrence],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(kind, "scheduled");
            occurrence
        }

        /// A runtime standing in for a daemon that launches the scoped read
        /// envelope with this fixture's harness.
        pub(crate) fn runtime(&self) -> RecordingRuntime {
            RecordingRuntime {
                scoped: Some((self.home.path().to_path_buf(), self.program.clone())),
                ..RecordingRuntime::default()
            }
        }

        /// Runs one daemon dispatch pass with the production adapter.
        pub(crate) fn dispatch_pass(
            &self,
        ) -> (crate::automations::runner::DispatchReport, RecordingRuntime) {
            self.dispatch_pass_with(self.runtime())
        }

        /// [`Self::dispatch_pass`] on `runtime`.
        pub(crate) fn dispatch_pass_with<R: SessionRuntime>(
            &self,
            runtime: R,
        ) -> (crate::automations::runner::DispatchReport, R) {
            let now = Utc::now()
                .duration_trunc(TimeDelta::milliseconds(1))
                .unwrap();
            let report = crate::automations::runner::dispatch_claimed_occurrences_with_clock(
                &self.home.conn,
                &runtime,
                now,
                || now,
            )
            .unwrap();
            (report, runtime)
        }

        /// The Threads store's decision, adoption and consumption rows.
        pub(crate) fn threads_rows(&self) -> (i64, i64, i64) {
            let count = |sql: &str| -> i64 {
                self.home
                    .conn
                    .query_row(sql, [], |row| row.get(0))
                    .unwrap_or(0)
            };
            (
                count("SELECT COUNT(*) FROM automation_threads_decisions"),
                count("SELECT COUNT(*) FROM automation_threads_store_events WHERE kind = 'adoption'"),
                count(
                    "SELECT COUNT(*) FROM automation_threads_store_events WHERE kind = 'consumption'",
                ),
            )
        }

        /// Schedules and dispatches `id` through the runner with the
        /// production adapter, returning the run id and the launch the
        /// runtime was handed.
        pub(crate) fn dispatch(&self, id: &str) -> (String, Launched) {
            self.schedule(id);
            let (report, runtime) = self.dispatch_pass();
            assert_eq!(report.dispatched.len(), 1, "{:?}", report.failed);
            let launched = runtime.launched.into_inner().unwrap();
            assert_eq!(launched.len(), 1);
            (
                report.dispatched[0].clone(),
                launched.into_iter().next().unwrap(),
            )
        }

        pub(crate) fn launch<'a>(&'a self, launched: &'a Launched) -> super::ScopedLaunch<'a> {
            super::ScopedLaunch {
                coven_home: self.home.path(),
                projection: &launched.projection,
                harness: &launched.harness,
                session_id: &launched.id,
                project: Path::new(&launched.cwd),
                program: &self.program,
                relay: Path::new("/usr/local/bin/coven"),
                owner_home: None,
                deadline: None,
            }
        }
    }

    /// What a runtime was asked to launch.
    #[derive(Clone)]
    pub(crate) struct Launched {
        pub id: String,
        pub harness: String,
        pub cwd: String,
        pub projection: AutomationAuthorityConsumerProjection,
    }

    /// Records the launch and its projection, as the daemon receives them,
    /// and establishes ownership without starting anything.
    #[derive(Default)]
    pub(crate) struct RecordingRuntime {
        pub launched: Mutex<Vec<Launched>>,
        /// Launches that carried no projection: ordinary BaseV1 runs.
        pub base: Mutex<usize>,
        /// `COVEN_HOME` and the harness, when this runtime stands in for a
        /// daemon that launches the scoped read envelope. Without them it
        /// preflights as today's daemon does.
        pub scoped: Option<(PathBuf, PathBuf)>,
    }

    impl SessionRuntime for RecordingRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch must use strict containment")
        }

        fn accepts_automation_authority_projection(&self) -> bool {
            true
        }

        fn preflight_authority_launch(
            &self,
            harness: &str,
            envelope: &'static crate::automations::runtime_envelope::RuntimeEnvelope,
        ) -> anyhow::Result<()> {
            let Some((coven_home, program)) = &self.scoped else {
                anyhow::ensure!(
                    crate::automations::runtime_envelope::is_launchable(envelope),
                    crate::api::RUNTIME_AUTHORITY_PROJECTION_UNSUPPORTED
                );
                return Ok(());
            };
            anyhow::ensure!(harness == envelope.harness, "another harness");
            crate::automations::scoped_read::admission::preflight(coven_home, envelope, program)
                .map(|_| ())
                .map_err(|error| anyhow::anyhow!("{error}"))
        }

        fn launch_authorized_contained_adopted_session(
            &self,
            launch: &SessionLaunch,
            authority: Option<&AutomationAuthorityConsumerProjection>,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            let Some(projection) = authority.cloned() else {
                *self.base.lock().unwrap() += 1;
                return ownership_established();
            };
            self.launched.lock().unwrap().push(Launched {
                id: launch.id.clone(),
                harness: launch.harness.clone(),
                cwd: launch.cwd.clone(),
                projection,
            });
            ownership_established()
        }

        fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Runs `requests` through the prepared launch's channel, as the harness
    /// would through the relay, and returns each tool result's text and
    /// whether it was refused. The launch is then finished.
    pub(crate) fn read_through(prepared: Prepared, requests: &[Value]) -> Vec<(Value, bool)> {
        let position = prepared
            .args
            .iter()
            .position(|arg| arg == "--mcp-config")
            .expect("the launch is pointed at its channel");
        let config: Value =
            serde_json::from_slice(&fs::read(&prepared.args[position + 1]).unwrap()).unwrap();
        let server = &config["mcpServers"]["coven"];
        let socket = PathBuf::from(server["env"][CHANNEL_ENV].as_str().unwrap());
        let token = server["env"][TOKEN_ENV].as_str().unwrap().to_owned();
        let mut input = Vec::new();
        let mut messages = vec![json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
                                         "params": { "protocolVersion": "2025-06-18" } })];
        for (index, arguments) in requests.iter().enumerate() {
            let mut arguments = arguments.clone();
            let tool = arguments
                .as_object_mut()
                .and_then(|object| object.remove("tool"))
                .and_then(|tool| tool.as_str().map(str::to_owned))
                .unwrap_or_else(|| "read_file".to_owned());
            messages.push(
                json!({ "jsonrpc": "2.0", "id": index + 1, "method": "tools/call",
                                  "params": { "name": tool, "arguments": arguments } }),
            );
        }
        for message in messages {
            input.extend(serde_json::to_vec(&message).unwrap());
            input.push(b'\n');
        }
        let mut output = Vec::new();
        relay(&socket, &token, Cursor::new(input), &mut output).expect("the relay runs");
        prepared.finish();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .skip(1)
            .map(|line| {
                let reply: Value = serde_json::from_str(line).unwrap();
                let result = &reply["result"];
                let text = result["content"][0]["text"].as_str().unwrap_or_default();
                (
                    serde_json::from_str(text).unwrap_or(Value::Null),
                    result["isError"].as_bool().unwrap_or(true),
                )
            })
            .collect()
    }

    /// A read of `path` on the project root.
    pub(crate) fn read(path: &str) -> Value {
        json!({ "root": "project", "path": path })
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use chrono::Utc;
    use serde_json::{json, Value};

    use super::test_support::{r0, read, read_through, scope, Fixture, GRANTED};
    use super::*;
    use crate::automations::runtime_envelope::{CLAUDE_R0_READ, CLAUDE_R0_SCOPED_READ};

    /// The refusal code a tool result carries, or `None` when it was served.
    fn refused(result: &(Value, bool)) -> Option<String> {
        result
            .1
            .then(|| result.0["refusal"].as_str().unwrap_or_default().to_owned())
    }

    fn content(result: &(Value, bool)) -> &str {
        assert!(!result.1, "refused: {}", result.0);
        result.0["content"].as_str().unwrap_or_default()
    }

    #[test]
    fn a_launch_reads_only_what_the_signed_decision_grants() {
        let fixture = Fixture::new();
        fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
        let (_run, launched) = fixture.dispatch("notes");
        let prepared = prepare(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
            .expect("the granted launch is admitted");
        assert_eq!(
            &prepared.args[..CLAUDE_R0_SCOPED_READ.args.len()],
            CLAUDE_R0_SCOPED_READ.args
        );
        assert!(
            !prepared.cwd.starts_with(&fixture.project),
            "the harness never starts in the project"
        );
        let results = read_through(
            prepared,
            &[
                read("notes/today.md"),
                // #1217's gap: the grant names notes/today.md, not note.txt.
                read("note.txt"),
                json!({ "tool": "list_directory", "root": "project", "path": "notes" }),
                json!({ "tool": "search", "root": "project", "path": "notes", "pattern": "notes" }),
            ],
        );
        assert_eq!(content(&results[0]), GRANTED);
        assert_eq!(refused(&results[1]).as_deref(), Some("scope_missing"));
        assert_eq!(refused(&results[2]).as_deref(), Some("scope_missing"));
        assert_eq!(refused(&results[3]).as_deref(), Some("scope_missing"));
    }

    #[test]
    fn traversal_absolute_and_symlink_escapes_are_refused() {
        let fixture = Fixture::new();
        // A granted name that is a link out of the project, and a granted
        // folder that is one.
        symlink(
            fixture.project.join("note.txt"),
            fixture.project.join("linked.md"),
        )
        .unwrap();
        symlink(&fixture.outside, fixture.project.join("elsewhere")).unwrap();
        fixture.create(
            "notes",
            r0(vec![
                scope("notes/today.md", false),
                scope("linked.md", false),
                scope("elsewhere/secret.md", false),
            ]),
            "ACTIVE",
        );
        let (_run, launched) = fixture.dispatch("notes");
        let prepared = prepare(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
            .expect("the launch is admitted");
        let outside = fixture.outside.join("secret.md");
        let results = read_through(
            prepared,
            &[
                read("notes/today.md"),
                read("../note.txt"),
                read("notes/../note.txt"),
                read("../outside/secret.md"),
                read(outside.to_str().unwrap()),
                read("linked.md"),
                read("elsewhere/secret.md"),
            ],
        );
        assert_eq!(content(&results[0]), GRANTED);
        let codes: Vec<Option<String>> = results[1..].iter().map(refused).collect();
        assert_eq!(
            codes,
            [
                Some("path_traversal".to_owned()),
                Some("path_traversal".to_owned()),
                Some("path_traversal".to_owned()),
                Some("path_absolute".to_owned()),
                Some("resource_symlink".to_owned()),
                Some("resource_symlink".to_owned()),
            ]
        );
    }

    #[test]
    fn a_launch_cannot_widen_the_signed_scopes() {
        let fixture = Fixture::new();
        fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
        let (_run, launched) = fixture.dispatch("notes");
        let conn = &fixture.home.conn;

        // The stored definition is rewritten to grant the sibling after the
        // decision: the launch still serves only what was signed.
        let mut definition: Value = serde_json::from_str(
            &conn
                .query_row(
                    "SELECT definition_json FROM automation_definitions WHERE id = 'notes'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        definition["authority"]["scopes"] =
            json!([scope("notes/today.md", false), scope("note.txt", false)]);
        conn.execute(
            "UPDATE automation_definitions SET definition_json = ?1 WHERE id = 'notes'",
            [definition.to_string()],
        )
        .unwrap();
        let prepared = prepare(conn, &fixture.launch(&launched), Utc::now())
            .expect("the launch is admitted under the signed scopes");
        let results = read_through(prepared, &[read("notes/today.md"), read("note.txt")]);
        assert_eq!(content(&results[0]), GRANTED);
        assert_eq!(refused(&results[1]).as_deref(), Some("scope_missing"));

        // A launch from another folder than the authorized project.
        let mut moved = launched.clone();
        moved.cwd = fixture
            .project
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(matches!(
            prepare(conn, &fixture.launch(&moved), Utc::now()),
            Err(LaunchRefusal::ProjectMismatch)
        ));

        // A projection that pins the unscoped envelope, or another binding.
        let repinned = |edit: fn(&mut Value)| {
            let mut value = serde_json::to_value(&launched.projection).unwrap();
            edit(&mut value);
            let mut edited = launched.clone();
            edited.projection = serde_json::from_value(value).unwrap();
            edited
        };
        let unscoped = repinned(|projection| {
            projection["runtime"]["descriptorDigest"]["value"] =
                json!(CLAUDE_R0_READ.descriptor_digest());
            projection["runtime"]["descriptorVersion"] = json!("r0-read.1");
        });
        assert!(matches!(
            prepare(conn, &fixture.launch(&unscoped), Utc::now()),
            Err(LaunchRefusal::NotScoped)
        ));
        let other = repinned(|projection| {
            projection["bindingDigest"]["value"] = json!("0".repeat(64));
        });
        assert!(matches!(
            prepare(conn, &fixture.launch(&other), Utc::now()),
            Err(LaunchRefusal::BindingMismatch(_))
        ));

        // The recorded request is rewritten to grant the sibling: its
        // signature no longer verifies, so nothing launches.
        let request: String = conn
            .query_row(
                "SELECT request_json FROM automation_threads_decisions",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut request: Value = serde_json::from_str(&request).unwrap();
        request["scopes"] = json!([scope("notes/today.md", false), scope("note.txt", false)]);
        conn.execute_batch("DROP TRIGGER automation_threads_decisions_immutable;")
            .unwrap();
        conn.execute(
            "UPDATE automation_threads_decisions SET request_json = ?1",
            [request.to_string()],
        )
        .unwrap();
        assert!(matches!(
            prepare(conn, &fixture.launch(&launched), Utc::now()),
            Err(LaunchRefusal::DecisionUntrusted(_))
        ));
    }

    #[test]
    fn the_launch_takes_the_signed_scopes_over_the_bound_project() {
        let fixture = Fixture::new();
        fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
        let (_run, launched) = fixture.dispatch("notes");
        let authorized = launch_scopes(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
            .expect("the signed scopes");
        assert_eq!(authorized.envelope, &CLAUDE_R0_SCOPED_READ);
        assert_eq!(authorized.scopes, [scope("notes/today.md", false)]);
        // COVEN_HOME is not the project the attempt was authorized for.
        let mut inside = launched.clone();
        inside.cwd = fixture.home.path().to_str().unwrap().to_owned();
        assert!(matches!(
            prepare(&fixture.home.conn, &fixture.launch(&inside), Utc::now()),
            Err(LaunchRefusal::ProjectMismatch)
        ));
    }

    /// The routine `notes`, dispatched, and the launch its runtime received.
    fn dispatched() -> (Fixture, String, super::test_support::Launched) {
        let fixture = Fixture::new();
        fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
        let (run, launched) = fixture.dispatch("notes");
        (fixture, run, launched)
    }

    /// The code a withdrawn launch was refused with.
    fn withdrawn(
        fixture: &Fixture,
        launched: &super::test_support::Launched,
    ) -> AuthorityProfileErrorCode {
        match prepare(&fixture.home.conn, &fixture.launch(launched), Utc::now()) {
            Err(LaunchRefusal::Withdrawn(code, _)) => code,
            Err(other) => panic!("refused for another reason: {other}"),
            Ok(_) => panic!("a withdrawn attempt was admitted"),
        }
    }

    fn session_of(fixture: &Fixture, run: &str) -> String {
        fixture
            .home
            .conn
            .query_row(
                "SELECT session_id FROM automation_runs WHERE id = ?1",
                [run],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn a_launch_is_refused_once_its_attempt_is_no_longer_live() {
        let (fixture, run, launched) = dispatched();
        let session = session_of(&fixture, &run);
        assert!(crate::store::update_session_terminal_if_active(
            &fixture.home.conn,
            &session,
            "completed",
            Some(0),
            &Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap());
        assert_eq!(
            withdrawn(&fixture, &launched),
            AuthorityProfileErrorCode::Stale
        );
    }

    #[test]
    fn a_launch_is_refused_under_a_stop_fence() {
        let (fixture, run, launched) = dispatched();
        let session = session_of(&fixture, &run);
        crate::automations::runner::claim_stop_fence(
            &fixture.home.conn,
            &run,
            &session,
            "timeout",
            None,
            None,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(
            withdrawn(&fixture, &launched),
            AuthorityProfileErrorCode::FenceStale
        );
    }

    #[test]
    fn a_launch_is_refused_while_a_cancellation_is_pending() {
        for state in ["requested", "stopping"] {
            let (fixture, run, launched) = dispatched();
            let session = session_of(&fixture, &run);
            let conn = &fixture.home.conn;
            crate::automations::cancellation::ensure_cancellation_schema(conn).unwrap();
            let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            conn.execute(
                "INSERT INTO automation_cancellations
                    (adoption_key, request_digest, automation_id, run_id, attempt_id, session_id,
                     scope, requested_by_json, state, requested_at, execution_expires_at)
                 VALUES ('adopt:cancel', ?1, 'notes', ?2, ?3, ?4, 'run', '{}', ?5, ?6, ?6)",
                rusqlite::params![
                    "0".repeat(64),
                    run,
                    launched.projection.attempt_id.as_str(),
                    session,
                    state,
                    now
                ],
            )
            .unwrap();
            assert_eq!(
                withdrawn(&fixture, &launched),
                AuthorityProfileErrorCode::FenceStale,
                "{state}"
            );
        }
    }

    #[test]
    fn a_launch_is_refused_once_the_familiar_revision_is_retired_or_revoked() {
        let (fixture, _run, launched) = dispatched();
        let conn = &fixture.home.conn;
        let revision = launched.projection.identity_revision_id.as_str().to_owned();
        conn.execute(
            "UPDATE familiar_ledger_revisions SET status = 'retired' WHERE revision_id = ?1",
            [&revision],
        )
        .unwrap();
        assert_eq!(
            withdrawn(&fixture, &launched),
            AuthorityProfileErrorCode::FamiliarStatusInvalid
        );
        conn.execute(
            "UPDATE familiar_ledger_revisions
             SET status = 'revoked', revoked_at = '2026-10-27T00:00:00.000Z',
                 revocation_reason = 'compromised'
             WHERE revision_id = ?1",
            [&revision],
        )
        .unwrap();
        assert_eq!(
            withdrawn(&fixture, &launched),
            AuthorityProfileErrorCode::FamiliarStatusInvalid
        );
    }

    #[test]
    fn a_launch_is_refused_once_the_definition_revision_moves_on() {
        let (fixture, _run, launched) = dispatched();
        let mut definition: Value = serde_json::from_str(
            &fixture
                .home
                .conn
                .query_row(
                    "SELECT definition_json FROM automation_definitions WHERE id = 'notes'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        definition["prompt"] = json!("Summarise the notes, briefly.");
        let (status, response) = crate::control_plane::route_action_with_authority(
            json!({ "action": "coven.automations.definition.revise.v1",
                    "adoptionKey": "adopt:notes:revise", "expectedRevision": 1,
                    "definition": definition }),
            &fixture.home.conn,
            &crate::api::NoopSessionRuntime,
            crate::automations::owner_grants::CommandAuthority::OwnerLocal,
        );
        assert!(status == 200 && response.accepted, "{response:?}");
        assert_eq!(
            withdrawn(&fixture, &launched),
            AuthorityProfileErrorCode::DefinitionMismatch
        );
    }

    /// Dispatches a scheduled `notes` on `runtime` and asserts the preflight
    /// refused it with nothing decided, adopted or consumed.
    fn refused_before_deciding<R: crate::api::SessionRuntime>(
        fixture: &Fixture,
        runtime: R,
        why: &str,
    ) {
        fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
        let occurrence = fixture.schedule("notes");
        let (report, _runtime) = fixture.dispatch_pass_with(runtime);
        assert!(report.dispatched.is_empty(), "{why}: {report:?}");
        assert_eq!(
            fixture.threads_rows(),
            (0, 0, 0),
            "{why}: no decision is burned"
        );
        let (state, reason): (String, Option<String>) = fixture
            .home
            .conn
            .query_row(
                "SELECT state, failure_reason FROM automation_occurrences WHERE id = ?1",
                [&occurrence],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "failed", "{why}");
        assert!(
            reason.as_deref().unwrap_or_default().contains("preflight"),
            "{why}: {reason:?}"
        );
    }

    #[test]
    fn an_unadmitted_harness_version_burns_no_decision() {
        let fixture = Fixture::new();
        std::fs::write(
            &fixture.program,
            "#!/bin/sh\necho '2.1.306 (Claude Code)'\n",
        )
        .unwrap();
        refused_before_deciding(&fixture, fixture.runtime(), "version");
    }

    #[test]
    fn a_missing_owner_key_burns_no_decision() {
        let fixture = Fixture::new();
        crate::automations::scoped_read::credential::remove(fixture.home.path()).unwrap();
        refused_before_deciding(&fixture, fixture.runtime(), "key");
    }

    #[test]
    fn an_envelope_the_runtime_cannot_launch_burns_no_decision() {
        let fixture = Fixture::new();
        // Today's daemon launches only LAUNCHABLE envelopes, and r0-read.2
        // is not one.
        refused_before_deciding(
            &fixture,
            super::test_support::RecordingRuntime::default(),
            "envelope",
        );
    }

    #[test]
    fn a_runtime_without_authority_support_burns_no_decision() {
        let fixture = Fixture::new();
        refused_before_deciding(&fixture, crate::api::NoopSessionRuntime, "runtime");
    }
}
