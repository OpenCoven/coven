//! Owner CLI for the protected introduction policy. The owner-root private key
//! is read transiently from outside `COVEN_HOME`, used to sign one exact action
//! after the owner reviews every signed field, and never persisted by Coven.

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, SecondsFormat, Utc};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::assurance::RequestedAssurance;
use super::grant::{DeviceActionIntent, DeviceScope};
use super::identity::installed_host_fingerprint;
use super::introduction::IntroductionPolicy;
use super::owner_policy::{
    broadened_dimensions, InstalledPolicy, OwnerPolicyStore, PolicyApproval, PolicyProposal,
};

const ROOT_ACTION_SECONDS: i64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyChangeKind {
    Install,
    Replace,
}

impl PolicyChangeKind {
    const fn confirm_word(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Replace => "replace",
        }
    }
}

pub fn run_generate_owner_root(out: &Path) -> Result<()> {
    let home = crate::coven_home_dir()?;
    refuse_inside_home(&home, out)?;
    let key = SigningKey::from_slice(&rand::random::<[u8; 32]>())
        .context("failed to generate owner-root key")?;
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .context("failed to encode owner-root key")?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(out)
        .with_context(|| format!("failed to create {}", out.display()))?;
    file.write_all(pem.as_bytes())?;
    file.sync_all()?;
    let public = public_key(&key);
    println!("Owner-root key written to {}.", out.display());
    println!("Public key fingerprint: {}", key_fingerprint(&public));
    println!(
        "Keep this file offline. Coven stores only the public key; losing the file means policy \
         downgrades and owner recovery require a trust-domain reset."
    );
    Ok(())
}

pub fn run_show(json: bool) -> Result<()> {
    let home = crate::coven_home_dir()?;
    let store = OwnerPolicyStore::open(&home, installed_host_fingerprint(&home)?)?;
    let Some(head) = store.current()? else {
        if json {
            println!("null");
        } else {
            println!("No introduction policy is installed.");
        }
        return Ok(());
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ShownPolicy::from(&head))?
        );
    } else {
        print!("{}", render_head(&head));
    }
    Ok(())
}

pub fn run_root_change(
    kind: PolicyChangeKind,
    root_key: &Path,
    policy: IntroductionPolicy,
    assume_yes: bool,
) -> Result<()> {
    let home = crate::coven_home_dir()?;
    refuse_inside_home(&home, root_key)?;
    let store = OwnerPolicyStore::open(&home, installed_host_fingerprint(&home)?)?;
    let key = load_root_key(root_key)?;
    let mut stdin = std::io::stdin().lock();
    let mut reader = |prompt: &str| -> Result<String> {
        if assume_yes {
            return Ok(kind.confirm_word().to_owned());
        }
        print!("{prompt}");
        std::io::stdout().flush()?;
        let mut line = String::new();
        stdin.read_line(&mut line)?;
        Ok(line.trim().to_owned())
    };
    match apply_root_change(
        &store,
        kind,
        &key,
        policy,
        Utc::now(),
        rand::random(),
        &mut reader,
    )? {
        // Nothing from the signing flow is echoed; `show` reads the stored head.
        Some(_) => {
            println!(
                "Introduction policy committed and audited. Run `coven device \
                 introduction-policy show` to review it."
            );
            Ok(())
        }
        None => bail!("introduction policy change cancelled; nothing was signed"),
    }
}

/// Builds the exact action, shows every signed field, and signs only after
/// the owner types the confirmation word. Returns `None` when cancelled.
pub(super) fn apply_root_change(
    store: &OwnerPolicyStore,
    kind: PolicyChangeKind,
    key: &SigningKey,
    policy: IntroductionPolicy,
    now: DateTime<Utc>,
    nonce: [u8; 32],
    reader: &mut impl FnMut(&str) -> Result<String>,
) -> Result<Option<InstalledPolicy>> {
    let root = public_key(key);
    let current = store.current()?;
    let proposal = PolicyProposal {
        revision: current.as_ref().map_or(1, |head| head.revision + 1),
        previous_digest: current.as_ref().map(|head| head.digest.clone()),
        policy,
        nonce,
        issued_at: now,
        expires_at: now + chrono::Duration::seconds(ROOT_ACTION_SECONDS),
    };
    let action = match (kind, &current) {
        (PolicyChangeKind::Install, None) => store.install_action(&root, &proposal)?,
        (PolicyChangeKind::Install, Some(_)) => bail!("introduction policy is already installed"),
        (PolicyChangeKind::Replace, None) => bail!("introduction policy is not installed"),
        (PolicyChangeKind::Replace, Some(head)) => {
            if head.owner_root_key != root {
                bail!("this key is not the installed owner-root credential");
            }
            store.replace_action(&proposal)?
        }
    };
    let review = render_review(&action, &proposal, &root, current.as_ref());
    let word = kind.confirm_word();
    let answer = reader(&format!(
        "{review}Type `{word}` to sign this exact action with the owner-root key \
         (anything else cancels): "
    ))?;
    if answer != word {
        return Ok(None);
    }
    let signature = sign(key, &action.canonical_bytes()?);
    let head = match kind {
        PolicyChangeKind::Install => store.install(&root, &proposal, &signature, now)?,
        PolicyChangeKind::Replace => {
            store.replace(&proposal, &PolicyApproval::OwnerRoot { signature }, now)?
        }
    };
    Ok(Some(head))
}

