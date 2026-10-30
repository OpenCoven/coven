//! Owner-root access recovery: enroll a device when no trusted introducer is
//! available. It is a distinct, separately audited operation from trusted-device
//! introduction (no device approves), from owner-root rotation (the root key is
//! unchanged), and from familiar identity, which recovery never reads or writes.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::assurance::{validate_public_key, verify_assurance_signature};
use super::audit::{append_event, append_introduction_event, MobileAuditEvent};
use super::grant::{DeviceActionIntent, DeviceGrant, DeviceScope, DEVICE_ACTION_VERSION};
use super::introduction::{validate_destination, IntroductionCommitUncertain, TransitionKind};
use super::owner_policy::{InstalledPolicy, OwnerPolicyStore};
use super::registry::{DeviceAuthorizationRecord, DeviceRecord, DeviceRegistry, PendingEnrollment};

#[cfg(test)]
thread_local! {
    /// Runs inside the commit after every check passes, before the registry write.
    static BEFORE_RECOVERY_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

const APPROVE_OPERATION: &str = "owner.access_recovery.approve";
const ACCEPT_OPERATION: &str = "owner.access_recovery.accept";

/// Every field is material. The owner-root signature binds the exact policy
/// head it was made under, so it cannot be replayed after any policy change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccessRecoveryRequest {
    pub version: u16,
    pub audience: [u8; 32],
    pub policy_revision: u64,
    pub policy_head_digest: String,
    pub destination_id: Uuid,
    pub destination_public_key: String,
    pub device_name: String,
    pub device_context: String,
    pub grant: DeviceGrant,
    pub nonce: [u8; 32],
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl AccessRecoveryRequest {
    pub fn action(&self) -> Result<DeviceActionIntent> {
        self.validate()?;
        let mut material = b"COVEN-ACCESS-RECOVERY/1\0".to_vec();
        material.extend(serde_jcs::to_vec(self)?);
        Ok(DeviceActionIntent {
            version: DEVICE_ACTION_VERSION,
            scope: DeviceScope::DeviceAdmin,
            operation: APPROVE_OPERATION.into(),
            target: URL_SAFE_NO_PAD.encode(self.audience),
            effect_digest: URL_SAFE_NO_PAD.encode(Sha256::digest(material)),
            nonce: URL_SAFE_NO_PAD.encode(self.nonce),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
        })
    }

    /// The recovering device proves possession of the key being enrolled.
    pub fn possession_bytes(&self) -> Result<Vec<u8>> {
        let mut action = self.action()?;
        action.operation = ACCEPT_OPERATION.into();
        Ok(action.canonical_bytes()?)
    }

    fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.audience == [0; 32]
            || self.nonce == [0; 32]
            || self.policy_revision == 0
            || self.destination_id.is_nil()
        {
            bail!("invalid access recovery request");
        }
        let digest = URL_SAFE_NO_PAD.decode(&self.policy_head_digest)?;
        if digest.len() != 32 || URL_SAFE_NO_PAD.encode(digest) != self.policy_head_digest {
            bail!("noncanonical access recovery policy digest");
        }
        validate_public_key(&self.destination_public_key)?;
        validate_destination(
            &self.grant,
            &self.destination_public_key,
            &self.device_name,
            &self.device_context,
            self.issued_at,
            self.expires_at,
        )
    }
}

/// What a recovering device hands the owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccessRecoveryEnvelope {
    pub request: AccessRecoveryRequest,
    pub destination_signature: String,
}

pub struct AccessRecoveryAuthority {
    home: PathBuf,
    audience: [u8; 32],
    store: OwnerPolicyStore,
    registry: DeviceRegistry,
}

impl AccessRecoveryAuthority {
    pub fn load(home: &Path, audience: [u8; 32]) -> Result<Self> {
        Ok(Self {
            home: home.to_owned(),
            audience,
            store: OwnerPolicyStore::open(home, audience)?,
            registry: DeviceRegistry::load(home)?,
        })
    }

