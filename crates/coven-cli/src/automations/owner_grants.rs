//! Owner grants: what authorizes a Runtime Authority run (coven#857, slice 2).
//!
//! The maintainer decisions in
//! `docs/architecture/coven-automations-runtime-authority.md` make the
//! principal the owner-local OS identity. A command is owner-authenticated
//! when it arrived over the owner-only socket or pipe, or in the owner's own
//! process; #1164 refuses automation mutations on every other transport. The
//! command executor records that as [`CommandAuthority`].
//!
//! Every run binds to an owner grant: the adopted owner command that
//! authorized it. A scheduled run is created by the scheduler, so no command
//! arrives with it. Its grant is the owner command that made its current
//! revision active: `definition.activate.v1`, or a `definition.create.v1` or
//! `definition.revise.v1` that leaves the definition active. The grant is
//! recorded in the same transaction as that command, with its adoption key
//! and request digest. A revision that became active any other way (a legacy
//! import, a migration, an unversioned update, or a command that was not
//! owner-authenticated) has no grant, and [`authorize_scheduled_dispatch`]
//! refuses it. So does a revision that is no longer current or active when the
//! attempt dispatches: the grant authorized that revision, not its successors.
//!
//! Manual run grants arrive with the versioned run-now command (slice 7).
//! Nothing here constructs Runtime Authority; the trusted adapter (slice 6)
//! maps [`OwnerAuthorization`] into the execution binding.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{params, Connection, OptionalExtension};

/// How the command executor's caller authenticated the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAuthority {
    /// The owner-only socket or pipe, or the owner's own process.
    OwnerLocal,
    /// Any other transport: it may still read, but it grants nothing.
    Unauthenticated,
}

/// The dispatch operation an activation grant authorizes.
pub(crate) const SCHEDULED_DISPATCH_OPERATION: &str = "automation.dispatch.scheduled";

pub(crate) const AUTOMATION_OWNER_GRANTS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_owner_principal (
        singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
        principal_id TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL
    );

    -- One grant per active revision: each is created by exactly one command.
    CREATE TABLE IF NOT EXISTS automation_owner_grants (
        automation_id TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision >= 1),
        command TEXT NOT NULL CHECK (
            command IN ('definition.create.v1', 'definition.activate.v1', 'definition.revise.v1')
        ),
        adoption_key TEXT NOT NULL,
        request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
        authority TEXT NOT NULL CHECK (authority = 'owner_local'),
        principal_id TEXT NOT NULL,
        granted_at TEXT NOT NULL,
        PRIMARY KEY (automation_id, revision)
    );
";

/// The owner command that made one revision active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerGrant {
    pub automation_id: String,
    pub revision: u64,
    pub command: String,
    pub adoption_key: String,
    /// Lowercase hex SHA-256 of the command's JCS request, as adopted.
    pub request_digest: String,
    pub principal_id: String,
    pub granted_at: String,
}

/// What a scheduled attempt presents: the owner principal, its grant, and a
/// nonce and validity window issued for this attempt alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerAuthorization {
    pub principal_id: String,
    pub operation: &'static str,
    pub grant: OwnerGrant,
    pub nonce: String,
    pub issued_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
}

/// Why a scheduled attempt has no owner authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnerAuthorizationRefusal {
    /// The revision became active without an owner-authenticated command.
    NoGrant {
        automation_id: String,
        revision: u64,
    },
    /// The definition has moved on, or never existed: its current revision is
    /// not the one this attempt would run.
    NotCurrent {
        automation_id: String,
        revision: u64,
        current_revision: Option<u64>,
    },
    /// The revision is current but no longer active (paused, disabled or
    /// tombstoned).
    NotActive {
        automation_id: String,
        revision: u64,
        lifecycle_state: String,
    },
}

pub(crate) fn ensure_owner_grants_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(AUTOMATION_OWNER_GRANTS_SCHEMA_SQL)
        .context("failed to initialize automation owner grants schema")
}

/// The owner's stable, opaque principal identifier, created on first use. It
/// names the owner of this store, never the numeric uid.
pub(crate) fn owner_principal_id(conn: &Connection, now: &str) -> Result<String> {
    ensure_owner_grants_schema(conn)?;
    if let Some(existing) = read_owner_principal_id(conn)? {
        return Ok(existing);
    }
    let principal_id = format!("principal:coven-local-owner:{}", random_hex()?);
    // A concurrent first use keeps whichever identifier committed first.
    conn.execute(
        "INSERT OR IGNORE INTO automation_owner_principal (singleton, principal_id, created_at)
         VALUES (1, ?1, ?2)",
        params![principal_id, now],
    )
    .context("failed to record the owner principal")?;
    read_owner_principal_id(conn)?.context("owner principal is missing after creation")
}

