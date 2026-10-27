//! Threads automation-authority decisions (coven#857, slice 4).
//!
//! Decision 3 has the daemon evaluate the automation-authority profile in
//! process, through the pinned `coven_threads_core::automation_authority`, and
//! sign each decision with its `threads-decision` key. The profile
//! authenticates every authorization request by a `principal`-role key bound
//! to the principal it speaks for. The owner is the only principal
//! (Decision 1), so the daemon signs requests on the owner's behalf with its
//! `owner-principal` key.
//!
//! [`decide`] takes an unsigned request draft and a policy snapshot. It stamps
//! the owner principal and the daemon's clock, signs the request, evaluates it
//! with the pinned evaluator, and signs the decision. It then verifies the pair
//! as an independent verifier would, and only then records both. The profile
//! keyring is built from the daemon's key records, so a revoked key, or one
//! used outside its validity window, authenticates nothing. [`recorded`] reads
//! a decision back and verifies it again under the keyring as it stood when the
//! decision was made, and [`valid_at`] also holds it to the decision's own
//! validity window.
//!
//! Nothing in production calls this module yet. The trusted adapter (slice 6)
//! has two jobs left. It composes the inputs: the definition's declared action,
//! risk class, capabilities and scopes; the policy and protected-surface
//! manifest with their digests; the recurring grants; and the side-effect
//! class, which the profile does not carry. It also consumes the decision at
//! dispatch.

use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine as _;
use chrono::{DateTime, DurationRound, SecondsFormat, TimeDelta, Utc};
use coven_threads_core::automation_authority::{
    canonical_digest, evaluate_authorization, sign_artifact, verify_decision_bundle,
    ArtifactSigner, AuthorityError, Domain, ErrorCode, Keyring, SignatureVerifier,
};
use ring::signature::{UnparsedPublicKey, ED25519};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

use super::authority_keys::{self, AuthorityKeyRole, RoleSigningKey};
use super::contract::authority::{
    AuthorityCapabilities, AuthorityOutcome, AuthorityThreadsBinding, RiskClass,
};
use super::owner_grants;

pub(crate) const AUTOMATION_THREADS_DECISIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_threads_decisions (
        decision_id TEXT PRIMARY KEY NOT NULL,
        decision_digest TEXT NOT NULL UNIQUE CHECK (length(decision_digest) = 71),
        request_id TEXT NOT NULL UNIQUE,
        request_digest TEXT NOT NULL UNIQUE CHECK (length(request_digest) = 71),
        nonce TEXT NOT NULL UNIQUE,
        adoption_key TEXT NOT NULL UNIQUE,
        outcome TEXT NOT NULL
            CHECK (outcome IN ('permit', 'requires_approval', 'degrade_to_proposal', 'reject')),
        request_json TEXT NOT NULL,
        decision_json TEXT NOT NULL,
        policy_json TEXT NOT NULL,
        decided_at TEXT NOT NULL
    );
    CREATE TRIGGER IF NOT EXISTS automation_threads_decisions_immutable
    BEFORE UPDATE ON automation_threads_decisions
    BEGIN SELECT RAISE(ABORT, 'Threads decisions are immutable'); END;
    CREATE TRIGGER IF NOT EXISTS automation_threads_decisions_retained
    BEFORE DELETE ON automation_threads_decisions
    BEGIN SELECT RAISE(ABORT, 'Threads decisions are retained'); END;
";

/// A signed request and its signed decision, as recorded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThreadsDecision {
    pub request: Value,
    pub decision: Value,
    /// The policy snapshot the decision was evaluated under, with the daemon's
    /// clock as its `now`.
    pub policy: Value,
    /// `sha256:`-prefixed, under the profile's request domain.
    pub request_digest: String,
    /// `sha256:`-prefixed, under the profile's decision domain.
    pub decision_digest: String,
    pub outcome: String,
    pub decided_at: DateTime<Utc>,
}

/// Why no decision was made, or a recorded one is not trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecisionRefusal {
    /// The profile refused the request, the policy snapshot or, on reading
    /// back, the recorded pair, with its first error code.
    Invalid { code: &'static str, message: String },
    /// A request with this id, nonce or adoption key was already decided, so
    /// its decision id was already recorded.
    Replayed,
    /// No decision has this id.
    Unknown,
}

impl From<AuthorityError> for DecisionRefusal {
    fn from(error: AuthorityError) -> Self {
        Self::Invalid {
            code: error.code.as_str(),
            message: error.to_string(),
        }
    }
}

