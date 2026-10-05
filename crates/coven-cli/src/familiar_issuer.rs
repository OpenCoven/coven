//! Familiar Contract embodiment bindings (coven#857, slice 3).
//!
//! The daemon issues a `familiar.embodiment_binding.v1` binding from the
//! familiar ledger's current head, signs it with its `familiar-binding` key,
//! and only hands it out after it passes two independent checks:
//!
//! - the Familiar Contract's own verifier (the pinned `familiar-contract`
//!   crate), run against the head's retained bundle and a trusted-ledger
//!   observation read in the same transaction; and
//! - Coven's key policy: the binding, and every lineage transition it cites,
//!   must be signed by a `familiar-binding` key the daemon trusts at the time
//!   each was signed. The verifier alone only proves that a document verifies
//!   under the key it carries.
//!
//! Issuance refuses a familiar with no live root, a head that is not active,
//! and declarations that no longer match the head: the owner must adopt them
//! first. Every issued binding is recorded, and its projection is the
//! execution binding's `familiar` member, mapped to Coven's vocabulary.

use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::{json, Value};

use crate::automations::authority_keys::{self, AuthorityKeyRole};
use crate::automations::contract::authority::AuthorityFamiliarBinding;
use crate::familiar_ledger::{self, LedgerHead, RevisionRecord};

/// The verifier's fixed v1 ceiling, which the binding also states as its own
/// freshness bound. Coven's projection carries it as `freshnessBoundSeconds`.
const FRESHNESS_BOUND_SECONDS: u16 = 300;
const FRESHNESS_POLICY_VERSION: &str = "familiar-freshness:v1";
const POLICY_VERSION: &str = "coven-familiar-binding-policy:v1";

pub(crate) const FAMILIAR_BINDINGS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS familiar_bindings (
        binding_id TEXT PRIMARY KEY NOT NULL,
        root_id TEXT NOT NULL REFERENCES familiar_ledger_roots (root_id),
        revision_id TEXT NOT NULL REFERENCES familiar_ledger_revisions (revision_id),
        binding_digest TEXT NOT NULL UNIQUE CHECK (length(binding_digest) = 64),
        binding_json TEXT NOT NULL,
        purpose TEXT NOT NULL CHECK (purpose IN ('dispatch', 'session_creation')),
        target_type TEXT NOT NULL,
        target_id TEXT NOT NULL,
        ledger_generation INTEGER NOT NULL CHECK (ledger_generation >= 1),
        issued_at TEXT NOT NULL
    );
    CREATE TRIGGER IF NOT EXISTS familiar_bindings_immutable
    BEFORE UPDATE ON familiar_bindings
    BEGIN SELECT RAISE(ABORT, 'issued familiar bindings are immutable'); END;
    CREATE TRIGGER IF NOT EXISTS familiar_bindings_retained
    BEFORE DELETE ON familiar_bindings
    BEGIN SELECT RAISE(ABORT, 'issued familiar bindings are retained'); END;
";

/// What a binding authorizes the familiar to embody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingPurpose {
    Dispatch,
    SessionCreation,
}

impl BindingPurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::SessionCreation => "session_creation",
        }
    }
}

/// The concrete execution a binding is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetType {
    DirectSession,
    AutomationRun,
    AutomationAttempt,
    PsycheDelegatedExecution,
}

impl TargetType {
    fn as_str(self) -> &'static str {
        match self {
            Self::DirectSession => "direct_session",
            Self::AutomationRun => "automation_run",
            Self::AutomationAttempt => "automation_attempt",
            Self::PsycheDelegatedExecution => "psyche_delegated_execution",
        }
    }
}

/// One binding request.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BindingRequest<'a> {
    /// The roster id the caller names; it resolves to the live root.
    pub familiar_id: &'a str,
    pub purpose: BindingPurpose,
    pub target_type: TargetType,
    /// The execution's own opaque id, such as an automation run id.
    pub target_id: &'a str,
    /// The authenticated principal the execution runs for.
    pub principal_id: &'a str,
}

