//! The familiar root and revision ledger (coven#857, slice 3).
//!
//! Runtime Authority Decision 2 makes the daemon the authority for familiar
//! identity: it mints each familiar's stable root, keeps that root's revision
//! history, and signs the Familiar Contract evidence that a binding cites.
//! This module is that ledger; the binding issuer reads it.
//!
//! **Only owner commands change it.** Coven has no committed declaration
//! state of its own: `SOUL.md`, `IDENTITY.md` and `ward.toml` are read from
//! disk, and a familiar's roster entry can change out of band. So nothing here
//! is minted on observation. An owner-local command registers a familiar
//! (genesis), adopts its current declarations as a new revision, retires it,
//! revokes a revision, or restores a retired familiar. The issuer re-reads the
//! declarations and refuses when they no longer match the head, so a changed
//! declaration needs an explicit `adopt` before it can be embodied.
//!
//! **What a revision records.** A Familiar Contract identity bundle
//! (`familiar.identity_bundle.v1`) with three components, each a JSON object
//! that wraps the exact declaration text:
//!
//! - `identity-declaration`: the roster's identity fields (`id`, `name`,
//!   `displayName`, `role`, `pronouns`, `person`, `coven`) and `IDENTITY.md`.
//!   Its digest is the binding's `declarationDigest`, so renaming or re-roling
//!   a familiar is an identity change.
//! - `soul-declaration`: `SOUL.md`.
//! - `ward-declaration`: `ward.toml`, when the workspace has one.
//!
//! Both Markdown files are required; a familiar without them cannot be
//! registered or adopted. The bundle is stored exactly as built and never
//! rewritten, because its digest covers its retention state too. Redacted
//! forms, when they exist, are history only.
//!
//! **Lineage.** A root is `familiar:<32 hex>`, never derived from the roster
//! id, which is an unvalidated alias that can be reused. A revision is
//! `familiar-revision:<root hex>:<position>`. Each revision after genesis
//! carries a transition signed with the daemon's `familiar-binding` key.
//! Statuses are the contract's own: `active`, `superseded`, `retired` and
//! `revoked`; the issuer maps them to Coven's vocabulary at its boundary.
//!
//! **Generation.** Every change to a root bumps its generation by one. The
//! issuer's trusted-ledger observation carries it, so a binding names the exact
//! ledger state it was decided against.

use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Map, Value};

use crate::automations::authority_keys::{self, AuthorityKeyRole, RoleSigningKey};
use crate::automations::contract::error::{ErrorCode, ErrorEnvelope};
use crate::automations::owner_grants::{self, CommandAuthority};
use crate::cockpit_sources::{self, RosterIdentity, RosterLookup};
use crate::control_plane::{
    automation_error, typed_rejection, ActionStatus, ControlActionResponse,
};

/// Every ledger action starts with this prefix; all of them need owner-local
/// IPC.
pub(crate) const ACTION_PREFIX: &str = "coven.familiars.ledger.";
const REGISTER: &str = "coven.familiars.ledger.register.v1";
const ADOPT: &str = "coven.familiars.ledger.adopt.v1";
const RETIRE: &str = "coven.familiars.ledger.retire.v1";
const REVOKE: &str = "coven.familiars.ledger.revoke.v1";
const RESTORE: &str = "coven.familiars.ledger.restore.v1";
const GET: &str = "coven.familiars.ledger.get.v1";

/// A revocation reason is bounded like the definition commands' reasons.
const MAX_REASON_BYTES: usize = 500;

pub(crate) const FAMILIAR_LEDGER_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS familiar_ledger_roots (
        root_id TEXT PRIMARY KEY NOT NULL,
        roster_id TEXT NOT NULL,
        generation INTEGER NOT NULL CHECK (generation >= 1),
        created_at TEXT NOT NULL,
        retired_at TEXT
    );
    -- The roster id is an alias: at most one live root answers to it.
    CREATE UNIQUE INDEX IF NOT EXISTS familiar_ledger_one_live_root
        ON familiar_ledger_roots (roster_id) WHERE retired_at IS NULL;
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_root_identity_immutable
    BEFORE UPDATE OF root_id, roster_id, created_at ON familiar_ledger_roots
    BEGIN SELECT RAISE(ABORT, 'familiar ledger root identity is immutable'); END;
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_root_generation_steps
    BEFORE UPDATE OF generation ON familiar_ledger_roots
    WHEN NEW.generation <> OLD.generation + 1
    BEGIN SELECT RAISE(ABORT, 'familiar ledger generation advances by one'); END;
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_roots_retained
    BEFORE DELETE ON familiar_ledger_roots
    BEGIN SELECT RAISE(ABORT, 'familiar ledger roots are retained'); END;

    CREATE TABLE IF NOT EXISTS familiar_ledger_revisions (
        revision_id TEXT PRIMARY KEY NOT NULL,
        root_id TEXT NOT NULL REFERENCES familiar_ledger_roots (root_id),
        lineage_position INTEGER NOT NULL CHECK (lineage_position >= 0),
        relationship TEXT NOT NULL
            CHECK (relationship IN ('genesis', 'same_familiar_revision', 'restoration')),
        predecessor_revision_id TEXT REFERENCES familiar_ledger_revisions (revision_id),
        transition_json TEXT,
        declaration_digest TEXT NOT NULL CHECK (length(declaration_digest) = 64),
        bundle_digest TEXT NOT NULL CHECK (length(bundle_digest) = 64),
        bundle_json TEXT NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'retired', 'revoked')),
        recorded_at TEXT NOT NULL,
        valid_from TEXT NOT NULL,
        valid_until TEXT,
        revoked_at TEXT,
        revocation_reason TEXT,
        UNIQUE (root_id, lineage_position),
        CHECK ((relationship = 'genesis') = (lineage_position = 0)),
        CHECK ((relationship = 'genesis') = (predecessor_revision_id IS NULL)),
        CHECK ((predecessor_revision_id IS NULL) = (transition_json IS NULL)),
        CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
        CHECK ((revoked_at IS NULL) = (revocation_reason IS NULL))
    );
    CREATE UNIQUE INDEX IF NOT EXISTS familiar_ledger_one_active_revision
        ON familiar_ledger_revisions (root_id) WHERE status = 'active';
    -- A revision's evidence never changes once recorded.
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_revision_evidence_immutable
    BEFORE UPDATE OF revision_id, root_id, lineage_position, relationship,
        predecessor_revision_id, transition_json, declaration_digest, bundle_digest,
        bundle_json, recorded_at, valid_from ON familiar_ledger_revisions
    BEGIN SELECT RAISE(ABORT, 'familiar ledger revision evidence is immutable'); END;
    -- Status only moves forward: an active revision is superseded, retired or
    -- revoked, and a superseded or retired one can still be revoked.
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_revision_status_forward
    BEFORE UPDATE OF status ON familiar_ledger_revisions
    WHEN NOT (NEW.status = OLD.status
        OR (OLD.status = 'active' AND NEW.status IN ('superseded', 'retired', 'revoked'))
        OR (OLD.status IN ('superseded', 'retired') AND NEW.status = 'revoked'))
    BEGIN SELECT RAISE(ABORT, 'familiar ledger revision status only moves forward'); END;
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_revisions_retained
    BEFORE DELETE ON familiar_ledger_revisions
    BEGIN SELECT RAISE(ABORT, 'familiar ledger revisions are retained'); END;

    -- One row per change to a root, numbered by the generation it produced.
    CREATE TABLE IF NOT EXISTS familiar_ledger_events (
        root_id TEXT NOT NULL REFERENCES familiar_ledger_roots (root_id),
        generation INTEGER NOT NULL CHECK (generation >= 1),
        event TEXT NOT NULL
            CHECK (event IN ('registered', 'adopted', 'retired', 'revoked', 'restored')),
        revision_id TEXT NOT NULL REFERENCES familiar_ledger_revisions (revision_id),
        adoption_key TEXT NOT NULL,
        request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
        principal_id TEXT NOT NULL,
        recorded_at TEXT NOT NULL,
        PRIMARY KEY (root_id, generation)
    );
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_events_immutable
    BEFORE UPDATE ON familiar_ledger_events
    BEGIN SELECT RAISE(ABORT, 'familiar ledger events are append-only'); END;
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_events_retained
    BEFORE DELETE ON familiar_ledger_events
    BEGIN SELECT RAISE(ABORT, 'familiar ledger events are append-only'); END;

    -- Adopted commands, so a retried command replays its first answer.
    CREATE TABLE IF NOT EXISTS familiar_ledger_commands (
        adoption_key TEXT PRIMARY KEY NOT NULL,
        action TEXT NOT NULL,
        request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
        response_json TEXT NOT NULL,
        recorded_at TEXT NOT NULL
    );
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_commands_immutable
    BEFORE UPDATE ON familiar_ledger_commands
    BEGIN SELECT RAISE(ABORT, 'familiar ledger commands are append-only'); END;
    CREATE TRIGGER IF NOT EXISTS familiar_ledger_commands_retained
    BEFORE DELETE ON familiar_ledger_commands
    BEGIN SELECT RAISE(ABORT, 'familiar ledger commands are append-only'); END;
