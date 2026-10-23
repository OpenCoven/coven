//! Local trusted-device enrollment. Only the owner socket admits a request,
//! after the owner reviews every signed field; only the request's named
//! introducer, authenticated over the gateway, may approve it. Every authority
//! decision runs through `IntroductionAuthority`; clients only display and sign.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::assurance::{
    AssuranceChallengeStore, AssuranceContextMode, PresentedAssuranceProof, RequestedAssurance,
};
use super::grant::{DeviceActionIntent, GrantTransportConstraint};
use super::introduction::{
    IntroductionApproval, IntroductionAuthority, IntroductionCommitUncertain, IntroductionRequest,
};
use super::owner_policy::InstalledPolicy;
use super::registry::DeviceRegistry;

pub(super) const LOCAL_INTRODUCTION_ROOT: &str = "/api/v1/internal/mobile/introductions";
const MAX_TRACKED_INTRODUCTIONS: usize = 16;

/// What the new installation hands the owner: the exact request and its
/// possession signature over `devices.introduction.accept`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IntroductionEnvelope {
    pub request: IntroductionRequest,
    pub destination_signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntroductionState {
    AwaitingApproval,
    Introduced,
    Rejected,
    /// Committed or possibly committed; never offered as retry-as-success.
    Uncertain,
    Expired,
    Cancelled,
}

impl IntroductionState {
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::AwaitingApproval)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IntroductionStatus {
    pub id: Uuid,
    pub state: IntroductionState,
    pub expires_at: DateTime<Utc>,
    pub device_id: Option<Uuid>,
}