/// An issued, verified binding.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IssuedBinding {
    /// The `familiar.embodiment_binding.v1` document.
    pub binding: Value,
    pub binding_digest: String,
    /// The execution binding's `familiar` member.
    pub authority: AuthorityFamiliarBinding,
}

/// Why no binding was issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IssueRefusal {
    /// The roster id has no live root: it was never registered, or retired.
    NotRegistered,
    /// The head is revoked, so the familiar has no active revision.
    NotActive { revision_id: String, status: String },
    /// The roster entry or declaration files changed since the head was
    /// recorded; the owner must adopt them first.
    DeclarationsChanged { revision_id: String },
}

pub(crate) fn ensure_familiar_bindings_schema(conn: &Connection) -> Result<()> {
    familiar_ledger::ensure_familiar_ledger_schema(conn)?;
    conn.execute_batch(FAMILIAR_BINDINGS_SCHEMA_SQL)
        .context("failed to initialize familiar bindings schema")
}

/// Issues a binding for `request` at `now`. The ledger read, the
/// observation, the verification and the record share one transaction, so
/// the binding names exactly the ledger state it was decided against.
pub(crate) fn issue(
    conn: &Connection,
    coven_home: &Path,
    request: &BindingRequest<'_>,
    now: DateTime<Utc>,
) -> Result<std::result::Result<IssuedBinding, IssueRefusal>> {
    let now = chrono::DurationRound::duration_trunc(now, TimeDelta::milliseconds(1))
        .context("binding time cannot be represented")?;
    ensure_familiar_bindings_schema(conn)?;
    // The key store opens its own transaction, so the key comes first.
    let key = authority_keys::current_signing_key(
        conn,
        coven_home,
        AuthorityKeyRole::FamiliarBinding,
        now,
    )?;
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .context("failed to begin familiar binding transaction")?;
    let Some(head) = familiar_ledger::live_head(&transaction, request.familiar_id)? else {
        return Ok(Err(IssueRefusal::NotRegistered));
    };
    if head.head.status != "active" {
        return Ok(Err(IssueRefusal::NotActive {
            revision_id: head.head.revision_id.clone(),
            status: head.head.status.clone(),
        }));
    }
    if !familiar_ledger::head_is_current(coven_home, &head)? {
        return Ok(Err(IssueRefusal::DeclarationsChanged {
            revision_id: head.head.revision_id.clone(),
        }));
    }

    let mut binding = unsigned_binding(&transaction, &head, request, now)?;
    let binding_digest = familiar_contract::binding_digest(&binding);
    binding["integrity"]["bindingDigest"] = json!(binding_digest);
    binding["commit"]["verifiedBindingDigest"] = json!(binding_digest);
    binding["authentication"] = familiar_ledger::contract_authentication(&key, &binding_digest)?;

    // The contract's own verifier, against the retained bundle and the
    // ledger as it stands in this transaction.
    let observation = familiar_ledger::observation(&transaction, &head.root.root_id, now)?
        .context("the familiar ledger root disappeared mid-transaction")?;
    let binding_text = binding.to_string();
    let observation_text = observation.to_string();
    let violations = familiar_contract::verify(&familiar_contract::EmbodimentInputs {
        binding: &binding_text,
        historical_bundle: Some(&head.head.bundle_json),
        trusted_ledger: Some(&observation_text),
        post_commit_revocation: None,
    });
    anyhow::ensure!(
        violations.is_empty(),
        "issued familiar binding failed the Familiar Contract verifier: {}",
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    );
    // Coven's key policy, for the binding and every transition it cites.
    authenticate(
        &transaction,
        &binding["authentication"],
        &binding_digest,
        now,
    )?;
    if let Some(predecessor) = binding["familiar"]["lineageEvidence"].get("predecessor") {
        let transition = &predecessor["transition"];
        let signed_at = DateTime::parse_from_rfc3339(&head.head.recorded_at)
            .context("revision time is RFC 3339")?
            .with_timezone(&Utc);
        authenticate(
            &transaction,
            &transition["authentication"],
            &transition_digest(transition)?,
            signed_at,
        )?;
    }

    let authority = projection(&binding, &head, now)?;
    transaction
        .execute(
            "INSERT INTO familiar_bindings
                (binding_id, root_id, revision_id, binding_digest, binding_json, purpose,
                 target_type, target_id, ledger_generation, issued_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                binding["bindingId"].as_str(),
                head.root.root_id,
                head.head.revision_id,
                binding_digest,
                binding_text,
                request.purpose.as_str(),
                request.target_type.as_str(),
                request.target_id,
                i64::try_from(head.root.generation).context("generation exceeds SQLite range")?,
                timestamp(now),
            ],
        )
        .context("failed to record the issued familiar binding")?;
    transaction
        .commit()
        .context("failed to commit the issued familiar binding")?;
    Ok(Ok(IssuedBinding {
        binding,
        binding_digest,
        authority,
    }))
}