";

pub(crate) fn ensure_familiar_ledger_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(FAMILIAR_LEDGER_SCHEMA_SQL)
        .context("failed to initialize familiar ledger schema")
}

/// One familiar's declarations, as revision components.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Declarations {
    identity: Value,
    soul: Value,
    ward: Option<Value>,
}

/// Why a familiar's declarations cannot be recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclarationRefusal {
    NotInRoster,
    DuplicateRosterId,
    Missing(&'static str),
    NotUtf8(&'static str),
    InvalidWard(String),
}

impl DeclarationRefusal {
    fn envelope(&self, familiar_id: &str) -> ErrorEnvelope {
        let (code, message) = match self {
            Self::NotInRoster => (
                ErrorCode::NotFound,
                format!("familiar `{familiar_id}` is not in familiars.toml"),
            ),
            Self::DuplicateRosterId => (
                ErrorCode::ValidationFailed,
                format!("familiar `{familiar_id}` appears more than once in familiars.toml"),
            ),
            Self::Missing(file) => (
                ErrorCode::ValidationFailed,
                format!(
                    "familiar `{familiar_id}` has no {file}; add it before recording its identity"
                ),
            ),
            Self::NotUtf8(file) => (
                ErrorCode::ValidationFailed,
                format!("familiar `{familiar_id}`'s {file} is not UTF-8 text"),
            ),
            Self::InvalidWard(error) => (
                ErrorCode::ValidationFailed,
                format!("familiar `{familiar_id}`'s ward.toml is invalid: {error}"),
            ),
        };
        automation_error(code, message)
    }
}

/// Reads `familiar_id`'s current declarations from the roster and its
/// workspace, confined to the workspace and refusing symlinks.
pub(crate) fn read_declarations(
    coven_home: &Path,
    familiar_id: &str,
) -> Result<std::result::Result<Declarations, DeclarationRefusal>> {
    let roster = match cockpit_sources::roster_identity(coven_home, familiar_id)? {
        RosterLookup::Found(roster) => roster,
        RosterLookup::Missing => return Ok(Err(DeclarationRefusal::NotInRoster)),
        RosterLookup::Duplicate => return Ok(Err(DeclarationRefusal::DuplicateRosterId)),
    };
    let read =
        |file: &'static str| -> Result<std::result::Result<Option<String>, DeclarationRefusal>> {
            let Some(bytes) = crate::threads_gate::read_surface_if_exists(&roster.workspace, file)
                .with_context(|| format!("failed to read {file} for familiar `{familiar_id}`"))?
            else {
                return Ok(Ok(None));
            };
            Ok(String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| DeclarationRefusal::NotUtf8(file)))
        };
    let identity_text = match read("IDENTITY.md")? {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(Err(DeclarationRefusal::Missing("IDENTITY.md"))),
        Err(refusal) => return Ok(Err(refusal)),
    };
    let soul_text = match read("SOUL.md")? {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(Err(DeclarationRefusal::Missing("SOUL.md"))),
        Err(refusal) => return Ok(Err(refusal)),
    };
    let ward_text = match read("ward.toml")? {
        Ok(text) => text,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if let Some(text) = &ward_text {
        if let Err(error) = crate::ward::WardConfig::from_toml_str(text) {
            return Ok(Err(DeclarationRefusal::InvalidWard(format!("{error:#}"))));
        }
    }
    Ok(Ok(Declarations {
        identity: json!({
            "roster": roster_fields(&roster),
            "mediaType": "text/markdown",
            "text": identity_text,
        }),
        soul: json!({ "mediaType": "text/markdown", "text": soul_text }),
        ward: ward_text.map(|text| json!({ "mediaType": "application/toml", "text": text })),
    }))
}

/// The roster's identity fields, with absent optional fields left out.
fn roster_fields(roster: &RosterIdentity) -> Value {
    let mut fields = Map::new();
    fields.insert("id".into(), json!(roster.id));
    fields.insert("displayName".into(), json!(roster.display_name));
    fields.insert("role".into(), json!(roster.role));
    for (key, value) in [
        ("name", &roster.name),
        ("pronouns", &roster.pronouns),
        ("person", &roster.person),
        ("coven", &roster.coven),
    ] {
        if let Some(value) = value {
            fields.insert(key.into(), json!(value));
        }
    }
    Value::Object(fields)
}

impl Declarations {
    fn components(&self) -> Vec<(&'static str, &Value)> {
        let mut components = vec![
            ("identity-declaration", &self.identity),
            ("soul-declaration", &self.soul),
        ];
        if let Some(ward) = &self.ward {
            components.push(("ward-declaration", ward));
        }
        components
    }

    fn declaration_digest(&self) -> String {
        familiar_contract::digest_object(&self.identity)
    }
}

/// The retained `familiar.identity_bundle.v1` for one revision.
fn identity_bundle(
    root_id: &str,
    revision_id: &str,
    position: u64,
    recorded_at: &str,
    declarations: &Declarations,
) -> Value {
    let components: Vec<Value> = declarations
        .components()
        .into_iter()
        .map(|(component_id, content)| {
            json!({
                "componentId": component_id,
                "mediaType": "application/json",
                "content": content,
                "digest": { "algorithm": "sha-256", "value": familiar_contract::digest_object(content) },
                "redactionState": "retained",
            })
        })
        .collect();
    let mut bundle = json!({
        "profile": "familiar.identity_bundle.v1",
        "canonicalization": "jcs-rfc8785",
        "familiarRootId": root_id,
        "identityRevisionId": revision_id,
        "lineagePosition": position,
        "recordedAt": recorded_at,
        "components": components,
        "retention": {
            "classification": "restricted-historical-identity",
            "verifierAccess": "authorized",
            "recordedAt": recorded_at,
            "tombstoneState": "live",
            "replicaPurgeState": "not_requested",
            "redactionState": "none",
        },
    });
    let digest = familiar_contract::bundle_digest(&bundle);
    bundle["bundleDigest"] = json!({ "algorithm": "sha-256", "value": digest });
    bundle
}

/// One stored revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisionRecord {
    pub revision_id: String,
    pub root_id: String,
    pub lineage_position: u64,
    pub relationship: String,
    pub predecessor_revision_id: Option<String>,
    pub transition_json: Option<String>,
    pub declaration_digest: String,
    pub bundle_digest: String,
    pub bundle_json: String,
    pub status: String,
    pub recorded_at: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub revoked_at: Option<String>,
    pub revocation_reason: Option<String>,
}

/// One root and its current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootRecord {
    pub root_id: String,
    pub roster_id: String,
    pub generation: u64,
    pub created_at: String,
    pub retired_at: Option<String>,
}

/// A root with its latest revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerHead {
    pub root: RootRecord,
    pub head: RevisionRecord,
}

const REVISION_COLUMNS: &str = "revision_id, root_id, lineage_position, relationship,
    predecessor_revision_id, transition_json, declaration_digest, bundle_digest, bundle_json,
    status, recorded_at, valid_from, valid_until, revoked_at, revocation_reason";

fn revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RevisionRecord> {
    let position: i64 = row.get(2)?;
    Ok(RevisionRecord {
        revision_id: row.get(0)?,
        root_id: row.get(1)?,
        lineage_position: u64::try_from(position).unwrap_or_default(),
        relationship: row.get(3)?,
        predecessor_revision_id: row.get(4)?,
        transition_json: row.get(5)?,
        declaration_digest: row.get(6)?,
        bundle_digest: row.get(7)?,
        bundle_json: row.get(8)?,
        status: row.get(9)?,
        recorded_at: row.get(10)?,
        valid_from: row.get(11)?,
        valid_until: row.get(12)?,
        revoked_at: row.get(13)?,
        revocation_reason: row.get(14)?,
    })
}

fn root_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RootRecord> {
    let generation: i64 = row.get(2)?;
    Ok(RootRecord {
        root_id: row.get(0)?,
        roster_id: row.get(1)?,
        generation: u64::try_from(generation).unwrap_or_default(),
        created_at: row.get(3)?,
        retired_at: row.get(4)?,
    })
}

fn root_by_id(conn: &Connection, root_id: &str) -> Result<Option<RootRecord>> {
    conn.query_row(
        "SELECT root_id, roster_id, generation, created_at, retired_at
         FROM familiar_ledger_roots WHERE root_id = ?1",
        [root_id],
        root_from_row,
    )
    .optional()
    .context("failed to read familiar ledger root")
}