/// The parts of the execution binding a dispatchable decision fills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionBinding {
    pub outcome: AuthorityOutcome,
    pub threads: AuthorityThreadsBinding,
    pub capabilities: AuthorityCapabilities,
    pub risk_class: RiskClass,
}

impl ThreadsDecision {
    /// The execution binding's `threads` and `capabilities` members, its risk
    /// class and the authorization outcome. Only a `permit` or
    /// `requires_approval` decision can authorize dispatch, so the others have
    /// none.
    ///
    /// Digests are the profile's own: SHA-256 over the JCS text under the
    /// artifact's domain prefix, which is how Threads names them.
    pub(crate) fn binding(&self) -> Result<Option<DecisionBinding>> {
        let outcome = match self.outcome.as_str() {
            "permit" => AuthorityOutcome::Permit,
            "requires_approval" => AuthorityOutcome::RequiresApproval,
            _ => return Ok(None),
        };
        let versions = &self.request["versions"];
        let threads = serde_json::from_value(json!({
            "decisionId": self.decision["decision_id"],
            "decisionDigest": digest_value(&self.decision_digest)?,
            "protectedSurfaceManifestId": versions["manifest"],
            "protectedSurfaceManifestDigest": digest_value(
                versions["manifest_digest"].as_str().unwrap_or_default()
            )?,
        }))
        .context("a Threads decision does not fit the execution binding's threads member")?;
        let denied: Vec<Value> = self.decision["denied_capabilities"]
            .as_array()
            .context("a Threads decision lists its denied capabilities")?
            .iter()
            .map(|denial| {
                json!({ "capability": denial["capability"], "reasonCode": denial["reason_code"] })
            })
            .collect();
        let capabilities = serde_json::from_value(json!({
            "requested": self.request["requested_capabilities"],
            "granted": self.decision["granted_capabilities"],
            "denied": denied,
            "degraded": self.decision["degraded_capabilities"],
        }))
        .context("a Threads decision does not fit the execution binding's capabilities")?;
        let risk_class = serde_json::from_value(self.request["action"]["risk_class"].clone())
            .context("a Threads request carries a risk class")?;
        Ok(Some(DecisionBinding {
            outcome,
            threads,
            capabilities,
            risk_class,
        }))
    }
}

pub(crate) fn ensure_threads_decisions_schema(conn: &Connection) -> Result<()> {
    owner_grants::ensure_owner_grants_schema(conn)?;
    conn.execute_batch(AUTOMATION_THREADS_DECISIONS_SCHEMA_SQL)
        .context("failed to initialize Threads decisions schema")
}

/// Decides `draft` under `policy` at `now`. The owner key signs the request,
/// the pinned evaluator computes the decision, the decision key signs it, and
/// the pair is verified and recorded in one transaction.
pub(crate) fn decide(
    conn: &Connection,
    coven_home: &Path,
    draft: &Value,
    policy: &Value,
    now: DateTime<Utc>,
) -> Result<std::result::Result<ThreadsDecision, DecisionRefusal>> {
    let now = now
        .duration_trunc(TimeDelta::milliseconds(1))
        .context("decision time cannot be represented")?;
    let at = timestamp(now);
    anyhow::ensure!(
        draft.get("integrity").is_none(),
        "a Threads request draft must be unsigned"
    );
    ensure_threads_decisions_schema(conn)?;
    // The key store opens its own transactions, so the keys come first.
    let request_key = authority_keys::current_signing_key(
        conn,
        coven_home,
        AuthorityKeyRole::OwnerPrincipal,
        now,
    )?;
    let decision_key = authority_keys::current_signing_key(
        conn,
        coven_home,
        AuthorityKeyRole::ThreadsDecision,
        now,
    )?;
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .context("failed to begin Threads decision transaction")?;
    let owner = owner_grants::owner_principal_id(&transaction, &at)?;

    // The owner is the only principal, and the daemon's clock the only clock.
    let mut request = draft.clone();
    if let Some(principal) = request.get_mut("principal").and_then(Value::as_object_mut) {
        principal.insert("id".to_owned(), json!(owner));
    }
    let mut policy = policy.clone();
    if let Some(snapshot) = policy.as_object_mut() {
        snapshot.insert("now".to_owned(), json!(at));
    }

    let request = match sign_artifact(&request, Domain::Request, &RoleSigner(&request_key)) {
        Ok(signed) => signed,
        Err(error) => return Ok(Err(error.into())),
    };
    let keyring = keyring_at(&transaction, &owner, now)?;
    let unsigned = match evaluate_authorization(&request, &policy, &keyring, &Ed25519) {
        Ok(decision) => decision,
        Err(error) => return Ok(Err(error.into())),
    };
    if replayed(&transaction, &request, &unsigned)? {
        return Ok(Err(DecisionRefusal::Replayed));
    }
    let decision = sign_artifact(&unsigned, Domain::Decision, &RoleSigner(&decision_key))
        .context("the evaluated Threads decision could not be signed")?;
    let outcome = verify_decision_bundle(&request, &decision, &policy, &keyring, &Ed25519)
        .context("the signed Threads decision failed verification")?;
    let decided = ThreadsDecision {
        request_digest: prefixed_digest(&request, Domain::Request)?,
        decision_digest: prefixed_digest(&decision, Domain::Decision)?,
        request,
        decision,
        policy,
        outcome,
        decided_at: now,
    };

    transaction
        .execute(
            "INSERT INTO automation_threads_decisions
                (decision_id, decision_digest, request_id, request_digest, nonce, adoption_key,
                 outcome, request_json, decision_json, policy_json, decided_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                decided.decision["decision_id"].as_str(),
                decided.decision_digest,
                decided.request["request_id"].as_str(),
                decided.request_digest,
                decided.request["replay"]["nonce"].as_str(),
                decided.request["replay"]["adoption_key"].as_str(),
                decided.outcome,
                decided.request.to_string(),
                decided.decision.to_string(),
                decided.policy.to_string(),
                at,
            ],
        )
        .context("failed to record the Threads decision")?;
    transaction
        .commit()
        .context("failed to commit the Threads decision")?;
    Ok(Ok(decided))
}