    pub fn current_policy(&self) -> Result<InstalledPolicy> {
        self.store
            .current()?
            .context("introduction policy is not installed; recovery needs its owner root")
    }

    /// Enrolls the destination only with the installed owner-root signature over
    /// the exact request. Rejected and uncertain outcomes are audited too.
    pub fn recover(
        &self,
        request: &AccessRecoveryRequest,
        destination_signature: &str,
        root_signature: &str,
        now: DateTime<Utc>,
    ) -> Result<DeviceAuthorizationRecord> {
        let committed = self.recover_inner(request, destination_signature, root_signature, now);
        let (destination, transition) = match committed {
            Ok(committed) => committed,
            Err(error) => {
                let event = if error.is::<IntroductionCommitUncertain>() {
                    MobileAuditEvent::AccessRecoveryUncertain
                } else {
                    MobileAuditEvent::AccessRecoveryRejected
                };
                append_event(&self.home, now, event, None).context(format!(
                    "access recovery failed ({error}); audit delivery failed"
                ))?;
                return Err(error);
            }
        };
        let receipt = append_introduction_event(&self.home, &transition)
            .context("access recovery committed; audit delivery pending")?;
        self.registry
            .mark_introduction_audited(&receipt, now)
            .context("access recovery committed; audit receipt persistence pending")?;
        Ok(destination)
    }

    fn recover_inner(
        &self,
        request: &AccessRecoveryRequest,
        destination_signature: &str,
        root_signature: &str,
        now: DateTime<Utc>,
    ) -> Result<(
        DeviceAuthorizationRecord,
        super::introduction::IntroductionTransition,
    )> {
        let action = request.action()?;
        let record = DeviceRecord {
            id: request.destination_id,
            display_name: request.device_name.clone(),
            public_key_x963: request.destination_public_key.clone(),
            paired_at: request.issued_at,
            revoked_at: None,
            suspended_at: None,
            scopes: request.grant.scopes.clone(),
        };
        let nonce_digest = URL_SAFE_NO_PAD.encode(Sha256::digest(request.nonce));
        // Policy → registry: the head is read and the enrollment written under
        // one policy lock, so a rotation or replacement cannot land between them.
        self.store.with_current_locked(|head| {
            let head = head
                .context("introduction policy is not installed; recovery needs its owner root")?;
            // No source device: the owner-root key is the sole approver, so the
            // commit locks no introducer state and shares the one nonce ledger.
            self.registry.commit_introduction(
                &[],
                PendingEnrollment {
                    device: record,
                    grant: request.grant.clone(),
                    nonce_digest,
                    kind: TransitionKind::AccessRecovery,
                },
                now,
                |_| {
                    self.validate_against(request, head, now)?;
                    verify_assurance_signature(
                        &request.destination_public_key,
                        &request.possession_bytes()?,
                        destination_signature,
                    )?;
                    verify_assurance_signature(
                        &head.owner_root_key,
                        &action.canonical_bytes()?,
                        root_signature,
                    )
                    .context("access recovery requires the installed owner-root signature")?;
                    #[cfg(test)]
                    BEFORE_RECOVERY_WRITE.with(|hook| hook.borrow_mut().take().map(|hook| hook()));
                    Ok(())
                },
            )
        })
    }

    fn validate_against(
        &self,
        request: &AccessRecoveryRequest,
        head: &InstalledPolicy,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if request.audience != self.audience
            || request.policy_revision != head.revision
            || request.policy_head_digest != head.digest
        {
            bail!("access recovery is bound to another installation or policy state");
        }
        if request.issued_at > now || now >= request.expires_at {
            bail!("access recovery request is outside its validity window");
        }
        let policy = &head.policy;
        let grant = &request.grant;
        let expiry = grant.expires_at.context("recovered grant must expire")?;
        if grant.not_before > now
            || now >= expiry
            || expiry - grant.issued_at
                > chrono::Duration::seconds(i64::from(policy.maximum_grant_lifetime_seconds))
            || grant
                .scopes
                .iter()
                .any(|scope| policy.allowed_scopes.binary_search(scope).is_err())
        {
            bail!("access recovery grant exceeds the owner policy");
        }
        Ok(())
    }
}