fn live_root(conn: &Connection, roster_id: &str) -> Result<Option<RootRecord>> {
    conn.query_row(
        "SELECT root_id, roster_id, generation, created_at, retired_at
         FROM familiar_ledger_roots WHERE roster_id = ?1 AND retired_at IS NULL",
        [roster_id],
        root_from_row,
    )
    .optional()
    .context("failed to read familiar ledger root")
}

fn latest_revision(conn: &Connection, root_id: &str) -> Result<RevisionRecord> {
    conn.query_row(
        &format!(
            "SELECT {REVISION_COLUMNS} FROM familiar_ledger_revisions
             WHERE root_id = ?1 ORDER BY lineage_position DESC LIMIT 1"
        ),
        [root_id],
        revision_from_row,
    )
    .context("familiar ledger root has no revision")
}

fn revisions(conn: &Connection, root_id: &str) -> Result<Vec<RevisionRecord>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {REVISION_COLUMNS} FROM familiar_ledger_revisions
         WHERE root_id = ?1 ORDER BY lineage_position"
    ))?;
    let rows = statement.query_map([root_id], revision_from_row)?;
    rows.collect::<rusqlite::Result<_>>()
        .context("failed to read familiar ledger revisions")
}

/// One revision by id.
#[cfg_attr(not(test), allow(dead_code))] // Read by the binding issuer.
pub(crate) fn revision(conn: &Connection, revision_id: &str) -> Result<Option<RevisionRecord>> {
    conn.query_row(
        &format!("SELECT {REVISION_COLUMNS} FROM familiar_ledger_revisions WHERE revision_id = ?1"),
        [revision_id],
        revision_from_row,
    )
    .optional()
    .context("failed to read familiar ledger revision")
}

/// The live root answering to `roster_id`, and its latest revision.
#[cfg_attr(not(test), allow(dead_code))] // Read by the binding issuer (slice 3).
pub(crate) fn live_head(conn: &Connection, roster_id: &str) -> Result<Option<LedgerHead>> {
    ensure_familiar_ledger_schema(conn)?;
    let Some(root) = live_root(conn, roster_id)? else {
        return Ok(None);
    };
    let head = latest_revision(conn, &root.root_id)?;
    Ok(Some(LedgerHead { root, head }))
}

/// The Familiar Contract trusted-ledger observation of `root_id` at
/// `observed_at`: `{generation, headRevisionId, status, observedAt,
/// revokedAt?}`, read in one statement so its fields agree.
#[cfg_attr(not(test), allow(dead_code))] // Read by the binding issuer (slice 3).
pub(crate) fn observation(
    conn: &Connection,
    root_id: &str,
    observed_at: DateTime<Utc>,
) -> Result<Option<Value>> {
    ensure_familiar_ledger_schema(conn)?;
    conn.query_row(
        "SELECT root.generation, revision.revision_id, revision.status, revision.revoked_at
         FROM familiar_ledger_roots AS root
         JOIN familiar_ledger_revisions AS revision ON revision.root_id = root.root_id
         WHERE root.root_id = ?1
         ORDER BY revision.lineage_position DESC LIMIT 1",
        [root_id],
        |row| {
            let mut observation = json!({
                "generation": row.get::<_, i64>(0)?,
                "headRevisionId": row.get::<_, String>(1)?,
                "status": row.get::<_, String>(2)?,
                "observedAt": timestamp(observed_at),
            });
            if let Some(revoked_at) = row.get::<_, Option<String>>(3)? {
                observation["revokedAt"] = json!(revoked_at);
            }
            Ok(observation)
        },
    )
    .optional()
    .context("failed to observe the familiar ledger")
}

/// Whether the familiar's current declarations still match `head`. The issuer
/// refuses a binding when they do not, so the owner must adopt them first.
#[cfg_attr(not(test), allow(dead_code))] // Read by the binding issuer (slice 3).
pub(crate) fn head_is_current(coven_home: &Path, head: &LedgerHead) -> Result<bool> {
    let declarations = match read_declarations(coven_home, &head.root.roster_id)? {
        Ok(declarations) => declarations,
        Err(_) => return Ok(false),
    };
    same_declarations(&declarations, &head.head)
}