/// The recorded decision `decision_id`, verified again under the keyring as
/// it stood when the decision was made. A key revoked since then
/// authenticates nothing, so the decision is refused.
pub(crate) fn recorded(
    conn: &Connection,
    decision_id: &str,
) -> Result<std::result::Result<ThreadsDecision, DecisionRefusal>> {
    ensure_threads_decisions_schema(conn)?;
    let row = conn
        .query_row(
            "SELECT request_json, decision_json, policy_json, decided_at
             FROM automation_threads_decisions WHERE decision_id = ?1",
            [decision_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .context("failed to read the Threads decision")?;
    let Some((request, decision, policy, decided_at)) = row else {
        return Ok(Err(DecisionRefusal::Unknown));
    };
    let request: Value = serde_json::from_str(&request).context("recorded request is JSON")?;
    let decision: Value = serde_json::from_str(&decision).context("recorded decision is JSON")?;
    let policy: Value = serde_json::from_str(&policy).context("recorded policy is JSON")?;
    let decided_at = DateTime::parse_from_rfc3339(&decided_at)
        .context("recorded decision time is RFC 3339")?
        .with_timezone(&Utc);
    let owner = owner_grants::owner_principal_id(conn, &timestamp(decided_at))?;
    let keyring = keyring_at(conn, &owner, decided_at)?;
    let outcome = match verify_decision_bundle(&request, &decision, &policy, &keyring, &Ed25519) {
        Ok(outcome) => outcome,
        Err(error) => return Ok(Err(error.into())),
    };
    Ok(Ok(ThreadsDecision {
        request_digest: prefixed_digest(&request, Domain::Request)?,
        decision_digest: prefixed_digest(&decision, Domain::Decision)?,
        request,
        decision,
        policy,
        outcome,
        decided_at,
    }))
}

/// [`recorded`], and also inside the decision's own `validity` window at
/// `now`. The window and its codes are the profile's: `verify_dispatch` holds
/// a decision to the same `not_before <= now < not_after` interval with
/// `decision_not_yet_valid` and `decision_expired`. This checks the window
/// only; consumption and the rest of dispatch verification stay with slice 6.
pub(crate) fn valid_at(
    conn: &Connection,
    decision_id: &str,
    now: DateTime<Utc>,
) -> Result<std::result::Result<ThreadsDecision, DecisionRefusal>> {
    let decided = match recorded(conn, decision_id)? {
        Ok(decided) => decided,
        refused => return Ok(refused),
    };
    let validity = &decided.decision["validity"];
    let bound = |name: &str| -> Result<DateTime<Utc>> {
        let text = validity[name]
            .as_str()
            .with_context(|| format!("a verified Threads decision carries validity.{name}"))?;
        Ok(DateTime::parse_from_rfc3339(text)
            .with_context(|| format!("validity.{name} is RFC 3339"))?
            .with_timezone(&Utc))
    };
    let window = if now < bound("not_before")? {
        Some((ErrorCode::DecisionNotYetValid, "has not started"))
    } else if now >= bound("not_after")? {
        Some((ErrorCode::DecisionExpired, "has ended"))
    } else {
        None
    };
    Ok(match window {
        Some((code, state)) => Err(DecisionRefusal::Invalid {
            code: code.as_str(),
            message: format!("the decision's validity interval {state}"),
        }),
        None => Ok(decided),
    })
}

/// The profile keyring as of `at`. The `threads-decision` keys become
/// `threads_authority` keys, and the `owner-principal` keys become `principal`
/// keys bound to the owner. A key is included only inside its validity window,
/// and never once revoked. No other role's key is included.
fn keyring_at(conn: &Connection, owner: &str, at: DateTime<Utc>) -> Result<Keyring> {
    let mut records = Vec::new();
    for record in authority_keys::key_records(conn)? {
        let mut entry = match record.role {
            AuthorityKeyRole::ThreadsDecision => json!({ "role": "threads_authority" }),
            AuthorityKeyRole::OwnerPrincipal => {
                json!({ "role": "principal", "principal_id": owner })
            }
            _ => continue,
        };
        let live = record.revoked_at.is_none()
            && record.valid_from <= at
            && record.valid_until.is_none_or(|until| at < until);
        if !live {
            continue;
        }
        entry["public_key_pem"] = json!(public_key_pem(&record.public_key_der_hex)?);
        records.push((record.key_id, entry));
    }
    Ok(Keyring::new(records))
}

/// Whether this request, or its decision, was already recorded. The decision
/// id is the profile's own (`decision:<request id>:<request digest prefix>`),
/// so a repeated id is a replayed request; a reused nonce or adoption key is
/// a replay under a new id.
fn replayed(conn: &Connection, request: &Value, decision: &Value) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM automation_threads_decisions
            WHERE decision_id = ?1 OR request_id = ?2 OR nonce = ?3 OR adoption_key = ?4
         )",
        params![
            decision["decision_id"].as_str(),
            request["request_id"].as_str(),
            request["replay"]["nonce"].as_str(),
            request["replay"]["adoption_key"].as_str(),
        ],
        |row| row.get(0),
    )
    .context("failed to check Threads request replay")
}