/// Every field the owner-root signature covers, in binding order.
pub(super) fn render_recovery_review(
    request: &AccessRecoveryRequest,
    action: &DeviceActionIntent,
) -> String {
    let grant = &request.grant;
    let scopes: Vec<_> = grant.scopes.iter().map(|scope| scope.as_str()).collect();
    let mut out = String::from("Review the exact owner-root access recovery:\n");
    let mut field = |name: &str, value: String| out.push_str(&format!("  {name:<28} {value}\n"));
    field("operation", action.operation.clone());
    field("installation (audience)", action.target.clone());
    field(
        "owner policy",
        format!(
            "revision {} ({})",
            request.policy_revision, request.policy_head_digest
        ),
    );
    field("recovered device id", request.destination_id.to_string());
    field(
        "recovered device key",
        format!(
            "sha256:{}",
            URL_SAFE_NO_PAD
                .encode(&Sha256::digest(request.destination_public_key.as_bytes())[..12])
        ),
    );
    field("recovered device name", request.device_name.clone());
    field("recovered device context", request.device_context.clone());
    field("grant id", grant.id.to_string());
    field("grant scopes", scopes.join(", "));
    field("grant not before", timestamp(grant.not_before));
    field(
        "grant expires at",
        grant.expires_at.map_or_else(|| "(never)".into(), timestamp),
    );
    field("nonce", action.nonce.clone());
    field("issued at", timestamp(request.issued_at));
    field("expires at", timestamp(request.expires_at));
    field("effect digest", action.effect_digest.clone());
    out.push_str(
        "  This recovers access only. It does not rotate the owner-root key, revoke other\n  \
         devices, or change any familiar identity.\n",
    );
    out
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn run_recover(envelope_path: &Path, root_key: &Path, assume_yes: bool) -> Result<()> {
    use std::io::{BufRead, Write};

    let home = crate::coven_home_dir()?;
    let bytes = std::fs::read(envelope_path)
        .with_context(|| format!("failed to read {}", envelope_path.display()))?;
    if bytes.len() > super::MAX_MOBILE_REQUEST_BYTES {
        bail!("access recovery envelope is too large");
    }
    let envelope: AccessRecoveryEnvelope =
        serde_json::from_slice(&bytes).context("access recovery envelope is malformed")?;
    let key = super::policy_cli::load_owner_root(&home, root_key)?;
    let authority =
        AccessRecoveryAuthority::load(&home, super::identity::installed_host_fingerprint(&home)?)?;
    if authority.current_policy()?.owner_root_key != super::policy_cli::public_key(&key) {
        bail!("this key is not the installed owner-root credential");
    }
    let action = envelope.request.action()?;
    print!("{}", render_recovery_review(&envelope.request, &action));
    if !assume_yes {
        print!("Type `recover` to sign this exact action with the owner-root key (anything else cancels): ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if line.trim() != "recover" {
            bail!("access recovery cancelled; nothing was signed");
        }
    }
    let signature = super::policy_cli::sign_action(&key, &action)?;
    authority.recover(
        &envelope.request,
        &envelope.destination_signature,
        &signature,
        Utc::now(),
    )?;
    // Recovery enrolls exactly the requested id. Print it from the request so
    // nothing read back from the key-bearing registry record reaches stdout.
    println!(
        "Recovered access for device {}. The event is in the mobile audit log.",
        envelope.request.destination_id
    );
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/fixtures/access-recovery/mod.rs"]
mod tests;