/// Whether `declarations` are exactly the components `revision` recorded.
fn same_declarations(declarations: &Declarations, revision: &RevisionRecord) -> Result<bool> {
    let bundle: Value = serde_json::from_str(&revision.bundle_json)
        .context("stored familiar identity bundle is not JSON")?;
    let recorded: Vec<(&str, &str)> = bundle["components"]
        .as_array()
        .context("stored familiar identity bundle has no components")?
        .iter()
        .map(|component| {
            (
                component["componentId"].as_str().unwrap_or_default(),
                component["digest"]["value"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    let current: Vec<(&str, String)> = declarations
        .components()
        .into_iter()
        .map(|(component_id, content)| (component_id, familiar_contract::digest_object(content)))
        .collect();
    Ok(recorded.len() == current.len()
        && recorded
            .iter()
            .zip(&current)
            .all(|((left_id, left), (right_id, right))| left_id == right_id && left == right))
}

/// Routes one ledger action. Every action needs an owner-local caller; the
/// API also refuses other transports before opening the store.
pub(crate) fn route(
    payload: &Value,
    conn: &Connection,
    coven_home: &Path,
    authority: CommandAuthority,
    now: DateTime<Utc>,
) -> (u16, ControlActionResponse) {
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if authority != CommandAuthority::OwnerLocal {
        return typed_rejection(
            &action,
            automation_error(
                ErrorCode::AuthorityRequired,
                "Familiar ledger commands require owner-local IPC.",
            ),
        );
    }
    let outcome = match action.as_str() {
        GET => get(conn, payload).map(|result| result.map(|value| (value, false))),
        REGISTER | ADOPT | RETIRE | REVOKE | RESTORE => {
            execute(conn, coven_home, &action, payload, now)
        }
        _ => Ok(Err(automation_error(
            ErrorCode::CapabilityUnsupported,
            format!("unknown familiar ledger action `{action}`"),
        ))),
    };
    match outcome {
        Ok(Ok((result, replayed))) => (
            200,
            ControlActionResponse {
                ok: true,
                accepted: true,
                action,
                status: ActionStatus::Completed,
                reason: replayed.then(|| "replayed previously adopted command".to_owned()),
                error: None,
                result: Some(result),
                event: None,
            },
        ),
        Ok(Err(error)) => typed_rejection(&action, error),
        Err(error) => typed_rejection(
            &action,
            automation_error(ErrorCode::Internal, format!("{error:#}")),
        ),
    }
}

type Outcome = std::result::Result<(Value, bool), ErrorEnvelope>;

fn invalid(message: impl Into<String>) -> ErrorEnvelope {
    automation_error(ErrorCode::ValidationFailed, message)
}

fn required_text<'a>(payload: &'a Value, key: &str) -> std::result::Result<&'a str, ErrorEnvelope> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(format!("`{key}` is required")))
}

/// Runs one mutating command inside an immediate transaction, replaying the
/// stored answer for an adoption key already used with the same request.
fn execute(
    conn: &Connection,
    coven_home: &Path,
    action: &str,
    payload: &Value,
    now: DateTime<Utc>,
) -> Result<Outcome> {
    let adoption_key = match required_text(payload, "adoptionKey") {
        Ok(key) => key.to_owned(),
        Err(error) => return Ok(Err(error)),
    };
    if crate::automations::contract::types::AdoptionKey::new(adoption_key.clone()).is_err() {
        return Ok(Err(invalid("`adoptionKey` is not a valid adoption key")));
    }
    let request_digest = familiar_contract::digest_object(payload);
    let now = chrono::DurationRound::duration_trunc(now, chrono::TimeDelta::milliseconds(1))
        .context("ledger time cannot be represented")?;
    ensure_familiar_ledger_schema(conn)?;
    if let Some(replayed) = replay(conn, &adoption_key, action, &request_digest)? {
        return Ok(replayed);
    }
    // The signing key is read before the transaction, because the key store
    // opens its own. So are the declarations of a familiar the request names;
    // restore learns its familiar from the root, inside the transaction.
    let familiar_id = payload
        .get("familiarId")
        .and_then(Value::as_str)
        .map(str::trim);
    let declarations = match (action, familiar_id) {
        (REGISTER | ADOPT, Some(familiar_id)) if !familiar_id.is_empty() => {
            match read_declarations(coven_home, familiar_id)? {
                Ok(declarations) => Some(declarations),
                Err(refusal) => return Ok(Err(refusal.envelope(familiar_id))),
            }
        }
        _ => None,
    };
    let key = if matches!(action, ADOPT | RESTORE) {
        Some(authority_keys::current_signing_key(
            conn,
            coven_home,
            AuthorityKeyRole::FamiliarBinding,
            now,
        )?)
    } else {
        None
    };
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .context("failed to begin familiar ledger transaction")?;
    // A concurrent command may have adopted this key since the first check.
    if let Some(replayed) = replay(&transaction, &adoption_key, action, &request_digest)? {
        return Ok(replayed);
    }
    let context = Command {
        conn: &transaction,
        coven_home,
        payload,
        now,
        adoption_key: &adoption_key,
        request_digest: &request_digest,
    };
    let result = match action {
        REGISTER => context.register(declarations),
        ADOPT => context.adopt(declarations, key.as_ref()),
        RETIRE => context.retire(),
        REVOKE => context.revoke(),
        RESTORE => context.restore(key.as_ref()),
        _ => unreachable!("execute is called only for mutating actions"),
    }?;
    match result {
        Ok(value) => {
            transaction
                .execute(
                    "INSERT INTO familiar_ledger_commands
                        (adoption_key, action, request_digest, response_json, recorded_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        adoption_key,
                        action,
                        request_digest,
                        value.to_string(),
                        timestamp(now)
                    ],
                )
                .context("failed to record the familiar ledger command")?;
            transaction
                .commit()
                .context("failed to commit familiar ledger command")?;
            Ok(Ok((value, false)))
        }
        // Refusals are not adopted: the key stays free for a corrected retry.
        Err(error) => Ok(Err(error)),
    }
}

fn replay(
    conn: &Connection,
    adoption_key: &str,
    action: &str,
    request_digest: &str,
) -> Result<Option<Outcome>> {
    let stored: Option<(String, String, String)> = conn
        .query_row(
            "SELECT action, request_digest, response_json FROM familiar_ledger_commands
             WHERE adoption_key = ?1",
            [adoption_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .context("failed to read familiar ledger command")?;
    Ok(stored.map(|(stored_action, stored_digest, response)| {
        if stored_action != action || stored_digest != request_digest {
            return Err(automation_error(
                ErrorCode::AdoptionReplayMismatch,
                format!("adoption key `{adoption_key}` was already used for a different request"),
            ));
        }
        serde_json::from_str(&response)
            .map(|value| (value, true))
            .map_err(|error| automation_error(ErrorCode::Internal, error.to_string()))
    }))
}

struct Command<'a> {
    conn: &'a Connection,
    coven_home: &'a Path,
    payload: &'a Value,
    now: DateTime<Utc>,
    adoption_key: &'a str,
    request_digest: &'a str,
}

impl Command<'_> {
    fn register(
        &self,
        declarations: Option<Declarations>,
    ) -> Result<std::result::Result<Value, ErrorEnvelope>> {
        let familiar_id = match required_text(self.payload, "familiarId") {
            Ok(id) => id,
            Err(error) => return Ok(Err(error)),
        };
        let declarations = declarations.context("register reads declarations first")?;
        if let Some(root) = live_root(self.conn, familiar_id)? {
            return Ok(Err(automation_error(
                ErrorCode::IllegalTransition,
                format!(
                    "familiar `{familiar_id}` is already registered as `{}`; adopt its declarations or retire it",
                    root.root_id
                ),
            )));
        }
        let root_suffix = random_hex()?;
        let root_id = format!("familiar:{root_suffix}");
        let now = timestamp(self.now);
        self.conn
            .execute(
                "INSERT INTO familiar_ledger_roots (root_id, roster_id, generation, created_at)
                 VALUES (?1, ?2, 1, ?3)",
                params![root_id, familiar_id, now],
            )
            .context("failed to record the familiar ledger root")?;
        let revision = self.insert_revision(&root_id, 0, "genesis", None, None, &declarations)?;
        self.record_event(&root_id, 1, "registered", &revision.revision_id)?;
        Ok(Ok(outcome("registered", &root_id, 1, &revision)))
    }

    fn adopt(
        &self,
        declarations: Option<Declarations>,
        key: Option<&RoleSigningKey>,
    ) -> Result<std::result::Result<Value, ErrorEnvelope>> {
        let familiar_id = match required_text(self.payload, "familiarId") {
            Ok(id) => id,
            Err(error) => return Ok(Err(error)),
        };
        let declarations = declarations.context("adopt reads declarations first")?;
        let Some(root) = live_root(self.conn, familiar_id)? else {
            return Ok(Err(automation_error(
                ErrorCode::NotFound,
                format!("familiar `{familiar_id}` is not registered; register it first"),
            )));
        };
        let head = latest_revision(self.conn, &root.root_id)?;
        if let Err(error) = self.expect_head(&head) {
            return Ok(Err(error));
        }
        if same_declarations(&declarations, &head)? {
            if head.status == "revoked" {
                return Ok(Err(automation_error(
                    ErrorCode::IllegalTransition,
                    format!(
                        "revision `{}` was revoked and its declarations are unchanged; change them before adopting",
                        head.revision_id
                    ),
                )));
            }
            return Ok(Ok(outcome(
                "unchanged",
                &root.root_id,
                root.generation,
                &head,
            )));
        }
        if head.status == "active" {
            self.close_revision(&head.revision_id, "superseded")?;
        }
        let revision = self.insert_revision(
            &root.root_id,
            head.lineage_position + 1,
            "same_familiar_revision",
            Some(&head),
            key,
            &declarations,
        )?;
        let generation = self.advance(&root)?;
        self.record_event(&root.root_id, generation, "adopted", &revision.revision_id)?;
        Ok(Ok(outcome("adopted", &root.root_id, generation, &revision)))
    }

    fn retire(&self) -> Result<std::result::Result<Value, ErrorEnvelope>> {
        let familiar_id = match required_text(self.payload, "familiarId") {
            Ok(id) => id,
            Err(error) => return Ok(Err(error)),
        };
        let Some(root) = live_root(self.conn, familiar_id)? else {
            return Ok(Err(automation_error(
                ErrorCode::NotFound,
                format!("familiar `{familiar_id}` has no live registration"),
            )));
        };
        let head = latest_revision(self.conn, &root.root_id)?;
        if let Err(error) = self.expect_head(&head) {
            return Ok(Err(error));
        }
        if head.status == "active" {
            self.close_revision(&head.revision_id, "retired")?;
        }
        self.conn
            .execute(
                "UPDATE familiar_ledger_roots SET retired_at = ?2 WHERE root_id = ?1",
                params![root.root_id, timestamp(self.now)],
            )
            .context("failed to retire the familiar ledger root")?;
        let generation = self.advance(&root)?;
        self.record_event(&root.root_id, generation, "retired", &head.revision_id)?;
        let head = latest_revision(self.conn, &root.root_id)?;
        Ok(Ok(outcome("retired", &root.root_id, generation, &head)))
    }

    fn revoke(&self) -> Result<std::result::Result<Value, ErrorEnvelope>> {
        let revision_id = match required_text(self.payload, "revisionId") {
            Ok(id) => id,
            Err(error) => return Ok(Err(error)),
        };
        let reason = match required_text(self.payload, "reason") {
            Ok(reason) if reason.len() <= MAX_REASON_BYTES => reason,
            Ok(_) => {
                return Ok(Err(invalid(format!(
                    "`reason` is longer than {MAX_REASON_BYTES} bytes"
                ))))
            }
            Err(error) => return Ok(Err(error)),
        };
        let revision = self
            .conn
            .query_row(
                &format!(
                    "SELECT {REVISION_COLUMNS} FROM familiar_ledger_revisions WHERE revision_id = ?1"
                ),
                [revision_id],
                revision_from_row,
            )
            .optional()
            .context("failed to read familiar ledger revision")?;
        let Some(revision) = revision else {
            return Ok(Err(automation_error(
                ErrorCode::NotFound,
                format!("familiar revision `{revision_id}` does not exist"),
            )));
        };
        if revision.status == "revoked" {
            return Ok(Err(automation_error(
                ErrorCode::IllegalTransition,
                format!("familiar revision `{revision_id}` is already revoked"),
            )));
        }
        self.conn
            .execute(
                "UPDATE familiar_ledger_revisions
                 SET status = 'revoked', revoked_at = ?2, revocation_reason = ?3,
                     valid_until = COALESCE(valid_until, ?2)
                 WHERE revision_id = ?1",
                params![revision_id, timestamp(self.now), reason],
            )
            .context("failed to revoke the familiar ledger revision")?;
        let root = root_by_id(self.conn, &revision.root_id)?
            .context("familiar ledger revision has no root")?;
        let generation = self.advance(&root)?;
        self.record_event(&root.root_id, generation, "revoked", revision_id)?;
        let revision = self
            .conn
            .query_row(
                &format!(
                    "SELECT {REVISION_COLUMNS} FROM familiar_ledger_revisions WHERE revision_id = ?1"
                ),
                [revision_id],
                revision_from_row,
            )
            .context("failed to reread the revoked revision")?;
        Ok(Ok(outcome("revoked", &root.root_id, generation, &revision)))
    }

    fn restore(
        &self,
        key: Option<&RoleSigningKey>,
    ) -> Result<std::result::Result<Value, ErrorEnvelope>> {
        let root_id = match required_text(self.payload, "rootId") {
            Ok(id) => id,
            Err(error) => return Ok(Err(error)),
        };
        let Some(root) = root_by_id(self.conn, root_id)? else {
            return Ok(Err(automation_error(
                ErrorCode::NotFound,
                format!("familiar root `{root_id}` does not exist"),
            )));
        };
        if root.retired_at.is_none() {
            return Ok(Err(automation_error(
                ErrorCode::IllegalTransition,
                format!("familiar root `{root_id}` is not retired"),
            )));
        }
        let head = latest_revision(self.conn, root_id)?;
        if let Err(error) = self.expect_head(&head) {
            return Ok(Err(error));
        }
        if head.status != "retired" {
            return Ok(Err(automation_error(
                ErrorCode::IllegalTransition,
                format!(
                    "familiar root `{root_id}` ended {}; only a retired familiar can be restored, so register a new root instead",
                    head.status
                ),
            )));
        }
        if let Some(live) = live_root(self.conn, &root.roster_id)? {
            return Ok(Err(automation_error(
                ErrorCode::IllegalTransition,
                format!(
                    "familiar `{}` is already registered as `{}`; retire it before restoring `{root_id}`",
                    root.roster_id, live.root_id
                ),
            )));
        }
        let declarations = match read_declarations(self.coven_home, &root.roster_id)? {
            Ok(declarations) => declarations,
            Err(refusal) => return Ok(Err(refusal.envelope(&root.roster_id))),
        };
        self.conn
            .execute(
                "UPDATE familiar_ledger_roots SET retired_at = NULL WHERE root_id = ?1",
                [root_id],
            )
            .context("failed to restore the familiar ledger root")?;
        let revision = self.insert_revision(
            root_id,
            head.lineage_position + 1,
            "restoration",
            Some(&head),
            key,
            &declarations,
        )?;
        let generation = self.advance(&root)?;
        self.record_event(root_id, generation, "restored", &revision.revision_id)?;
        Ok(Ok(outcome("restored", root_id, generation, &revision)))
    }

    /// Commands that change a root's head name the head they expect, so two
    /// owners acting at once cannot overwrite each other.
    fn expect_head(&self, head: &RevisionRecord) -> std::result::Result<(), ErrorEnvelope> {
        let expected = required_text(self.payload, "expectedRevisionId")?;
        if expected != head.revision_id {
            return Err(automation_error(
                ErrorCode::RevisionConflict,
                format!(
                    "the familiar's head revision is `{}`, not `{expected}`",
                    head.revision_id
                ),
            ));
        }
        Ok(())
    }

    fn close_revision(&self, revision_id: &str, status: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE familiar_ledger_revisions SET status = ?2, valid_until = ?3
                 WHERE revision_id = ?1",
                params![revision_id, status, timestamp(self.now)],
            )
            .context("failed to close the familiar ledger revision")?;
        Ok(())
    }

    fn insert_revision(
        &self,
        root_id: &str,
        position: u64,
        relationship: &str,
        predecessor: Option<&RevisionRecord>,
        key: Option<&RoleSigningKey>,
        declarations: &Declarations,
    ) -> Result<RevisionRecord> {
        let root_suffix = root_id.strip_prefix("familiar:").unwrap_or(root_id);
        let revision_id = format!("familiar-revision:{root_suffix}:{position}");
        let now = timestamp(self.now);
        let bundle = identity_bundle(root_id, &revision_id, position, &now, declarations);
        let bundle_digest = bundle["bundleDigest"]["value"]
            .as_str()
            .context("identity bundle has no digest")?
            .to_owned();
        let declaration_digest = declarations.declaration_digest();
        let transition = match predecessor {
            None => None,
            Some(predecessor) => Some(signed_transition(
                key.context("a transition needs the familiar-binding key")?,
                relationship,
                &predecessor.bundle_digest,
                root_id,
                &revision_id,
                &bundle_digest,
                &declaration_digest,
            )?),
        };
        let record = RevisionRecord {
            revision_id,
            root_id: root_id.to_owned(),
            lineage_position: position,
            relationship: relationship.to_owned(),
            predecessor_revision_id: predecessor.map(|p| p.revision_id.clone()),
            transition_json: transition.map(|value| value.to_string()),
            declaration_digest,
            bundle_digest,
            bundle_json: bundle.to_string(),
            status: "active".to_owned(),
            recorded_at: now.clone(),
            valid_from: now,
            valid_until: None,
            revoked_at: None,
            revocation_reason: None,
        };
        self.conn
            .execute(
                &format!(
                    "INSERT INTO familiar_ledger_revisions ({REVISION_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, NULL, NULL)"
                ),
                params![
                    record.revision_id,
                    record.root_id,
                    i64::try_from(position).context("lineage position exceeds SQLite range")?,
                    record.relationship,
                    record.predecessor_revision_id,
                    record.transition_json,
                    record.declaration_digest,
                    record.bundle_digest,
                    record.bundle_json,
                    record.status,
                    record.recorded_at,
                    record.valid_from,
                ],
            )
            .context("failed to record the familiar ledger revision")?;
        Ok(record)
    }

    fn advance(&self, root: &RootRecord) -> Result<u64> {
        let generation = root.generation + 1;
        self.conn
            .execute(
                "UPDATE familiar_ledger_roots SET generation = ?2 WHERE root_id = ?1",
                params![
                    root.root_id,
                    i64::try_from(generation).context("generation exceeds SQLite range")?
                ],
            )
            .context("failed to advance the familiar ledger generation")?;
        Ok(generation)
    }

    fn record_event(
        &self,
        root_id: &str,
        generation: u64,
        event: &str,
        revision_id: &str,
    ) -> Result<()> {
        let now = timestamp(self.now);
        let principal_id = owner_grants::owner_principal_id(self.conn, &now)?;
        self.conn
            .execute(
                "INSERT INTO familiar_ledger_events
                    (root_id, generation, event, revision_id, adoption_key, request_digest,
                     principal_id, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    root_id,
                    i64::try_from(generation).context("generation exceeds SQLite range")?,
                    event,
                    revision_id,
                    self.adoption_key,
                    self.request_digest,
                    principal_id,
                    now,
                ],
            )
            .context("failed to record the familiar ledger event")?;
        Ok(())
    }
}

/// The Familiar Contract lineage transition from a predecessor's bundle to
/// this revision, signed with the `familiar-binding` key over the raw bytes
/// of its digest.
fn signed_transition(
    key: &RoleSigningKey,
    relationship: &str,
    predecessor_bundle_digest: &str,
    root_id: &str,
    revision_id: &str,
    bundle_digest: &str,
    declaration_digest: &str,
) -> Result<Value> {
    let digest = familiar_contract::transition_digest(&familiar_contract::TransitionPreimage {
        relationship,
        predecessor_bundle_digest,
        successor_familiar_root_id: root_id,
        successor_identity_revision_id: revision_id,
        successor_bundle_digest: bundle_digest,
        successor_declaration_digest: declaration_digest,
    });
    Ok(json!({
        "relationship": relationship,
        "predecessorBundleDigest": predecessor_bundle_digest,
        "successorFamiliarRootId": root_id,
        "successorIdentityRevisionId": revision_id,
        "successorBundleDigest": bundle_digest,
        "successorDeclarationDigest": declaration_digest,
        "authentication": contract_authentication(key, &digest)?,
    }))
}

/// A Familiar Contract `authentication` member: the key's id, its SPKI public
/// key and the signature over the digest's raw bytes, both base64.
pub(crate) fn contract_authentication(key: &RoleSigningKey, digest_hex: &str) -> Result<Value> {
    let digest: [u8; 32] = decode_hex(digest_hex)
        .and_then(|bytes| bytes.try_into().ok())
        .context("a signed digest is 32 bytes of lowercase hex")?;
    let signature = decode_hex(&key.sign_digest(&digest)).context("signature is hex")?;
    let public_key = decode_hex(&key.record().public_key_der_hex).context("public key is hex")?;
    let base64 = base64::engine::general_purpose::STANDARD;
    Ok(json!({
        "method": "ed25519",
        "signerId": key.record().key_id,
        "publicKey": base64.encode(public_key),
        "signature": base64.encode(signature),
    }))
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

fn outcome(event: &str, root_id: &str, generation: u64, revision: &RevisionRecord) -> Value {
    json!({
        "outcome": event,
        "rootId": root_id,
        "generation": generation,
        "revision": revision_summary(revision),
    })
}

fn revision_summary(revision: &RevisionRecord) -> Value {
    json!({
        "revisionId": revision.revision_id,
        "lineagePosition": revision.lineage_position,
        "relationship": revision.relationship,
        "predecessorRevisionId": revision.predecessor_revision_id,
        "status": revision.status,
        "declarationDigest": revision.declaration_digest,
        "bundleDigest": revision.bundle_digest,
        "recordedAt": revision.recorded_at,
        "validFrom": revision.valid_from,
        "validUntil": revision.valid_until,
        "revokedAt": revision.revoked_at,
        "revocationReason": revision.revocation_reason,
    })
}

/// `get.v1`: every root registered under a roster id (`familiarId`), or one
/// root (`rootId`), with its revisions. Retired roots stay readable after the
/// familiar leaves the roster. Bundle contents are not returned.
fn get(conn: &Connection, payload: &Value) -> Result<std::result::Result<Value, ErrorEnvelope>> {
    ensure_familiar_ledger_schema(conn)?;
    let roots: Vec<RootRecord> = match (
        payload.get("familiarId").and_then(Value::as_str),
        payload.get("rootId").and_then(Value::as_str),
    ) {
        (Some(familiar_id), None) => {
            let mut statement = conn.prepare(
                "SELECT root_id, roster_id, generation, created_at, retired_at
                 FROM familiar_ledger_roots WHERE roster_id = ?1 ORDER BY created_at, root_id",
            )?;
            let rows = statement.query_map([familiar_id], root_from_row)?;
            rows.collect::<rusqlite::Result<_>>()?
        }
        (None, Some(root_id)) => root_by_id(conn, root_id)?.into_iter().collect(),
        _ => return Ok(Err(invalid("give exactly one of `familiarId` or `rootId`"))),
    };
    let roots = roots
        .into_iter()
        .map(|root| {
            let revisions = revisions(conn, &root.root_id)?;
            Ok(json!({
                "rootId": root.root_id,
                "familiarId": root.roster_id,
                "generation": root.generation,
                "createdAt": root.created_at,
                "retiredAt": root.retired_at,
                "revisions": revisions.iter().map(revision_summary).collect::<Vec<_>>(),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Ok(json!({ "roots": roots })))
}

/// The millisecond UTC form both the Familiar Contract and Coven's authority
/// timestamps accept.
fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn random_hex() -> Result<String> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0_u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to draw a random familiar root"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;

    const WARD: &str = "principal_key_fingerprint = \"SHA256:abc\"\nprotected_surface = [\"SOUL.md\"]\n\n[[surface]]\npath = \"SOUL.md\"\ntier = 0\n";

    pub(crate) struct Home {
        pub(crate) dir: tempfile::TempDir,
        pub(crate) conn: Connection,
    }

    impl Home {
        pub(crate) fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = crate::api::store_path(dir.path());
            crate::store::initialize_store(&path).unwrap();
            let conn = crate::store::open_store(&path).unwrap();
            let home = Self { dir, conn };
            home.roster(&[("sage", "Sage", "")]);
            home.write(
                "sage",
                "IDENTITY.md",
                "# IDENTITY.md - Sage\n- **Name:** Sage\n",
            );
            home.write("sage", "SOUL.md", "# SOUL\nResearch familiar.\n");
            home.write("sage", "ward.toml", WARD);
            home
        }

        pub(crate) fn path(&self) -> &Path {
            self.dir.path()
        }

        /// Writes familiars.toml with `(id, display name, extra TOML)` entries.
        pub(crate) fn roster(&self, entries: &[(&str, &str, &str)]) {
            let text: String = entries
                .iter()
                .map(|(id, display, extra)| {
                    format!("[[familiar]]\nid = \"{id}\"\ndisplay_name = \"{display}\"\nrole = \"Research\"\ndescription = \"Finds things.\"\npronouns = \"she/her\"\n{extra}\n")
                })
                .collect();
            std::fs::write(self.path().join("familiars.toml"), text).unwrap();
        }

        pub(crate) fn write(&self, familiar: &str, file: &str, text: &str) {
            let dir = self.path().join("familiars").join(familiar);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), text).unwrap();
        }

        pub(crate) fn remove(&self, familiar: &str, file: &str) {
            std::fs::remove_file(self.path().join("familiars").join(familiar).join(file)).unwrap();
        }

        pub(crate) fn act_as(
            &self,
            authority: CommandAuthority,
            request: Value,
        ) -> (u16, ControlActionResponse) {
            route(&request, &self.conn, self.path(), authority, at(0))
        }

        pub(crate) fn act(&self, request: Value) -> (u16, ControlActionResponse) {
            self.act_as(CommandAuthority::OwnerLocal, request)
        }

        pub(crate) fn ok(&self, request: Value) -> Value {
            let (status, response) = self.act(request);
            assert_eq!(status, 200, "{response:?}");
            response.result.unwrap()
        }

        pub(crate) fn refused(&self, request: Value) -> (u16, String) {
            let (status, response) = self.act(request);
            assert_ne!(status, 200, "{response:?}");
            (
                status,
                response.error.unwrap()["code"].as_str().unwrap().to_owned(),
            )
        }

        pub(crate) fn register(&self, key: &str) -> Value {
            self.ok(json!({"action": REGISTER, "adoptionKey": key, "familiarId": "sage"}))
        }

        pub(crate) fn adopt(&self, key: &str, expected: &str) -> (u16, ControlActionResponse) {
            self.act(json!({
                "action": ADOPT, "adoptionKey": key, "familiarId": "sage",
                "expectedRevisionId": expected,
            }))
        }

        pub(crate) fn head(&self) -> LedgerHead {
            live_head(&self.conn, "sage").unwrap().expect("live head")
        }

        pub(crate) fn count(&self, table: &str) -> i64 {
            self.conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        }
    }

    pub(crate) fn at(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 3, 21, 0, 0).unwrap()
            + chrono::TimeDelta::seconds(seconds)
            + chrono::TimeDelta::microseconds(400)
    }

    pub(crate) fn revision_id(result: &Value) -> String {
        result["revision"]["revisionId"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Checks a stored transition the way the contract verifier does: the
    /// fields name the two revisions, and the signature verifies over the raw
    /// bytes of the transition digest under the key it carries.
    fn assert_transition(revision: &RevisionRecord, predecessor: &RevisionRecord) {
        let transition: Value =
            serde_json::from_str(revision.transition_json.as_deref().unwrap()).unwrap();
        assert_eq!(transition["relationship"], revision.relationship.as_str());
        assert_eq!(
            transition["predecessorBundleDigest"],
            predecessor.bundle_digest.as_str()
        );
        assert_eq!(
            transition["successorFamiliarRootId"],
            revision.root_id.as_str()
        );
        assert_eq!(
            transition["successorIdentityRevisionId"],
            revision.revision_id.as_str()
        );
        assert_eq!(
            transition["successorBundleDigest"],
            revision.bundle_digest.as_str()
        );
        assert_eq!(
            transition["successorDeclarationDigest"],
            revision.declaration_digest.as_str()
        );
        let authentication = &transition["authentication"];
        assert_eq!(authentication["method"], "ed25519");
        assert!(authentication["signerId"]
            .as_str()
            .unwrap()
            .starts_with("coven-local:familiar-binding:"));
        let public_key =
            familiar_contract::spki_public_key(authentication["publicKey"].as_str().unwrap())
                .expect("an Ed25519 SPKI key");
        let signature = base64::engine::general_purpose::STANDARD
            .decode(authentication["signature"].as_str().unwrap())
            .unwrap();
        let digest = familiar_contract::transition_digest(&familiar_contract::TransitionPreimage {
            relationship: &revision.relationship,
            predecessor_bundle_digest: &predecessor.bundle_digest,
            successor_familiar_root_id: &revision.root_id,
            successor_identity_revision_id: &revision.revision_id,
            successor_bundle_digest: &revision.bundle_digest,
            successor_declaration_digest: &revision.declaration_digest,
        });
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
            .verify(&decode_hex(&digest).unwrap(), &signature)
            .expect("the transition signature verifies");
    }

    #[test]
    fn register_records_a_genesis_revision_and_its_retained_bundle() {
        let home = Home::new();
        let result = home.register("adopt:ledger:register");
        assert_eq!(result["outcome"], "registered");
        assert_eq!(result["generation"], 1);
        let root_id = result["rootId"].as_str().unwrap();
        let suffix = root_id.strip_prefix("familiar:").expect("an opaque root");
        assert!(suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            revision_id(&result),
            format!("familiar-revision:{suffix}:0")
        );

        let head = home.head();
        assert_eq!(
            (
                head.head.relationship.as_str(),
                head.head.status.as_str(),
                head.head.lineage_position
            ),
            ("genesis", "active", 0)
        );
        assert!(head.head.transition_json.is_none());
        assert_eq!(head.head.recorded_at, "2026-10-03T21:00:00.000Z");

        let bundle: Value = serde_json::from_str(&head.head.bundle_json).unwrap();
        assert_eq!(
            bundle["bundleDigest"]["value"],
            head.head.bundle_digest.as_str()
        );
        assert_eq!(
            familiar_contract::bundle_digest(&bundle),
            head.head.bundle_digest
        );
        let components = bundle["components"].as_array().unwrap();
        let ids: Vec<&str> = components
            .iter()
            .map(|c| c["componentId"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "identity-declaration",
                "soul-declaration",
                "ward-declaration"
            ]
        );
        for component in components {
            assert_eq!(
                component["digest"]["value"],
                familiar_contract::digest_object(&component["content"]).as_str()
            );
        }
        let identity = &components[0]["content"];
        assert_eq!(
            head.head.declaration_digest,
            familiar_contract::digest_object(identity)
        );
        assert_eq!(
            identity["roster"],
            json!({"id": "sage", "displayName": "Sage", "role": "Research", "pronouns": "she/her"})
        );
        assert_eq!(identity["text"], "# IDENTITY.md - Sage\n- **Name:** Sage\n");
        assert_eq!(
            components[1]["content"]["text"],
            "# SOUL\nResearch familiar.\n"
        );
        assert_eq!(
            components[2]["content"],
            json!({"mediaType": "application/toml", "text": WARD})
        );
        // No local path reaches the bundle.
        assert!(!head
            .head
            .bundle_json
            .contains(home.path().to_str().unwrap()));

        assert_eq!(
            observation(&home.conn, root_id, at(5)).unwrap().unwrap(),
            json!({
                "generation": 1, "headRevisionId": revision_id(&result), "status": "active",
                "observedAt": "2026-10-03T21:00:05.000Z",
            })
        );
        assert!(head_is_current(home.path(), &head).unwrap());

        // A familiar without ward.toml records two components.
        home.remove("sage", "ward.toml");
        let declarations = read_declarations(home.path(), "sage").unwrap().unwrap();
        assert_eq!(declarations.components().len(), 2);
        assert!(!head_is_current(home.path(), &head).unwrap());
    }

    #[test]
    fn adopt_supersedes_the_head_with_a_signed_transition() {
        let home = Home::new();
        let genesis = revision_id(&home.register("adopt:ledger:register"));

        // Unchanged declarations record nothing.
        let (status, response) = home.adopt("adopt:ledger:same", &genesis);
        assert_eq!(status, 200, "{response:?}");
        assert_eq!(response.result.unwrap()["outcome"], "unchanged");
        assert_eq!(home.count("familiar_ledger_revisions"), 1);

        // Display-only roster fields are not identity.
        home.roster(&[("sage", "Sage", "emoji = \"🦉\"\nicon = \"ph:owl\"")]);
        assert!(head_is_current(home.path(), &home.head()).unwrap());

        // A changed soul is a new revision, but only once adopted.
        home.write("sage", "SOUL.md", "# SOUL\nResearch and teach.\n");
        assert!(!head_is_current(home.path(), &home.head()).unwrap());
        assert_eq!(
            home.adopt("adopt:ledger:stale", "familiar-revision:other:0")
                .0,
            409,
            "a stale expected head is a revision conflict"
        );
        let (status, response) = home.adopt("adopt:ledger:soul", &genesis);
        assert_eq!(status, 200, "{response:?}");
        let adopted = response.result.unwrap();
        assert_eq!(
            (adopted["outcome"].as_str(), adopted["generation"].as_u64()),
            (Some("adopted"), Some(2))
        );
        let head = home.head();
        assert_eq!(
            (
                head.head.lineage_position,
                head.head.relationship.as_str(),
                head.head.status.as_str()
            ),
            (1, "same_familiar_revision", "active")
        );
        assert_eq!(
            head.head.predecessor_revision_id.as_deref(),
            Some(genesis.as_str())
        );
        let revisions = revisions(&home.conn, &head.root.root_id).unwrap();
        assert_eq!(revisions[0].status, "superseded");
        assert_eq!(
            revisions[0].valid_until.as_deref(),
            Some("2026-10-03T21:00:00.000Z")
        );
        assert_transition(&revisions[1], &revisions[0]);
        assert!(head_is_current(home.path(), &head).unwrap());

        // Renaming the familiar in the roster changes its identity declaration.
        home.roster(&[("sage", "Sage the Elder", "")]);
        assert!(!head_is_current(home.path(), &home.head()).unwrap());
        let (status, response) = home.adopt("adopt:ledger:rename", &head.head.revision_id);
        assert_eq!(status, 200, "{response:?}");
        let renamed = home.head();
        assert_ne!(
            renamed.head.declaration_digest,
            head.head.declaration_digest
        );
        assert_eq!(renamed.root.generation, 3);
    }

    #[test]
    fn a_retried_command_replays_and_a_reused_key_is_refused() {
        let home = Home::new();
        let request =
            json!({"action": REGISTER, "adoptionKey": "adopt:ledger:once", "familiarId": "sage"});
        let first = home.ok(request.clone());
        let (status, replayed) = home.act(request);
        assert_eq!(status, 200);
        assert_eq!(replayed.result.unwrap(), first);
        assert_eq!(
            replayed.reason.as_deref(),
            Some("replayed previously adopted command")
        );
        assert_eq!(home.count("familiar_ledger_roots"), 1);
        assert_eq!(
            home.refused(json!({"action": RETIRE, "adoptionKey": "adopt:ledger:once", "familiarId": "sage", "expectedRevisionId": revision_id(&first)})),
            (409, "ADOPTION_REPLAY_MISMATCH".to_owned())
        );
        assert_eq!(
            home.refused(json!({"action": REGISTER, "adoptionKey": "bad", "familiarId": "sage"})),
            (400, "VALIDATION_FAILED".to_owned())
        );
    }

    #[test]
    fn declarations_must_be_complete_unambiguous_and_owner_supplied() {
        let home = Home::new();
        let register = |key: &str, familiar: &str| json!({"action": REGISTER, "adoptionKey": key, "familiarId": familiar});
        assert_eq!(
            home.refused(register("adopt:ledger:nobody", "nova")),
            (404, "NOT_FOUND".to_owned())
        );
        home.remove("sage", "SOUL.md");
        assert_eq!(
            home.refused(register("adopt:ledger:nosoul", "sage")),
            (400, "VALIDATION_FAILED".to_owned())
        );
        home.write("sage", "SOUL.md", "# SOUL\n");
        home.write("sage", "ward.toml", "this is not = = toml");
        assert_eq!(
            home.refused(register("adopt:ledger:badward", "sage")),
            (400, "VALIDATION_FAILED".to_owned())
        );
        home.write("sage", "ward.toml", WARD);
        home.write("sage", "IDENTITY.md", "");
        std::fs::write(home.path().join("familiars/sage/IDENTITY.md"), [0xff, 0xfe]).unwrap();
        assert_eq!(
            home.refused(register("adopt:ledger:binary", "sage")),
            (400, "VALIDATION_FAILED".to_owned())
        );
        home.write("sage", "IDENTITY.md", "# IDENTITY\n");
        home.roster(&[("sage", "Sage", ""), ("sage", "Other Sage", "")]);
        assert_eq!(
            home.refused(register("adopt:ledger:twice", "sage")),
            (400, "VALIDATION_FAILED".to_owned())
        );
        assert_eq!(
            home.count("familiar_ledger_commands"),
            0,
            "refusals adopt nothing"
        );

        home.roster(&[("sage", "Sage", "")]);
        let (status, response) = home.act_as(
            CommandAuthority::Unauthenticated,
            register("adopt:ledger:fixed", "sage"),
        );
        assert_eq!(status, 403, "{response:?}");
        let (status, _) = home.act_as(
            CommandAuthority::Unauthenticated,
            json!({"action": GET, "familiarId": "sage"}),
        );
        assert_eq!(status, 403, "reads need the owner too");
        // The refused keys are still free once the declarations are fixed.
        home.register("adopt:ledger:fixed");
        assert_eq!(
            home.refused(register("adopt:ledger:again", "sage")),
            (422, "ILLEGAL_TRANSITION".to_owned())
        );
        assert_eq!(
            home.refused(json!({"action": "coven.familiars.ledger.erase.v1"})),
            (422, "CAPABILITY_UNSUPPORTED".to_owned())
        );
    }

    #[test]
    fn retire_restore_and_revoke_move_status_forward() {
        let home = Home::new();
        let genesis = revision_id(&home.register("adopt:ledger:register"));
        let root_id = home.head().root.root_id;
        home.write("sage", "SOUL.md", "# SOUL\nSecond.\n");
        let second = revision_id(
            &home
                .adopt("adopt:ledger:second", &genesis)
                .1
                .result
                .unwrap(),
        );

        let retired = home.ok(json!({
            "action": RETIRE, "adoptionKey": "adopt:ledger:retire", "familiarId": "sage",
            "expectedRevisionId": second,
        }));
        assert_eq!(
            (retired["outcome"].as_str(), retired["generation"].as_u64()),
            (Some("retired"), Some(3))
        );
        assert!(live_head(&home.conn, "sage").unwrap().is_none());
        assert_eq!(
            observation(&home.conn, &root_id, at(1)).unwrap().unwrap()["status"],
            "retired"
        );

        // The roster id is free again: registering it mints a new root.
        let replacement = home.register("adopt:ledger:replacement");
        assert_ne!(replacement["rootId"].as_str(), Some(root_id.as_str()));
        let restore = json!({
            "action": RESTORE, "adoptionKey": "adopt:ledger:restore", "rootId": root_id,
            "expectedRevisionId": second,
        });
        assert_eq!(
            home.refused(restore.clone()),
            (422, "ILLEGAL_TRANSITION".to_owned())
        );
        home.ok(json!({
            "action": RETIRE, "adoptionKey": "adopt:ledger:retire-replacement", "familiarId": "sage",
            "expectedRevisionId": revision_id(&replacement),
        }));
        let restored = home.ok(restore);
        assert_eq!(
            (
                restored["outcome"].as_str(),
                restored["generation"].as_u64()
            ),
            (Some("restored"), Some(4))
        );
        let history = revisions(&home.conn, &root_id).unwrap();
        assert_eq!(
            history
                .iter()
                .map(|r| r.status.as_str())
                .collect::<Vec<_>>(),
            ["superseded", "retired", "active"]
        );
        assert_eq!(history[2].relationship, "restoration");
        assert_transition(&history[2], &history[1]);

        // Revoking the head leaves the root with no active revision.
        let revoke = |key: &str, revision: &str| json!({"action": REVOKE, "adoptionKey": key, "revisionId": revision, "reason": "key compromise"});
        home.ok(revoke("adopt:ledger:revoke-old", &genesis));
        assert_eq!(
            home.refused(revoke("adopt:ledger:revoke-twice", &genesis)),
            (422, "ILLEGAL_TRANSITION".to_owned())
        );
        let head = revision_id(&restored);
        home.ok(revoke("adopt:ledger:revoke-head", &head));
        let observed = observation(&home.conn, &root_id, at(2)).unwrap().unwrap();
        assert_eq!(
            (observed["status"].as_str(), observed["generation"].as_u64()),
            (Some("revoked"), Some(6))
        );
        assert_eq!(observed["revokedAt"], "2026-10-03T21:00:00.000Z");
        // A revoked head cannot be re-adopted unchanged, only replaced.
        assert_eq!(
            home.adopt("adopt:ledger:readopt", &head).0,
            422,
            "unchanged declarations stay revoked"
        );
        home.write("sage", "SOUL.md", "# SOUL\nRotated.\n");
        let (status, response) = home.adopt("adopt:ledger:replace-revoked", &head);
        assert_eq!(status, 200, "{response:?}");
        assert_eq!(home.head().head.status, "active");

        // History stays readable by roster id and by root.
        let by_alias = home.ok(json!({"action": GET, "familiarId": "sage"}));
        assert_eq!(by_alias["roots"].as_array().unwrap().len(), 2);
        let by_root = home.ok(json!({"action": GET, "rootId": root_id}));
        assert_eq!(
            by_root["roots"][0]["revisions"].as_array().unwrap().len(),
            4
        );
        assert!(
            !by_root.to_string().contains("Rotated"),
            "no declaration text"
        );
    }

    #[test]
    fn a_root_retired_after_its_head_was_revoked_cannot_be_restored() {
        // The contract's restoration follows a retired predecessor, so a root
        // that ended revoked must be registered afresh instead.
        let home = Home::new();
        let genesis = revision_id(&home.register("adopt:ledger:register"));
        let root_id = home.head().root.root_id;
        home.ok(json!({
            "action": REVOKE, "adoptionKey": "adopt:ledger:revoke", "revisionId": genesis,
            "reason": "declaration compromised",
        }));
        let retired = home.ok(json!({
            "action": RETIRE, "adoptionKey": "adopt:ledger:retire", "familiarId": "sage",
            "expectedRevisionId": genesis,
        }));
        assert_eq!(
            retired["revision"]["status"], "revoked",
            "retiring keeps the revocation"
        );
        assert_eq!(
            home.refused(json!({
                "action": RESTORE, "adoptionKey": "adopt:ledger:restore", "rootId": root_id,
                "expectedRevisionId": genesis,
            })),
            (422, "ILLEGAL_TRANSITION".to_owned())
        );
        assert_eq!(home.count("familiar_ledger_revisions"), 1);
    }

    #[test]
    fn evidence_is_immutable_and_history_append_only() {
        let home = Home::new();
        let genesis = revision_id(&home.register("adopt:ledger:register"));
        let root_id = home.head().root.root_id;
        let refused = |sql: &str| home.conn.execute(sql, []).is_err();
        assert!(refused(
            "UPDATE familiar_ledger_revisions SET bundle_json = '{}'"
        ));
        assert!(refused(
            "UPDATE familiar_ledger_revisions SET declaration_digest = ''"
        ));
        assert!(refused("DELETE FROM familiar_ledger_revisions"));
        assert!(refused("DELETE FROM familiar_ledger_events"));
        assert!(refused(
            "UPDATE familiar_ledger_events SET event = 'adopted'"
        ));
        assert!(refused("DELETE FROM familiar_ledger_commands"));
        assert!(refused("UPDATE familiar_ledger_roots SET generation = 5"));
        assert!(refused(
            "UPDATE familiar_ledger_roots SET roster_id = 'nova'"
        ));
        home.write("sage", "SOUL.md", "# SOUL\nSecond.\n");
        home.adopt("adopt:ledger:second", &genesis);
        assert!(refused(&format!(
            "UPDATE familiar_ledger_revisions SET status = 'active' WHERE revision_id = '{genesis}'"
        )));
        assert!(
            refused(
                "INSERT INTO familiar_ledger_roots (root_id, roster_id, generation, created_at)
             VALUES ('familiar:dup', 'sage', 1, 'now')"
            ),
            "one live root per roster id"
        );
        assert_eq!(home.head().root.root_id, root_id);
    }

    #[test]
    fn the_api_routes_ledger_actions_for_the_owner_only() {
        use crate::request_authority::RequestAuthority;
        let home = Home::new();
        let post = |authority: RequestAuthority, body: Value| {
            crate::api::handle_request_with_runtime_and_authority(
                "POST",
                "/api/v1/actions",
                home.path(),
                None,
                Some(&body.to_string()),
                &crate::api::NoopSessionRuntime,
                authority,
            )
            .unwrap()
        };
        let register =
            json!({"action": REGISTER, "adoptionKey": "adopt:api:register", "familiarId": "sage"});
        // The transport guard refuses every ledger action before the store
        // opens, independently of the ledger's own owner check.
        for action in [REGISTER, ADOPT, RETIRE, REVOKE, RESTORE, GET] {
            let guarded = crate::control_plane::automation_transport_rejection(
                &json!({ "action": action }),
                RequestAuthority::Tcp,
            );
            assert_eq!(guarded.map(|(status, _)| status), Some(403), "{action}");
            assert!(crate::control_plane::automation_transport_rejection(
                &json!({ "action": action }),
                RequestAuthority::OwnerLocalIpc,
            )
            .is_none());
        }
        let refused = post(RequestAuthority::Tcp, register.clone());
        assert_eq!(refused.status, 403, "{}", refused.body);
        assert!(refused.body.contains("AUTHORITY_REQUIRED"));
        let read = post(
            RequestAuthority::Tcp,
            json!({"action": GET, "familiarId": "sage"}),
        );
        assert_eq!(read.status, 403, "{}", read.body);
        let registered = post(RequestAuthority::OwnerLocalIpc, register);
        assert_eq!(registered.status, 200, "{}", registered.body);
        assert!(registered.body.contains("\"outcome\":\"registered\""));
    }
}