/// The binding before its digest and signature. Every decision time is
/// `now`: for dispatch the contract requires the final check, decision and
/// commit to share one instant.
fn unsigned_binding(
    conn: &Connection,
    head: &LedgerHead,
    request: &BindingRequest<'_>,
    now: DateTime<Utc>,
) -> Result<Value> {
    let revision = &head.head;
    let at = timestamp(now);
    let snapshot_id = format!("snapshot:{}", random_hex()?);
    let mut lineage = json!({
        "relationship": revision.relationship,
        "rootEvidence": if revision.relationship == "genesis" { "genesis" } else { "continued" },
    });
    if let Some(predecessor_id) = &revision.predecessor_revision_id {
        let predecessor = familiar_ledger::revision(conn, predecessor_id)?
            .context("a revision's predecessor is missing")?;
        let transition: Value = serde_json::from_str(
            revision
                .transition_json
                .as_deref()
                .context("a revision after genesis has a transition")?,
        )
        .context("stored transition is JSON")?;
        lineage["predecessor"] = json!({
            "familiarRootId": predecessor.root_id,
            "identityRevisionId": predecessor.revision_id,
            "lineagePosition": predecessor.lineage_position,
            "status": predecessor.status,
            "identityBundleRef": format!("urn:sha256:{}", predecessor.bundle_digest),
            "transition": transition,
        });
    }
    let bundle_digest = json!({ "algorithm": "sha-256", "value": revision.bundle_digest });
    Ok(json!({
        "profile": "familiar.embodiment_binding.v1",
        "schemaVersion": "1.0.0",
        "bindingId": format!("familiar-binding:{}", random_hex()?),
        "familiar": {
            "familiarRootId": revision.root_id,
            "identityRevisionId": revision.revision_id,
            "lineagePosition": revision.lineage_position,
            "lineageEvidence": lineage,
        },
        "resolutionSnapshot": {
            "snapshotId": snapshot_id,
            "resolvedAt": at,
            "authoritativeLedgerGeneration": head.root.generation,
            "authoritativeHeadRevisionId": revision.revision_id,
            "freshnessBoundSeconds": FRESHNESS_BOUND_SECONDS,
            "cacheObservedAt": at,
            "cacheProvenance": "authoritative-read",
            "familiarRootId": revision.root_id,
            "identityRevisionId": revision.revision_id,
            "lineagePosition": revision.lineage_position,
            "bundleDigest": revision.bundle_digest,
            "status": revision.status,
        },
        // The roster id the caller named is non-authoritative evidence.
        "aliasResolution": {
            "alias": request.familiar_id,
            "authoritative": false,
            "resolvedRootIds": [revision.root_id],
        },
        "identityBundle": {
            "canonicalization": "jcs-rfc8785",
            "declarationDigest": { "algorithm": "sha-256", "value": revision.declaration_digest },
            "bundleDigest": bundle_digest,
            "historicalBundleRef": format!("urn:sha256:{}", revision.bundle_digest),
        },
        "revisionRecordedAt": revision.recorded_at,
        "validTime": { "notBefore": revision.valid_from },
        "statusAtDecision": { "status": revision.status, "decisionTime": at },
        "principal": { "authenticatedPrincipalId": request.principal_id },
        "target": {
            "targetType": request.target_type.as_str(),
            "targetId": request.target_id,
            "authenticatedPrincipalId": request.principal_id,
        },
        "bindingPurpose": request.purpose.as_str(),
        "identityMeaning": "unchanged",
        "issuedAt": at,
        "decisionAt": at,
        "policyVersion": POLICY_VERSION,
        "resolver": { "id": "coven:familiar-ledger", "version": env!("CARGO_PKG_VERSION") },
        "verifier": { "id": "familiar-contract:verifier", "version": "0.7.0" },
        "historicalVerification": { "state": "verified", "readAuthorization": "not_requested" },
        "revocation": { "outcome": "none" },
        "commit": {
            "snapshotId": snapshot_id,
            "state": "committed",
            "finalValidityCheckAt": at,
            "committedAt": at,
            "verifiedBindingDigest": "",
        },
        "integrity": {
            "algorithm": "sha-256",
            "canonicalization": "jcs-rfc8785",
            "bindingDigest": "",
        },
        "authentication": {},
        "privacy": {
            "classification": "metadata-only",
            "retention": "dispatch-audit-minimum",
            "containsSensitiveIdentityContent": false,
            "recordedAt": at,
            "tombstoneState": "live",
            "replicaPurgeState": "not_requested",
        },
    }))
}