/// Everything the introducer must render before it signs: the complete request
/// and the exact action its possession and step-up signatures cover.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingIntroductionView {
    pub id: Uuid,
    pub request: IntroductionRequest,
    pub action: DeviceActionIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MobileIntroductionApproval {
    pub possession_signature: String,
    pub step_up: MobileStepUpProof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MobileStepUpProof {
    pub challenge: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub requested_assurance: RequestedAssurance,
    pub signature: String,
}

impl MobileStepUpProof {
    fn into_proof(self) -> Result<PresentedAssuranceProof> {
        let challenge = URL_SAFE_NO_PAD
            .decode(&self.challenge)
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .filter(|bytes| URL_SAFE_NO_PAD.encode(bytes) == self.challenge)
            .context("step-up challenge is not canonical base64url")?;
        Ok(PresentedAssuranceProof {
            context_mode: AssuranceContextMode::Action,
            challenge,
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            requested_assurance: self.requested_assurance,
            signature: self.signature,
        })
    }
}

struct TrackedIntroduction {
    envelope: IntroductionEnvelope,
    state: IntroductionState,
    device_id: Option<Uuid>,
}

impl TrackedIntroduction {
    fn status(&self, id: Uuid) -> IntroductionStatus {
        IntroductionStatus {
            id,
            state: self.state,
            expires_at: self.envelope.request.expires_at,
            device_id: self.device_id,
        }
    }
}

/// In-memory and bounded: a daemon restart drops pending introductions, which
/// must then be re-admitted by the owner. Committed state lives only in the
/// registry, so nothing here is evidence of enrollment.
pub(super) struct IntroductionLedger {
    home: PathBuf,
    audience: [u8; 32],
    registry: Arc<DeviceRegistry>,
    challenges: Arc<AssuranceChallengeStore>,
    entries: Mutex<HashMap<Uuid, TrackedIntroduction>>,
    #[cfg(test)]
    lock_wait_observer: Mutex<Option<std::sync::mpsc::SyncSender<()>>>,
}

impl IntroductionLedger {
    #[cfg(test)]
    pub(super) fn new(home: &Path, audience: [u8; 32]) -> Self {
        Self::with_stores(
            home,
            audience,
            Arc::new(DeviceRegistry::load(home).unwrap()),
            Arc::new(AssuranceChallengeStore::load(home).unwrap()),
        )
    }

    pub(super) fn with_stores(
        home: &Path,
        audience: [u8; 32],
        registry: Arc<DeviceRegistry>,
        challenges: Arc<AssuranceChallengeStore>,
    ) -> Self {
        Self {
            home: home.to_owned(),
            audience,
            registry,
            challenges,
            entries: Mutex::new(HashMap::new()),
            #[cfg(test)]
            lock_wait_observer: Mutex::new(None),
        }
    }

    fn authority(&self) -> Result<IntroductionAuthority> {
        self.registry.ensure_running()?;
        IntroductionAuthority::load_installed_with_stores(
            &self.home,
            self.audience,
            Arc::clone(&self.registry),
            Arc::clone(&self.challenges),
        )
    }

    fn entries(&self) -> Result<std::sync::MutexGuard<'_, HashMap<Uuid, TrackedIntroduction>>> {
        loop {
            self.registry.ensure_running()?;
            match self.entries.try_lock() {
                Ok(entries) => return Ok(entries),
                Err(TryLockError::Poisoned(_)) => bail!("introduction ledger lock was poisoned"),
                Err(TryLockError::WouldBlock) => {
                    #[cfg(test)]
                    if let Some(observer) = self.lock_wait_observer.lock().unwrap().as_ref() {
                        let _ = observer.try_send(());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    /// Owner-socket admission. Policy and audience come from local authority;
    /// the request's own claims are only compared against them.
    pub(super) fn admit(
        &self,
        envelope: IntroductionEnvelope,
        now: DateTime<Utc>,
    ) -> Result<IntroductionStatus> {
        let authority = self.authority()?;
        authority.admit(&envelope.request, &envelope.destination_signature, now)?;
        self.registry.reload()?;
        if self
            .registry
            .device(envelope.request.destination_id)?
            .is_some()
        {
            bail!("introduction destination is already enrolled");
        }
        let mut entries = self.entries()?;
        expire(&mut entries, now);
        if entries
            .values()
            .any(|entry| entry.envelope.request.nonce == envelope.request.nonce)
        {
            bail!("introduction request was already admitted");
        }
        if entries.len() >= MAX_TRACKED_INTRODUCTIONS {
            let oldest_terminal = entries
                .iter()
                .filter(|(_, entry)| entry.state.is_terminal())
                .min_by_key(|(_, entry)| entry.envelope.request.expires_at)
                .map(|(id, _)| *id);
            match oldest_terminal {
                Some(id) => {
                    entries.remove(&id);
                }
                None => bail!("too many introductions are awaiting approval"),
            }
        }
        let id = Uuid::new_v4();
        let entry = TrackedIntroduction {
            envelope,
            state: IntroductionState::AwaitingApproval,
            device_id: None,
        };
        let status = entry.status(id);
        entries.insert(id, entry);
        Ok(status)
    }

    pub(super) fn status(&self, id: Uuid, now: DateTime<Utc>) -> Result<IntroductionStatus> {
        let mut entries = self.entries()?;
        expire(&mut entries, now);
        Ok(entries
            .get(&id)
            .context("introduction not found")?
            .status(id))
    }

    pub(super) fn cancel(&self, id: Uuid, now: DateTime<Utc>) -> Result<IntroductionStatus> {
        let mut entries = self.entries()?;
        expire(&mut entries, now);
        let entry = entries.get_mut(&id).context("introduction not found")?;
        if entry.state == IntroductionState::AwaitingApproval {
            entry.state = IntroductionState::Cancelled;
        }
        Ok(entry.status(id))
    }

    /// Only the named introducer sees a request; other devices see nothing.
    pub(super) fn pending_for(
        &self,
        introducer: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Vec<PendingIntroductionView>> {
        let mut entries = self.entries()?;
        expire(&mut entries, now);
        let mut views = entries
            .iter()
            .filter(|(_, entry)| {
                entry.state == IntroductionState::AwaitingApproval
                    && entry.envelope.request.introducer_id == introducer
            })
            .map(|(id, entry)| {
                Ok(PendingIntroductionView {
                    id: *id,
                    request: entry.envelope.request.clone(),
                    action: entry.envelope.request.action()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        views.sort_by_key(|view| view.request.issued_at);
        Ok(views)
    }

    /// One attempt per admitted request: any failure is terminal, so a bad
    /// or replayed approval can never be retried into a success.
    pub(super) fn approve(
        &self,
        id: Uuid,
        introducer: Uuid,
        approval: MobileIntroductionApproval,
        now: DateTime<Utc>,
    ) -> Result<IntroductionStatus> {
        let mut entries = self.entries()?;
        expire(&mut entries, now);
        let entry = entries
            .get_mut(&id)
            .filter(|entry| entry.envelope.request.introducer_id == introducer)
            .context("introduction not found")?;
        if entry.state != IntroductionState::AwaitingApproval {
            bail!("introduction is no longer awaiting approval");
        }
        let outcome = (|| {
            let approval = IntroductionApproval {
                possession_signature: approval.possession_signature,
                step_up: approval.step_up.into_proof()?,
            };
            let authority = self.authority()?;
            authority.introduce(
                &entry.envelope.request,
                &entry.envelope.destination_signature,
                &approval,
                now,
            )
        })();
        match outcome {
            Ok(record) => {
                entry.state = IntroductionState::Introduced;
                entry.device_id = Some(record.device.id);
                Ok(entry.status(id))
            }
            Err(error) => {
                let destination = entry.envelope.request.destination_id;
                let committed = self
                    .registry
                    .reload()
                    .and_then(|()| self.registry.device(destination))
                    .map_or(true, |device| device.is_some());
                entry.state = if committed || error.is::<IntroductionCommitUncertain>() {
                    IntroductionState::Uncertain
                } else {
                    IntroductionState::Rejected
                };
                Err(error.context(format!("introduction {id} is {:?}", entry.state)))
            }
        }
    }
}

fn expire(entries: &mut HashMap<Uuid, TrackedIntroduction>, now: DateTime<Utc>) {
    for entry in entries.values_mut() {
        if entry.state == IntroductionState::AwaitingApproval
            && now >= entry.envelope.request.expires_at
        {
            entry.state = IntroductionState::Expired;
        }
    }
}

/// Every material field of the request, in binding order. The owner admits
/// nothing until they have seen exactly what the introducer will sign.
// Outside tests only the Unix owner-socket CLI renders this review.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn render_introduction_review(
    request: &IntroductionRequest,
    action: &DeviceActionIntent,
    policy: &InstalledPolicy,
) -> String {
    let grant = &request.grant;
    let scopes = |list: &[super::grant::DeviceScope]| {
        if list.is_empty() {
            "(none)".to_owned()
        } else {
            list.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let mut out = String::from("Review the exact trusted-device introduction:\n");
    let mut field = |name: &str, value: String| out.push_str(&format!("  {name:<32} {value}\n"));
    field("operation", action.operation.clone());
    field("installation (audience)", action.target.clone());
    field(
        "owner policy",
        format!("revision {} ({})", policy.revision, request.policy_digest),
    );
    field("introducer device", request.introducer_id.to_string());
    field("introducer grant", request.introducer_grant_id.to_string());
    field(
        "introducer revocation epoch",
        request.introducer_revocation_epoch.to_string(),
    );
    field("new device id", request.destination_id.to_string());
    field(
        "new device key",
        format!(
            "sha256:{}",
            URL_SAFE_NO_PAD
                .encode(&Sha256::digest(request.destination_public_key.as_bytes())[..12])
        ),
    );
    field("new device name", request.device_name.clone());
    field("new device context", request.device_context.clone());
    field("grant id", grant.id.to_string());
    field("grant scopes", scopes(&grant.scopes));
    field(
        "grant minimum assurance",
        serde_json::to_value(grant.minimum_assurance)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default(),
    );
    field(
        "fresh verification required for",
        scopes(&grant.restrictions.require_fresh_user_verification_for),
    );
    field(
        "grant transport",
        match grant.restrictions.transport {
            GrantTransportConstraint::AnyAuthenticated => "any authenticated".into(),
            GrantTransportConstraint::DirectOnly => "direct only".into(),
        },
    );
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
        "  The context is display material, not device-identity evidence. The introducer must\n  \
         still approve with a fresh step-up on its own device.\n",
    );
    out
}

/// Owner CLI: review the envelope, admit it over the owner socket, then wait
/// for the introducer's approval. The CLI decides nothing; the daemon does.
#[cfg(unix)]
pub fn run_introduce(envelope_path: &Path, assume_yes: bool) -> Result<()> {
    use std::io::{BufRead, Write};

    let home = crate::coven_home_dir()?;
    let bytes = std::fs::read(envelope_path)
        .with_context(|| format!("failed to read {}", envelope_path.display()))?;
    if bytes.len() > super::MAX_MOBILE_REQUEST_BYTES {
        bail!("introduction envelope is too large");
    }
    let envelope: IntroductionEnvelope =
        serde_json::from_slice(&bytes).context("introduction envelope is malformed")?;
    let audience = super::identity::installed_host_fingerprint(&home)?;
    let policy = super::owner_policy::OwnerPolicyStore::open(&home, audience)?
        .current()?
        .context("introduction policy is not installed")?;
    let review =
        render_introduction_review(&envelope.request, &envelope.request.action()?, &policy);
    print!("{review}");
    if !assume_yes {
        print!("Type `introduce` to send this request to the introducer (anything else cancels): ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if line.trim() != "introduce" {
            bail!("introduction cancelled; nothing was admitted");
        }
    }
    let (status, body) = super::post_mobile_control(
        &home,
        LOCAL_INTRODUCTION_ROOT,
        &serde_json::to_string(&envelope)?,
    )?;
    if status != 201 {
        bail!("Coven daemon refused the introduction with HTTP {status}: {body}");
    }
    let mut current: IntroductionStatus =
        serde_json::from_str(&body).context("daemon returned an invalid introduction status")?;
    println!(
        "Waiting for device {} to approve with a fresh step-up (expires {}).",
        envelope.request.introducer_id,
        timestamp(current.expires_at)
    );
    let status_path = format!("{LOCAL_INTRODUCTION_ROOT}/{}/status", current.id);
    while !current.state.is_terminal() {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let (status, body) = super::post_mobile_control(&home, &status_path, "{}")?;
        if status != 200 {
            bail!("introduction status unavailable (HTTP {status}); check `coven device list`");
        }
        current = serde_json::from_str(&body)?;
    }
    match current.state {
        IntroductionState::Introduced => {
            println!(
                "Introduced device {}. The event is in the mobile audit log.",
                current.device_id.unwrap_or(envelope.request.destination_id)
            );
            Ok(())
        }
        IntroductionState::Uncertain => bail!(
            "introduction outcome is uncertain: it may have committed. Inspect `coven device list` \
             and do not resubmit as if it failed"
        ),
        state => bail!("introduction ended as {state:?}; nothing was enrolled"),
    }
}

#[cfg(not(unix))]
pub fn run_introduce(_envelope_path: &Path, _assume_yes: bool) -> Result<()> {
    bail!("trusted-device introduction requires the Unix owner socket")
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
#[path = "../../tests/fixtures/trusted-device-introduction/enrollment.rs"]
pub(super) mod tests;