/// A role key as the profile's signer.
struct RoleSigner<'a>(&'a RoleSigningKey);

impl ArtifactSigner for RoleSigner<'_> {
    fn key_id(&self) -> &str {
        &self.0.record().key_id
    }

    fn sign_ed25519(&self, message: &[u8; 32]) -> [u8; 64] {
        self.0.sign(message)
    }
}

/// The profile's signature check, by `ring`.
struct Ed25519;

impl SignatureVerifier for Ed25519 {
    fn verify_ed25519(
        &self,
        public_key: &[u8; 32],
        message: &[u8; 32],
        signature: &[u8; 64],
    ) -> bool {
        UnparsedPublicKey::new(&ED25519, public_key)
            .verify(message, signature)
            .is_ok()
    }
}

fn public_key_pem(der_hex: &str) -> Result<String> {
    let der = (0..der_hex.len())
        .step_by(2)
        .map(|index| {
            der_hex
                .get(index..index + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect::<Option<Vec<u8>>>()
        .context("an authority key's public key is not hex")?;
    Ok(format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        base64::engine::general_purpose::STANDARD.encode(der)
    ))
}

fn prefixed_digest(value: &Value, domain: Domain) -> Result<String> {
    Ok(format!(
        "sha256:{}",
        canonical_digest(value, domain.as_str()).context("a signed artifact has a digest")?
    ))
}

fn digest_value(prefixed: &str) -> Result<Value> {
    let value = prefixed
        .strip_prefix("sha256:")
        .context("a profile digest is sha256-prefixed")?;
    Ok(json!({ "algorithm": "sha256", "canonicalization": "jcs-rfc8785", "value": value }))
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::authority_keys::{self, AuthorityKeyRole};
    use crate::automations::owner_grants;
    use chrono::TimeZone;
    use coven_threads_core::automation_authority::{sign_artifact, verify_decision_bundle, Domain};
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let conn = Connection::open(temp.path().join("store.sqlite")).unwrap();
        (temp, conn)
    }

    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, hour, minute, 0).unwrap()
    }

    fn digest(seed: char) -> String {
        format!("sha256:{}", seed.to_string().repeat(64))
    }

    /// The profile's R0 read vector, unsigned, with this test's own ids.
    fn draft(name: &str) -> Value {
        json!({
            "schema_version": "opencoven.automation-authorization-request/v1",
            "request_id": format!("request:{name}"),
            "principal": { "id": "principal:anyone", "authorization_proof_ref": format!("proof:{name}") },
            "replay": {
                "nonce": format!("nonce:{name}"),
                "adoption_key": format!("adopt:{name}"),
                "issued_at": "2026-10-04T08:55:00Z",
                "expires_at": "2026-10-04T10:00:00Z"
            },
            "familiar": { "id": "familiar:sage", "embodiment_digest": digest('1') },
            "automation": {
                "id": "automation:daily-report",
                "definition_revision": 7,
                "definition_digest": digest('2')
            },
            "execution": {
                "occurrence_id": "occ:2026-10-04",
                "run_id": "run:01",
                "attempt": 1,
                "fence_generation": 4
            },
            "action": {
                "type": "analysis.read",
                "digest": digest('3'),
                "risk_class": "R0",
                "proposal_safe": true
            },
            "requested_capabilities": ["analysis.read"],
            "scopes": [{
                "kind": "filesystem",
                "root": "workspace",
                "path": "inputs/report.json",
                "access": "read",
                "recursive": false
            }],
            "context": {
                "project_id": "project:coven",
                "workspace_id": "workspace:main",
                "runtime": {
                    "id": "runtime:node",
                    "descriptor_digest": digest('4'),
                    "capabilities": ["analysis.read"]
                }
            },
            "versions": {
                "profile": "1.0.0",
                "policy": "policy:2026-10-04",
                "policy_digest": digest('5'),
                "manifest": "manifest:1",
                "manifest_digest": digest('6')
            },
            "previous_approval_digest": null,
            "conditions": [],
            "data": { "sensitivity": "internal", "retention": "authority_evidence_90d" }
        })
    }

    /// The draft reshaped into one of the profile's risk ladders.
    fn draft_for(name: &str, risk: &str) -> Value {
        let (action, capability, scope, conditions) = match risk {
            "R0" => return draft(name),
            "R2" => (
                json!({ "type": "state.migrate", "digest": digest('3'), "risk_class": "R2", "proposal_safe": false }),
                "state.mutate",
                json!({ "kind": "filesystem", "root": "workspace", "path": "state/v2.sqlite", "access": "write", "recursive": false }),
                json!(["deterministic_validation", "rollback_plan"]),
            ),
            "R3" => (
                json!({ "type": "external.publish", "digest": digest('3'), "risk_class": "R3", "proposal_safe": true }),
                "network.publish",
                json!({ "kind": "network", "scheme": "https", "host": "api.example.invalid", "port": 443, "path_prefix": "/v1/releases", "methods": ["POST"] }),
                json!([]),
            ),
            "R4" => (
                json!({ "type": "identity.mutate", "digest": digest('3'), "risk_class": "R4", "proposal_safe": false }),
                "identity.mutate",
                json!({ "kind": "filesystem", "root": "workspace", "path": "identity/SOUL.md", "access": "write", "recursive": false }),
                json!([]),
            ),
            other => panic!("no ladder for {other}"),
        };
        let mut request = draft(name);
        request["action"] = action;
        request["requested_capabilities"] = json!([capability]);
        request["context"]["runtime"]["capabilities"] = json!([capability]);
        request["scopes"] = json!([scope]);
        request["conditions"] = conditions;
        request
    }

    /// A policy whose one recurring grant covers [`draft`] for `owner`. Its
    /// `now` is deliberately wrong: the daemon's clock replaces it.
    fn policy(owner: &str) -> Value {
        json!({
            "now": "2000-01-01T00:00:00Z",
            "policy": "policy:2026-10-04",
            "policy_digest": digest('5'),
            "manifest": "manifest:1",
            "manifest_digest": digest('6'),
            "recurring_grants": [{
                "grant_id": "grant:daily-report",
                "principal_id": owner,
                "familiar_id": "familiar:sage",
                "familiar_embodiment_digest": digest('1'),
                "automation_id": "automation:daily-report",
                "definition_revision": 7,
                "definition_digest": digest('2'),
                "action_type": "analysis.read",
                "action_digest": digest('3'),
                "project_id": "project:coven",
                "workspace_id": "workspace:main",
                "runtime_id": "runtime:node",
                "runtime_descriptor_digest": digest('4'),
                "runtime_capabilities": ["analysis.read"],
                "risk_classes": ["R0", "R1"],
                "capabilities": ["analysis.read"],
                "scopes": [{
                    "kind": "filesystem",
                    "root": "workspace",
                    "path": "inputs/report.json",
                    "access": "read",
                    "recursive": false
                }],
                "expires_at": "2026-11-01T00:00:00Z",
                "max_uses": 31,
                "uses": 1
            }],
            "protected_owner_approval": false,
            "recurring_approval_allowed": false
        })
    }

    fn owner(conn: &Connection) -> String {
        ensure_threads_decisions_schema(conn).unwrap();
        owner_grants::owner_principal_id(conn, "2026-10-04T08:00:00.000Z").unwrap()
    }

    fn signed_by(value: &Value, role: AuthorityKeyRole) -> bool {
        value["integrity"]["key_id"]
            .as_str()
            .unwrap()
            .starts_with(&format!("coven-local:{}:", role.as_str()))
    }

    fn decided(conn: &Connection, home: &Path, name: &str, risk: &str) -> ThreadsDecision {
        let owner = owner(conn);
        decide(
            conn,
            home,
            &draft_for(name, risk),
            &policy(&owner),
            at(9, 0),
        )
        .unwrap()
        .unwrap()
    }

    fn refusal_code(refusal: std::result::Result<ThreadsDecision, DecisionRefusal>) -> String {
        match refusal {
            Err(DecisionRefusal::Invalid { code, .. }) => code.to_owned(),
            other => panic!("expected a profile refusal, got {other:?}"),
        }
    }

    fn rows(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM automation_threads_decisions",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn each_outcome_class_maps_to_a_signed_decision_that_verifies() {
        let (temp, conn) = store();
        let owner = owner(&conn);
        for (risk, expected) in [
            ("R0", "permit"),
            ("R2", "requires_approval"),
            ("R3", "degrade_to_proposal"),
            ("R4", "reject"),
        ] {
            let decided = decided(&conn, temp.path(), risk, risk);
            assert_eq!(decided.outcome, expected, "{risk}");
            assert_eq!(decided.decision["outcome"], json!(expected), "{risk}");
            // The owner key signs the request for the owner; the decision key
            // signs the decision; the daemon's clock is the policy's.
            assert_eq!(decided.request["principal"]["id"], json!(owner));
            assert!(signed_by(
                &decided.request,
                AuthorityKeyRole::OwnerPrincipal
            ));
            assert!(signed_by(
                &decided.decision,
                AuthorityKeyRole::ThreadsDecision
            ));
            assert_eq!(decided.policy["now"], json!("2026-10-04T09:00:00.000Z"));
            assert_eq!(
                decided.decision["request_digest"],
                json!(decided.request_digest)
            );

            // An independent verifier reaches the same outcome from the
            // recorded triple, under the daemon's keys.
            let decision_id = decided.decision["decision_id"].as_str().unwrap();
            assert_eq!(recorded(&conn, decision_id).unwrap(), Ok(decided.clone()));
            let keyring = keyring_at(&conn, &owner, at(9, 0)).unwrap();
            assert_eq!(
                verify_decision_bundle(
                    &decided.request,
                    &decided.decision,
                    &decided.policy,
                    &keyring,
                    &Ed25519,
                )
                .unwrap(),
                expected
            );

            let binding = decided.binding().unwrap();
            match expected {
                "permit" => {
                    let binding = binding.unwrap();
                    assert_eq!(binding.outcome, AuthorityOutcome::Permit);
                    assert_eq!(binding.risk_class, RiskClass::R0);
                }
                "requires_approval" => {
                    let binding = binding.unwrap();
                    assert_eq!(binding.outcome, AuthorityOutcome::RequiresApproval);
                    assert_eq!(binding.risk_class, RiskClass::R2);
                    assert!(binding.capabilities.granted.as_slice().is_empty());
                }
                // Neither can authorize dispatch.
                _ => assert_eq!(binding, None, "{risk}"),
            }
        }
        assert_eq!(rows(&conn), 4);
        assert_eq!(
            recorded(&conn, "decision:none").unwrap(),
            Err(DecisionRefusal::Unknown)
        );
    }

    #[test]
    fn a_permit_fills_the_execution_binding_with_profile_digests() {
        let (temp, conn) = store();
        let decided = decided(&conn, temp.path(), "binding", "R0");
        let binding = decided.binding().unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&binding.capabilities).unwrap(),
            json!({
                "requested": ["analysis.read"],
                "granted": ["analysis.read"],
                "denied": [],
                "degraded": []
            })
        );
        let threads = serde_json::to_value(&binding.threads).unwrap();
        assert_eq!(threads["decisionId"], decided.decision["decision_id"]);
        assert_eq!(
            format!(
                "sha256:{}",
                threads["decisionDigest"]["value"].as_str().unwrap()
            ),
            decided.decision_digest
        );
        assert_eq!(threads["protectedSurfaceManifestId"], json!("manifest:1"));
    }

    #[test]
    fn a_tampered_decision_body_refuses() {
        let (temp, conn) = store();
        let decided = decided(&conn, temp.path(), "tampered", "R2");
        let owner = owner(&conn);
        let keyring = keyring_at(&conn, &owner, at(9, 0)).unwrap();

        // Any edit after signing, even one the schema accepts, breaks the signed
        // digest: here the approval requirement is widened to recurring use.
        let mut forged = decided.decision.clone();
        forged["approval_requirement"]["recurring_allowed"] = json!(true);
        let error = verify_decision_bundle(
            &decided.request,
            &forged,
            &decided.policy,
            &keyring,
            &Ed25519,
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), "integrity_digest_mismatch");

        // A row edited behind the store's back is refused on reading back.
        conn.execute_batch("DROP TRIGGER automation_threads_decisions_immutable;")
            .unwrap();
        conn.execute(
            "UPDATE automation_threads_decisions SET decision_json = ?1",
            [forged.to_string()],
        )
        .unwrap();
        let decision_id = decided.decision["decision_id"].as_str().unwrap();
        assert_eq!(
            refusal_code(recorded(&conn, decision_id).unwrap()),
            "integrity_digest_mismatch"
        );
    }

    #[test]
    fn an_expired_decision_refuses() {
        let (temp, conn) = store();
        let decided = decided(&conn, temp.path(), "window", "R0");
        let decision_id = decided.decision["decision_id"].as_str().unwrap();
        // The profile's window: from the daemon's clock at evaluation until
        // the request expires.
        assert_eq!(
            decided.decision["validity"],
            json!({ "not_before": "2026-10-04T09:00:00.000Z", "not_after": "2026-10-04T10:00:00Z" })
        );
        assert_eq!(
            valid_at(&conn, decision_id, at(9, 59)).unwrap(),
            Ok(decided.clone())
        );
        assert_eq!(
            refusal_code(valid_at(&conn, decision_id, at(10, 0)).unwrap()),
            "decision_expired"
        );
        assert_eq!(
            refusal_code(valid_at(&conn, decision_id, at(8, 59)).unwrap()),
            "decision_not_yet_valid"
        );
        assert_eq!(
            valid_at(&conn, "decision:none", at(9, 30)).unwrap(),
            Err(DecisionRefusal::Unknown)
        );

        // A request already expired at the daemon's clock is never decided.
        let owner = owner(&conn);
        let mut expired = draft("expired");
        expired["replay"]["expires_at"] = json!("2026-10-04T08:59:00Z");
        assert_eq!(
            refusal_code(decide(&conn, temp.path(), &expired, &policy(&owner), at(9, 0)).unwrap()),
            "request_expired"
        );
        assert_eq!(rows(&conn), 1);
    }

    #[test]
    fn a_revoked_key_refuses() {
        let (temp, conn) = store();
        let owner = owner(&conn);
        let decided = decided(&conn, temp.path(), "kept", "R0");
        let decision_id = decided.decision["decision_id"].as_str().unwrap();

        // Rotation closes the key's window after the decision, which still
        // verifies as of when it was made.
        authority_keys::rotate_signing_key(
            &conn,
            temp.path(),
            AuthorityKeyRole::ThreadsDecision,
            at(9, 30),
        )
        .unwrap();
        assert!(recorded(&conn, decision_id).unwrap().is_ok());
        // Outside its window, before it existed or after it closed, the key
        // is not in the keyring.
        for outside in [at(8, 0), at(9, 30)] {
            let keyring = keyring_at(&conn, &owner, outside).unwrap();
            let error = verify_decision_bundle(
                &decided.request,
                &decided.decision,
                &decided.policy,
                &keyring,
                &Ed25519,
            )
            .unwrap_err();
            assert_eq!(error.code.as_str(), "integrity_key_unknown", "{outside}");
        }

        // Revocation makes the key authenticate nothing, even for the past.
        let key_id = decided.decision["integrity"]["key_id"].as_str().unwrap();
        authority_keys::revoke_key(&conn, temp.path(), key_id, "test", at(10, 0)).unwrap();
        assert_eq!(
            refusal_code(recorded(&conn, decision_id).unwrap()),
            "integrity_key_unknown"
        );
        assert_eq!(
            refusal_code(valid_at(&conn, decision_id, at(9, 30)).unwrap()),
            "integrity_key_unknown"
        );
    }

    #[test]
    fn a_wrong_role_key_refuses() {
        let (temp, conn) = store();
        let owner = owner(&conn);
        let decided = decided(&conn, temp.path(), "roles", "R0");
        let keyring = keyring_at(&conn, &owner, at(9, 0)).unwrap();
        for (role, expected) in [
            // Not in the profile keyring at all.
            (AuthorityKeyRole::DispatchAuthority, "integrity_key_unknown"),
            (AuthorityKeyRole::FamiliarBinding, "integrity_key_unknown"),
            (AuthorityKeyRole::TerminalObserver, "integrity_key_unknown"),
            // In it, but as the owner's principal key.
            (AuthorityKeyRole::OwnerPrincipal, "integrity_role_mismatch"),
        ] {
            let key =
                authority_keys::current_signing_key(&conn, temp.path(), role, at(9, 0)).unwrap();
            let mut unsigned = decided.decision.clone();
            unsigned.as_object_mut().unwrap().remove("integrity");
            let forged = sign_artifact(&unsigned, Domain::Decision, &RoleSigner(&key)).unwrap();
            let error = verify_decision_bundle(
                &decided.request,
                &forged,
                &decided.policy,
                &keyring,
                &Ed25519,
            )
            .unwrap_err();
            assert_eq!(error.code.as_str(), expected, "{role:?}");
        }
    }

    #[test]
    fn a_replayed_decision_id_refuses() {
        let (temp, conn) = store();
        let owner = owner(&conn);
        let policy = policy(&owner);
        let first = decide(&conn, temp.path(), &draft("first"), &policy, at(9, 0))
            .unwrap()
            .unwrap();
        // The same request at the same clock is the same decision id.
        assert_eq!(
            decide(&conn, temp.path(), &draft("first"), &policy, at(9, 0)).unwrap(),
            Err(DecisionRefusal::Replayed)
        );
        // So is a later attempt at it, or any reuse of its nonce or adoption key.
        let mut same_nonce = draft("second");
        same_nonce["replay"]["nonce"] = json!("nonce:first");
        let mut same_adoption = draft("third");
        same_adoption["replay"]["adoption_key"] = json!("adopt:first");
        for request in [draft("first"), same_nonce, same_adoption] {
            assert_eq!(
                decide(&conn, temp.path(), &request, &policy, at(9, 1)).unwrap(),
                Err(DecisionRefusal::Replayed)
            );
        }
        assert_eq!(rows(&conn), 1);
        let decision_id = first.decision["decision_id"].as_str().unwrap();
        assert_eq!(recorded(&conn, decision_id).unwrap(), Ok(first));
    }

    #[test]
    fn recorded_rows_are_immutable() {
        let (temp, conn) = store();
        decided(&conn, temp.path(), "immutable", "R0");
        let error = conn
            .execute(
                "UPDATE automation_threads_decisions SET outcome = 'reject'",
                [],
            )
            .unwrap_err();
        assert!(error.to_string().contains("immutable"), "{error}");
        let error = conn
            .execute("DELETE FROM automation_threads_decisions", [])
            .unwrap_err();
        assert!(error.to_string().contains("retained"), "{error}");
        assert_eq!(rows(&conn), 1);
    }

    #[test]
    fn refuses_with_the_profiles_first_code_and_records_nothing() {
        let (temp, conn) = store();
        let owner = owner(&conn);
        let mut unknown = draft("unknown");
        unknown["requested_capabilities"] = json!(["prompt.override"]);
        let mut stale = policy(&owner);
        stale["policy_digest"] = json!(digest('7'));
        for (request, policy, code) in [
            (unknown, policy(&owner), "capability_unknown"),
            (draft("stale"), stale, "policy_stale"),
        ] {
            assert_eq!(
                refusal_code(decide(&conn, temp.path(), &request, &policy, at(9, 0)).unwrap()),
                code
            );
        }
        assert_eq!(rows(&conn), 0);
        // A draft that arrives signed is a caller bug, not a refusal.
        assert!(decide(
            &conn,
            temp.path(),
            &json!({ "integrity": {} }),
            &policy(&owner),
            at(9, 0)
        )
        .is_err());
    }
}