/// The digest a stored transition signs.
fn transition_digest(transition: &Value) -> Result<String> {
    let field = |name: &str| {
        transition[name]
            .as_str()
            .with_context(|| format!("transition has no `{name}`"))
    };
    Ok(familiar_contract::transition_digest(
        &familiar_contract::TransitionPreimage {
            relationship: field("relationship")?,
            predecessor_bundle_digest: field("predecessorBundleDigest")?,
            successor_familiar_root_id: field("successorFamiliarRootId")?,
            successor_identity_revision_id: field("successorIdentityRevisionId")?,
            successor_bundle_digest: field("successorBundleDigest")?,
            successor_declaration_digest: field("successorDeclarationDigest")?,
        },
    ))
}

/// Requires a Familiar Contract `authentication` member to be a signature by
/// the `familiar-binding` key named by its `signerId`, trusted at
/// `signed_at`, over `digest_hex`, and to carry that key's own public key.
fn authenticate(
    conn: &Connection,
    authentication: &Value,
    digest_hex: &str,
    signed_at: DateTime<Utc>,
) -> Result<()> {
    let signer_id = authentication["signerId"]
        .as_str()
        .context("authentication has no signer")?;
    let record = authority_keys::key_records(conn)?
        .into_iter()
        .find(|record| {
            record.role == AuthorityKeyRole::FamiliarBinding && record.key_id == signer_id
        })
        .with_context(|| format!("`{signer_id}` is not a familiar-binding key of this daemon"))?;
    let base64 = base64::engine::general_purpose::STANDARD;
    let carried = base64
        .decode(authentication["publicKey"].as_str().unwrap_or_default())
        .context("authentication public key is base64")?;
    anyhow::ensure!(
        hex(&carried) == record.public_key_der_hex,
        "`{signer_id}` signed with a public key other than its own"
    );
    let signature = base64
        .decode(authentication["signature"].as_str().unwrap_or_default())
        .context("authentication signature is base64")?;
    authority_keys::trusted_keys(conn, AuthorityKeyRole::FamiliarBinding)?
        .authenticate(
            &record.key_id,
            &record.proof_ref,
            (&record.producer_component, &record.producer_instance_id),
            signed_at,
            digest_hex,
            &hex(&signature),
        )
        .map_err(|refusal| {
            anyhow::anyhow!("`{signer_id}` is not trusted at {signed_at}: {refusal:?}")
        })
}