fn read_owner_principal_id(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT principal_id FROM automation_owner_principal WHERE singleton = 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .context("failed to read the owner principal")
}

/// Records the grant for a committed command when it made `revision` active
/// and was owner-authenticated. Called inside the command's own transaction,
/// so the grant commits or rolls back with the revision it authorizes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_grant_if_activating(
    conn: &Connection,
    authority: CommandAuthority,
    command: &str,
    lifecycle_state: &str,
    automation_id: &str,
    revision: u64,
    adoption_key: &str,
    request_digest: &str,
    granted_at: &str,
) -> Result<bool> {
    let activating = matches!(
        command,
        "definition.create.v1" | "definition.activate.v1" | "definition.revise.v1"
    ) && lifecycle_state == "active";
    if authority != CommandAuthority::OwnerLocal || !activating {
        return Ok(false);
    }
    let principal_id = owner_principal_id(conn, granted_at)?;
    conn.execute(
        "INSERT INTO automation_owner_grants
            (automation_id, revision, command, adoption_key, request_digest, authority,
             principal_id, granted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'owner_local', ?6, ?7)",
        params![
            automation_id,
            i64::try_from(revision).context("automation revision exceeds SQLite range")?,
            command,
            adoption_key,
            request_digest,
            principal_id,
            granted_at,
        ],
    )
    .context("failed to record the owner grant")?;
    Ok(true)
}

/// The grant that made `revision` of `automation_id` active, if any.
pub(crate) fn activation_grant(
    conn: &Connection,
    automation_id: &str,
    revision: u64,
) -> Result<Option<OwnerGrant>> {
    ensure_owner_grants_schema(conn)?;
    conn.query_row(
        "SELECT automation_id, revision, command, adoption_key, request_digest, principal_id,
                granted_at
         FROM automation_owner_grants
         WHERE automation_id = ?1 AND revision = ?2",
        params![
            automation_id,
            i64::try_from(revision).context("automation revision exceeds SQLite range")?,
        ],
        |row| {
            let revision: i64 = row.get(1)?;
            Ok(OwnerGrant {
                automation_id: row.get(0)?,
                revision: u64::try_from(revision).unwrap_or_default(),
                command: row.get(2)?,
                adoption_key: row.get(3)?,
                request_digest: row.get(4)?,
                principal_id: row.get(5)?,
                granted_at: row.get(6)?,
            })
        },
    )
    .optional()
    .context("failed to read the owner grant")
}

/// The owner authorization for one scheduled attempt of `revision`, with a
/// fresh nonce valid from `now` for `validity`. Refuses unless `revision` is
/// the definition's current, active revision and has an owner grant, all read
/// now rather than when the occurrence was planned. The attempt's occurrence
/// fence and identity, which the execution binding pins beside this and the
/// runner matches against the claimed attempt, are what make a replayed
/// binding stale.
pub(crate) fn authorize_scheduled_dispatch(
    conn: &Connection,
    automation_id: &str,
    revision: u64,
    now: DateTime<Utc>,
    validity: TimeDelta,
) -> Result<std::result::Result<OwnerAuthorization, OwnerAuthorizationRefusal>> {
    // Contract timestamps carry at most milliseconds, so a shorter or
    // fractional window could not be stated without collapsing or shifting it.
    anyhow::ensure!(
        validity.num_milliseconds() > 0
            && validity == TimeDelta::milliseconds(validity.num_milliseconds()),
        "an owner authorization needs a positive, whole-millisecond validity"
    );
    let current = match super::store::get_definition_with_tombstone(conn, automation_id, true)? {
        Some(record) if record.revision == revision => record,
        other => {
            return Ok(Err(OwnerAuthorizationRefusal::NotCurrent {
                automation_id: automation_id.to_owned(),
                revision,
                current_revision: other.map(|record| record.revision),
            }))
        }
    };
    if current.tombstoned_at.is_some() || current.lifecycle_state != "active" {
        return Ok(Err(OwnerAuthorizationRefusal::NotActive {
            automation_id: automation_id.to_owned(),
            revision,
            lifecycle_state: if current.tombstoned_at.is_some() {
                "tombstoned".to_owned()
            } else {
                current.lifecycle_state
            },
        }));
    }
    let Some(grant) = activation_grant(conn, automation_id, revision)? else {
        return Ok(Err(OwnerAuthorizationRefusal::NoGrant {
            automation_id: automation_id.to_owned(),
            revision,
        }));
    };
    let issued_at = chrono::DurationRound::duration_trunc(now, TimeDelta::milliseconds(1))
        .context("authorization time cannot be represented")?;
    Ok(Ok(OwnerAuthorization {
        principal_id: grant.principal_id.clone(),
        operation: SCHEDULED_DISPATCH_OPERATION,
        nonce: format!("nonce:{}", random_hex()?),
        issued_at,
        valid_until: issued_at + validity,
        grant,
    }))
}