/// Every field covered by the signature, in the order the action binds them.
pub(super) fn render_review(
    action: &DeviceActionIntent,
    proposal: &PolicyProposal,
    root: &str,
    current: Option<&InstalledPolicy>,
) -> String {
    let policy = &proposal.policy;
    let mut out = String::from("Review the exact owner-root action:\n");
    let mut field = |name: &str, value: String| out.push_str(&format!("  {name:<28} {value}\n"));
    field("operation", action.operation.clone());
    field("scope", action.scope.as_str().into());
    field("installation (audience)", action.target.clone());
    field("owner-root key", key_fingerprint(root));
    field("revision", proposal.revision.to_string());
    field(
        "previous digest",
        proposal
            .previous_digest
            .clone()
            .unwrap_or_else(|| "(none: first install)".into()),
    );
    field("eligible introducers", join(&policy.eligible_introducers));
    field("required approvals", policy.required_approvals.to_string());
    field(
        "allowed scopes",
        policy
            .allowed_scopes
            .iter()
            .map(|scope| scope.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    );
    field(
        "maximum grant lifetime",
        format!("{} seconds", policy.maximum_grant_lifetime_seconds),
    );
    field(
        "required assurance",
        policy.required_assurance.as_str().into(),
    );
    field(
        "maximum step-up age",
        format!("{} seconds", policy.maximum_step_up_age_seconds),
    );
    field("nonce", action.nonce.clone());
    field("issued at", timestamp(action.issued_at));
    field("expires at", timestamp(action.expires_at));
    field("effect digest", action.effect_digest.clone());
    if let Some(head) = current {
        let broadened = broadened_dimensions(policy, &head.policy);
        out.push_str(&if broadened.is_empty() {
            "  Change keeps or tightens the current policy.\n".to_owned()
        } else {
            format!(
                "  DOWNGRADE: broadens {}. Only the owner-root key can approve this.\n",
                broadened.join(", ")
            )
        });
    }
    out
}

fn render_head(head: &InstalledPolicy) -> String {
    let policy = &head.policy;
    let scopes: Vec<_> = policy.allowed_scopes.iter().map(|s| s.as_str()).collect();
    format!(
        "Introduction policy revision {}\n  digest                {}\n  owner-root key        {}\n  \
         eligible introducers  {}\n  required approvals    {}\n  allowed scopes        {}\n  \
         max grant lifetime    {} seconds\n  required assurance    {}\n  max step-up age       {} \
         seconds\n  committed at          {}\n",
        head.revision,
        head.digest,
        key_fingerprint(&head.owner_root_key),
        join(&policy.eligible_introducers),
        policy.required_approvals,
        scopes.join(", "),
        policy.maximum_grant_lifetime_seconds,
        policy.required_assurance.as_str(),
        policy.maximum_step_up_age_seconds,
        timestamp(head.committed_at),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShownPolicy {
    revision: u64,
    digest: String,
    owner_root_key_fingerprint: String,
    eligible_introducers: Vec<Uuid>,
    required_approvals: u16,
    allowed_scopes: Vec<DeviceScope>,
    maximum_grant_lifetime_seconds: u32,
    required_assurance: RequestedAssurance,
    maximum_step_up_age_seconds: u32,
    committed_at: DateTime<Utc>,
}

impl From<&InstalledPolicy> for ShownPolicy {
    fn from(head: &InstalledPolicy) -> Self {
        Self {
            revision: head.revision,
            digest: head.digest.clone(),
            owner_root_key_fingerprint: key_fingerprint(&head.owner_root_key),
            eligible_introducers: head.policy.eligible_introducers.clone(),
            required_approvals: head.policy.required_approvals,
            allowed_scopes: head.policy.allowed_scopes.clone(),
            maximum_grant_lifetime_seconds: head.policy.maximum_grant_lifetime_seconds,
            required_assurance: head.policy.required_assurance,
            maximum_step_up_age_seconds: head.policy.maximum_step_up_age_seconds,
            committed_at: head.committed_at,
        }
    }
}

/// The root key must never live in `COVEN_HOME`, where it would fall to the
/// same compromise as the state it is meant to protect.
fn refuse_inside_home(home: &Path, path: &Path) -> Result<()> {
    let resolve = |path: &Path| -> PathBuf {
        if let Ok(resolved) = path.canonicalize() {
            return resolved;
        }
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => {
                let parent = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
                parent
                    .canonicalize()
                    .map_or_else(|_| path.to_owned(), |parent| parent.join(name))
            }
            _ => path.to_owned(),
        }
    };
    if resolve(path).starts_with(resolve(home)) {
        bail!("the owner-root key must be kept outside COVEN_HOME");
    }
    Ok(())
}

fn load_root_key(path: &Path) -> Result<SigningKey> {
    let pem = Zeroizing::new(
        std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?,
    );
    SigningKey::from_pkcs8_pem(&pem).context("owner-root key is not a P-256 PKCS#8 PEM key")
}

pub(super) fn public_key(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes())
}

fn key_fingerprint(public_key: &str) -> String {
    let digest = Sha256::digest(public_key.as_bytes());
    format!("sha256:{}", URL_SAFE_NO_PAD.encode(&digest[..12]))
}

fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    let signature: Signature = key.sign(bytes);
    URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
}

fn join(ids: &[Uuid]) -> String {
    ids.iter()
        .map(Uuid::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
#[path = "../../tests/fixtures/owner-introduction-policy/cli.rs"]
mod tests;