/// The execution binding's `familiar` member. Coven's contract admits only an
/// active, unrevoked, unretired revision, and needs a closed validity window:
/// the binding's own `notAfter` when it has one, otherwise the decision time
/// plus the freshness bound, exclusive.
fn projection(
    binding: &Value,
    head: &LedgerHead,
    now: DateTime<Utc>,
) -> Result<AuthorityFamiliarBinding> {
    let revision: &RevisionRecord = &head.head;
    let at = timestamp(now);
    let not_after = timestamp(now + TimeDelta::seconds(i64::from(FRESHNESS_BOUND_SECONDS)));
    let digest = |value: &str| json!({ "algorithm": "sha256", "canonicalization": "jcs-rfc8785", "value": value });
    serde_json::from_value(json!({
        "familiarRootId": revision.root_id,
        "identityRevisionId": revision.revision_id,
        "declarationDigest": digest(&revision.declaration_digest),
        "embodimentBindingId": binding["bindingId"],
        "embodimentDigest": digest(binding["integrity"]["bindingDigest"].as_str().unwrap_or_default()),
        "statusAtDecision": "active",
        "verifiedAt": at,
        "freshnessPolicyVersion": FRESHNESS_POLICY_VERSION,
        "freshnessBoundSeconds": FRESHNESS_BOUND_SECONDS,
        "validTime": { "notBefore": revision.valid_from, "notAfter": not_after },
        "revocation": { "state": "not_revoked", "checkedAt": at },
        "retirement": { "state": "not_retired", "checkedAt": at },
    }))
    .context("the issued binding does not fit Coven's familiar binding")
}

fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn random_hex() -> Result<String> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0_u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to draw a random identifier"))?;
    Ok(hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::familiar_ledger::tests::{at, revision_id, Home};

    const PRINCIPAL: &str = "principal:coven-local-owner:0123456789abcdef0123456789abcdef";

    fn request(target_id: &str) -> BindingRequest<'_> {
        BindingRequest {
            familiar_id: "sage",
            purpose: BindingPurpose::Dispatch,
            target_type: TargetType::AutomationRun,
            target_id,
            principal_id: PRINCIPAL,
        }
    }

    fn issue_at(home: &Home, seconds: i64) -> IssuedBinding {
        issue(
            &home.conn,
            home.path(),
            &request("automation-run:nightly-1"),
            at(seconds),
        )
        .unwrap()
        .expect("a binding")
    }

    /// Re-runs the contract verifier on an issued binding, as an independent
    /// consumer would, and checks a tampered copy fails it.
    fn assert_verifies(home: &Home, issued: &IssuedBinding, seconds: i64) {
        let head = familiar_ledger::live_head(&home.conn, "sage")
            .unwrap()
            .unwrap();
        let observation = familiar_ledger::observation(&home.conn, &head.root.root_id, at(seconds))
            .unwrap()
            .unwrap()
            .to_string();
        let verify = |binding: &Value| {
            familiar_contract::verify(&familiar_contract::EmbodimentInputs {
                binding: &binding.to_string(),
                historical_bundle: Some(&head.head.bundle_json),
                trusted_ledger: Some(&observation),
                post_commit_revocation: None,
            })
        };
        assert_eq!(verify(&issued.binding), Vec::new());
        let mut tampered = issued.binding.clone();
        tampered["target"]["targetId"] = json!("automation-run:other");
        assert!(verify(&tampered)
            .iter()
            .any(|violation| violation.code == familiar_contract::Code::BindingDigest));
    }

    #[test]
    fn issues_a_verified_genesis_binding_and_its_projection() {
        let home = Home::new();
        let genesis = revision_id(&home.register("adopt:issue:register"));
        let issued = issue_at(&home, 0);
        assert_verifies(&home, &issued, 0);

        let binding = &issued.binding;
        assert_eq!(binding["familiar"]["identityRevisionId"], genesis.as_str());
        assert_eq!(
            binding["familiar"]["lineageEvidence"],
            json!({"relationship": "genesis", "rootEvidence": "genesis"})
        );
        assert_eq!(binding["aliasResolution"]["alias"], "sage");
        assert_eq!(binding["target"]["targetId"], "automation-run:nightly-1");
        assert!(binding["authentication"]["signerId"]
            .as_str()
            .unwrap()
            .starts_with("coven-local:familiar-binding:"));
        assert_eq!(
            issued.binding_digest,
            familiar_contract::binding_digest(binding)
        );

        let authority = serde_json::to_value(&issued.authority).unwrap();
        let head = home.head();
        assert_eq!(authority["familiarRootId"], head.root.root_id.as_str());
        assert_eq!(authority["identityRevisionId"], genesis.as_str());
        assert_eq!(
            authority["declarationDigest"],
            json!({"algorithm": "sha256", "canonicalization": "jcs-rfc8785", "value": head.head.declaration_digest})
        );
        assert_eq!(
            authority["embodimentDigest"]["value"],
            issued.binding_digest.as_str()
        );
        assert_eq!(authority["embodimentBindingId"], binding["bindingId"]);
        assert_eq!(authority["statusAtDecision"], "active");
        assert_eq!(
            authority["validTime"],
            json!({"notBefore": "2026-10-03T21:00:00.000Z", "notAfter": "2026-10-03T21:05:00.000Z"})
        );
        assert_eq!(home.count("familiar_bindings"), 1);
        assert!(home
            .conn
            .execute("UPDATE familiar_bindings SET target_id = 'x'", [])
            .is_err());
    }

    #[test]
    fn adopted_and_restored_revisions_carry_their_signed_lineage() {
        let home = Home::new();
        let genesis = revision_id(&home.register("adopt:issue:register"));
        home.write("sage", "SOUL.md", "# SOUL\nAdopted.\n");
        let (status, adopted) = home.adopt("adopt:issue:adopt", &genesis);
        assert_eq!(status, 200, "{adopted:?}");
        let adopted = revision_id(&adopted.result.unwrap());
        let issued = issue_at(&home, 1);
        assert_verifies(&home, &issued, 1);
        let lineage = &issued.binding["familiar"]["lineageEvidence"];
        assert_eq!(lineage["relationship"], "same_familiar_revision");
        assert_eq!(lineage["rootEvidence"], "continued");
        assert_eq!(
            lineage["predecessor"]["identityRevisionId"],
            genesis.as_str()
        );
        assert_eq!(lineage["predecessor"]["status"], "superseded");

        let root_id = home.head().root.root_id;
        home.ok(json!({
            "action": "coven.familiars.ledger.retire.v1", "adoptionKey": "adopt:issue:retire",
            "familiarId": "sage", "expectedRevisionId": adopted,
        }));
        assert_eq!(
            issue(
                &home.conn,
                home.path(),
                &request("automation-run:retired"),
                at(2)
            )
            .unwrap(),
            Err(IssueRefusal::NotRegistered)
        );
        home.ok(json!({
            "action": "coven.familiars.ledger.restore.v1", "adoptionKey": "adopt:issue:restore",
            "rootId": root_id, "expectedRevisionId": adopted,
        }));
        let restored = issue_at(&home, 3);
        assert_verifies(&home, &restored, 3);
        let lineage = &restored.binding["familiar"]["lineageEvidence"];
        assert_eq!(lineage["relationship"], "restoration");
        assert_eq!(lineage["predecessor"]["status"], "retired");
    }

    #[test]
    fn refuses_changed_declarations_and_revoked_heads() {
        let home = Home::new();
        assert_eq!(
            issue(
                &home.conn,
                home.path(),
                &request("automation-run:early"),
                at(0)
            )
            .unwrap(),
            Err(IssueRefusal::NotRegistered)
        );
        let genesis = revision_id(&home.register("adopt:issue:register"));
        home.write(
            "sage",
            "IDENTITY.md",
            "# IDENTITY.md - Sage\n- **Name:** Sage II\n",
        );
        assert_eq!(
            issue(
                &home.conn,
                home.path(),
                &request("automation-run:changed"),
                at(1)
            )
            .unwrap(),
            Err(IssueRefusal::DeclarationsChanged {
                revision_id: genesis.clone()
            })
        );
        home.ok(json!({
            "action": "coven.familiars.ledger.revoke.v1", "adoptionKey": "adopt:issue:revoke",
            "revisionId": genesis, "reason": "identity compromised",
        }));
        assert_eq!(
            issue(
                &home.conn,
                home.path(),
                &request("automation-run:revoked"),
                at(2)
            )
            .unwrap(),
            Err(IssueRefusal::NotActive {
                revision_id: genesis,
                status: "revoked".to_owned()
            })
        );
        assert_eq!(home.count("familiar_bindings"), 0);
    }

    #[test]
    fn a_corrupted_ledger_never_yields_a_binding() {
        // If stored evidence were ever altered, the contract verifier run at
        // issuance refuses it rather than signing over it.
        let home = Home::new();
        home.register("adopt:issue:register");
        home.conn
            .execute_batch("DROP TRIGGER familiar_ledger_revision_evidence_immutable;")
            .unwrap();
        let tampered: String = home
            .conn
            .query_row(
                "SELECT bundle_json FROM familiar_ledger_revisions",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut bundle: Value = serde_json::from_str(&tampered).unwrap();
        bundle["retention"]["recordedAt"] = json!("2026-10-03T20:00:00.000Z");
        home.conn
            .execute(
                "UPDATE familiar_ledger_revisions SET bundle_json = ?1",
                [bundle.to_string()],
            )
            .unwrap();
        let error = issue(
            &home.conn,
            home.path(),
            &request("automation-run:corrupt"),
            at(1),
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("E_BUNDLE_DIGEST"),
            "{error:#}"
        );
        assert_eq!(home.count("familiar_bindings"), 0);
    }

    #[test]
    fn lineage_signed_by_a_rotated_key_verifies_until_that_key_is_revoked() {
        let home = Home::new();
        let genesis = revision_id(&home.register("adopt:issue:register"));
        home.write("sage", "SOUL.md", "# SOUL\nAdopted.\n");
        home.adopt("adopt:issue:adopt", &genesis);
        let transition_key = authority_keys::current_signing_key(
            &home.conn,
            home.path(),
            AuthorityKeyRole::FamiliarBinding,
            at(0),
        )
        .unwrap()
        .record()
        .key_id
        .clone();

        // After rotation the binding is signed by the new key, and the
        // transition still verifies under the old key's window.
        authority_keys::rotate_signing_key(
            &home.conn,
            home.path(),
            AuthorityKeyRole::FamiliarBinding,
            at(10),
        )
        .unwrap();
        let issued = issue_at(&home, 20);
        assert_verifies(&home, &issued, 20);
        assert_ne!(
            issued.binding["authentication"]["signerId"],
            transition_key.as_str()
        );
        assert_eq!(
            issued.binding["familiar"]["lineageEvidence"]["predecessor"]["transition"]
                ["authentication"]["signerId"],
            transition_key.as_str()
        );

        // Revoking the key that signed the transition stops issuance.
        authority_keys::revoke_key(
            &home.conn,
            home.path(),
            &transition_key,
            "lost laptop",
            at(30),
        )
        .unwrap();
        let error = issue(
            &home.conn,
            home.path(),
            &request("automation-run:after"),
            at(40),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("not trusted"), "{error:#}");
        assert_eq!(home.count("familiar_bindings"), 1);
    }
}