fn random_hex() -> Result<String> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to draw a random identifier"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NoopSessionRuntime;
    use crate::control_plane::{route_action, route_action_with_authority};
    use chrono::TimeZone;
    use serde_json::{json, Value};

    fn store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    fn routine(status: &str, prompt: &str) -> Value {
        json!({
            "schemaVersion": 1, "id": "owned", "name": "Owned", "status": status,
            "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
            "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code", "prompt": prompt
        })
    }

    fn act(conn: &Connection, authority: CommandAuthority, request: Value) {
        let (status, response) =
            route_action_with_authority(request, conn, &NoopSessionRuntime, authority);
        assert!(status == 200 && response.accepted, "{response:?}");
    }

    fn lifecycle(conn: &Connection, command: &str, key: &str, revision: u64) {
        act(
            conn,
            CommandAuthority::OwnerLocal,
            json!({
                "action": format!("coven.automations.definition.{command}.v1"),
                "id": "owned", "adoptionKey": key, "expectedRevision": revision,
            }),
        );
    }

    fn adopted_digest(conn: &Connection, key: &str) -> String {
        conn.query_row(
            "SELECT request_digest FROM automation_command_adoptions WHERE adoption_key = ?1",
            [key],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn grant_count(conn: &Connection) -> i64 {
        ensure_owner_grants_schema(conn).unwrap();
        conn.query_row("SELECT COUNT(*) FROM automation_owner_grants", [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 3, 9, 0, 0).unwrap() + TimeDelta::microseconds(250_400)
    }

    #[test]
    fn owner_activation_and_active_revisions_are_granted() {
        let (_temp, conn) = store();
        act(
            &conn,
            CommandAuthority::OwnerLocal,
            json!({
                "action": "coven.automations.definition.create.v1",
                "adoptionKey": "adopt:grant:create", "definition": routine("PAUSED", "First."),
            }),
        );
        assert_eq!(
            activation_grant(&conn, "owned", 1).unwrap(),
            None,
            "a paused draft"
        );

        lifecycle(&conn, "activate", "adopt:grant:activate", 1);
        let activated = activation_grant(&conn, "owned", 2)
            .unwrap()
            .expect("activation grant");
        assert_eq!(activated.command, "definition.activate.v1");
        assert_eq!(activated.adoption_key, "adopt:grant:activate");
        assert_eq!(
            activated.request_digest,
            adopted_digest(&conn, "adopt:grant:activate")
        );

        // A revise of an active definition makes its new revision active.
        act(
            &conn,
            CommandAuthority::OwnerLocal,
            json!({
                "action": "coven.automations.definition.revise.v1",
                "adoptionKey": "adopt:grant:revise", "expectedRevision": 2,
                "definition": routine("ACTIVE", "Second."),
            }),
        );
        let revised = activation_grant(&conn, "owned", 3)
            .unwrap()
            .expect("revise grant");
        assert_eq!(revised.command, "definition.revise.v1");
        assert_eq!(
            revised.request_digest,
            adopted_digest(&conn, "adopt:grant:revise")
        );

        lifecycle(&conn, "pause", "adopt:grant:pause", 3);
        assert_eq!(
            activation_grant(&conn, "owned", 4).unwrap(),
            None,
            "pausing grants nothing"
        );
        lifecycle(&conn, "activate", "adopt:grant:reactivate", 4);
        assert!(activation_grant(&conn, "owned", 5).unwrap().is_some());

        // A replay commits nothing new, so it records no second grant.
        let before = grant_count(&conn);
        lifecycle(&conn, "activate", "adopt:grant:reactivate", 4);
        assert_eq!(grant_count(&conn), before);

        // Every grant names the same stable, contract-valid owner principal.
        let principal = owner_principal_id(&conn, "2026-10-03T09:00:00.000Z").unwrap();
        assert!(principal.starts_with("principal:coven-local-owner:") && principal.len() <= 128);
        assert!(principal
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:@-".contains(&byte)));
        for revision in [2, 3, 5] {
            assert_eq!(
                activation_grant(&conn, "owned", revision)
                    .unwrap()
                    .unwrap()
                    .principal_id,
                principal
            );
        }
    }

    #[test]
    fn activations_without_an_owner_command_have_no_grant() {
        let (_temp, conn) = store();
        act(
            &conn,
            CommandAuthority::OwnerLocal,
            json!({
                "action": "coven.automations.definition.create.v1",
                "adoptionKey": "adopt:nogrant:create", "definition": routine("PAUSED", "First."),
            }),
        );
        // An activation the executor did not see as owner-authenticated
        // commits, but authorizes no run.
        act(
            &conn,
            CommandAuthority::Unauthenticated,
            json!({
                "action": "coven.automations.definition.activate.v1",
                "id": "owned", "adoptionKey": "adopt:nogrant:activate", "expectedRevision": 1,
            }),
        );
        let record = crate::automations::store::get_definition(&conn, "owned")
            .unwrap()
            .unwrap();
        assert_eq!(
            (record.revision, record.lifecycle_state.as_str()),
            (2, "active")
        );
        assert_eq!(activation_grant(&conn, "owned", 2).unwrap(), None);
        assert_eq!(
            authorize_scheduled_dispatch(&conn, "owned", 2, at(), TimeDelta::minutes(5)).unwrap(),
            Err(OwnerAuthorizationRefusal::NoGrant {
                automation_id: "owned".to_owned(),
                revision: 2
            })
        );

        // The unversioned legacy create can make a routine active, but grants nothing.
        let (status, response) = route_action(
            json!({ "action": "coven.automations.create", "definition": {
                "schemaVersion": 1, "id": "legacy", "name": "Legacy", "status": "ACTIVE",
                "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
                "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code",
                "prompt": "Legacy."
            }}),
            &conn,
            &NoopSessionRuntime,
        );
        assert_eq!(status, 200, "{response:?}");
        assert_eq!(grant_count(&conn), 0);
    }

    #[test]
    fn scheduled_dispatch_gets_a_fresh_nonce_and_window_for_each_attempt() {
        let (_temp, conn) = store();
        act(
            &conn,
            CommandAuthority::OwnerLocal,
            json!({
                "action": "coven.automations.definition.create.v1",
                "adoptionKey": "adopt:nonce:create", "definition": routine("PAUSED", "First."),
            }),
        );
        lifecycle(&conn, "activate", "adopt:nonce:activate", 1);
        let first = authorize_scheduled_dispatch(&conn, "owned", 2, at(), TimeDelta::minutes(5))
            .unwrap()
            .unwrap();
        let second = authorize_scheduled_dispatch(&conn, "owned", 2, at(), TimeDelta::minutes(5))
            .unwrap()
            .unwrap();
        assert_ne!(first.nonce, second.nonce);
        assert!(first.nonce.starts_with("nonce:") && first.nonce.len() == "nonce:".len() + 32);
        assert_eq!(first.operation, SCHEDULED_DISPATCH_OPERATION);
        assert_eq!(
            first.grant,
            activation_grant(&conn, "owned", 2).unwrap().unwrap()
        );
        assert_eq!(first.principal_id, first.grant.principal_id);
        let truncated =
            Utc.with_ymd_and_hms(2026, 10, 3, 9, 0, 0).unwrap() + TimeDelta::milliseconds(250);
        assert_eq!(
            (first.issued_at, first.valid_until),
            (truncated, truncated + TimeDelta::minutes(5))
        );
        for invalid in [
            TimeDelta::zero(),
            TimeDelta::milliseconds(-1),
            TimeDelta::microseconds(1),
            TimeDelta::microseconds(1_500),
        ] {
            assert!(
                authorize_scheduled_dispatch(&conn, "owned", 2, at(), invalid).is_err(),
                "{invalid:?}"
            );
        }
        let shortest =
            authorize_scheduled_dispatch(&conn, "owned", 2, at(), TimeDelta::milliseconds(1))
                .unwrap()
                .unwrap();
        assert_eq!(
            shortest.valid_until - shortest.issued_at,
            TimeDelta::milliseconds(1)
        );
    }

    #[test]
    fn dispatch_refuses_a_revision_that_is_no_longer_current_and_active() {
        let (_temp, conn) = store();
        let authorize = |revision: u64| {
            authorize_scheduled_dispatch(&conn, "owned", revision, at(), TimeDelta::minutes(5))
                .unwrap()
        };
        let not_current = |revision: u64, current: Option<u64>| {
            Err(OwnerAuthorizationRefusal::NotCurrent {
                automation_id: "owned".to_owned(),
                revision,
                current_revision: current,
            })
        };
        let not_active = |revision: u64, state: &str| {
            Err(OwnerAuthorizationRefusal::NotActive {
                automation_id: "owned".to_owned(),
                revision,
                lifecycle_state: state.to_owned(),
            })
        };
        assert_eq!(authorize(1), not_current(1, None), "no definition yet");

        // Creating a definition as active grants its first revision.
        act(
            &conn,
            CommandAuthority::OwnerLocal,
            json!({
                "action": "coven.automations.definition.create.v1",
                "adoptionKey": "adopt:stale:create", "definition": routine("ACTIVE", "First."),
            }),
        );
        let created = authorize(1).expect("active create grant");
        assert_eq!(created.grant.command, "definition.create.v1");
        assert_eq!(
            created.grant.request_digest,
            adopted_digest(&conn, "adopt:stale:create")
        );

        // A revise supersedes revision 1: its grant no longer authorizes runs.
        act(
            &conn,
            CommandAuthority::OwnerLocal,
            json!({
                "action": "coven.automations.definition.revise.v1",
                "adoptionKey": "adopt:stale:revise", "expectedRevision": 1,
                "definition": routine("ACTIVE", "Second."),
            }),
        );
        assert!(activation_grant(&conn, "owned", 1).unwrap().is_some());
        assert_eq!(authorize(1), not_current(1, Some(2)));
        assert!(authorize(2).is_ok());

        lifecycle(&conn, "pause", "adopt:stale:pause", 2);
        assert_eq!(authorize(2), not_current(2, Some(3)));
        assert_eq!(authorize(3), not_active(3, "paused"));

        lifecycle(&conn, "activate", "adopt:stale:activate", 3);
        assert!(authorize(4).is_ok());
        lifecycle(&conn, "tombstone", "adopt:stale:tombstone", 4);
        assert_eq!(authorize(4), not_current(4, Some(5)));
        assert_eq!(authorize(5), not_active(5, "tombstoned"));
    }

    #[test]
    fn the_api_grants_owner_ipc_activations_and_refuses_tcp_mutations() {
        use crate::request_authority::RequestAuthority;
        let temp = tempfile::tempdir().unwrap();
        let post = |authority: RequestAuthority, body: Value| {
            crate::api::handle_request_with_runtime_and_authority(
                "POST",
                "/api/v1/actions",
                temp.path(),
                None,
                Some(&body.to_string()),
                &NoopSessionRuntime,
                authority,
            )
            .unwrap()
        };
        let created = post(
            RequestAuthority::OwnerLocalIpc,
            json!({
                "action": "coven.automations.definition.create.v1",
                "adoptionKey": "adopt:api:create", "definition": routine("PAUSED", "First."),
            }),
        );
        assert_eq!(created.status, 200, "{}", created.body);
        let activate = json!({
            "action": "coven.automations.definition.activate.v1",
            "id": "owned", "adoptionKey": "adopt:api:activate", "expectedRevision": 1,
        });
        assert_ne!(
            post(RequestAuthority::Tcp, activate.clone()).status,
            200,
            "TCP cannot mutate"
        );
        let activated = post(RequestAuthority::OwnerLocalIpc, activate);
        assert_eq!(activated.status, 200, "{}", activated.body);

        let conn = crate::store::open_store(&crate::api::store_path(temp.path())).unwrap();
        let grant = activation_grant(&conn, "owned", 2)
            .unwrap()
            .expect("owner IPC grant");
        assert_eq!(grant.adoption_key, "adopt:api:activate");
        assert_eq!(grant_count(&conn), 1);
    }
}
