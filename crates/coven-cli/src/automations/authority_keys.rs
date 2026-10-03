//! Role-separated Ed25519 keys for Runtime Authority (coven#857, slice 1).
//!
//! The maintainer decisions in
//! `docs/architecture/coven-automations-runtime-authority.md` make the local
//! daemon the trust root. It signs four kinds of artifact, each with its own
//! key, so one can be rotated or revoked without touching the others:
//!
//! - the execution binding at dispatch, and the receipt-correlated authority
//!   evidence at settlement (`dispatch-authority`);
//! - familiar embodiment bindings (`familiar-binding`);
//! - Threads automation-authority decisions (`threads-decision`);
//! - runtime terminal observations (`terminal-observer`).
//!
//! Each key's public half, `keyId`, `proofRef`, producer and validity window
//! are recorded in the store, and [`trusted_keys`] builds the #1192 verifiers'
//! trust set for one role from those records. Roles are enforced by that
//! scoping, because every role shares the daemon's producer identity. The private half lives in an owner-only file
//! under `COVEN_HOME/authority-keys`. On Unix it is created `0600` inside a
//! `0700` directory. On Windows it inherits the owner-only DACL that the daemon
//! places on `COVEN_HOME`.
//!
//! A role has at most one current key. Rotation closes the current key's window
//! and creates its successor. Revocation makes a key authenticate nothing.
//! Either way the retired private key file is deleted, while the public record
//! stays, so evidence signed earlier can still be checked. A current key whose
//! private file is missing or does not match its record is refused rather than
//! silently replaced: losing a key is an explicit rotation, not a side effect.
//!
//! Nothing here constructs Runtime Authority or signs a production artifact.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::ed25519_trust::{TrustedKey, TrustedKeys};

/// The producer every role key is scoped to: the daemon's authority identity,
/// as receipts and events already name it.
pub(crate) const PRODUCER_COMPONENT: &str = "coven-daemon";
pub(crate) const PRODUCER_INSTANCE_ID: &str = "local-authority";

const KEY_DIRECTORY: &str = "authority-keys";

/// The fixed SubjectPublicKeyInfo prefix of an Ed25519 public key (RFC 8410).
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

pub(crate) const AUTOMATION_AUTHORITY_KEYS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_authority_keys (
        key_id TEXT PRIMARY KEY NOT NULL,
        role TEXT NOT NULL
            CHECK (role IN (
                'dispatch-authority', 'familiar-binding', 'threads-decision', 'terminal-observer'
            )),
        proof_ref TEXT NOT NULL UNIQUE,
        producer_component TEXT NOT NULL,
        producer_instance_id TEXT NOT NULL,
        public_key_der_hex TEXT NOT NULL UNIQUE,
        valid_from TEXT NOT NULL,
        valid_until TEXT,
        revoked_at TEXT,
        revocation_reason TEXT,
        created_at TEXT NOT NULL,
        CHECK (valid_until IS NULL OR valid_until > valid_from),
        CHECK ((revoked_at IS NULL) = (revocation_reason IS NULL))
    );

    -- At most one current key per role: open-ended and not revoked.
    CREATE UNIQUE INDEX IF NOT EXISTS idx_automation_authority_keys_current
        ON automation_authority_keys(role)
        WHERE valid_until IS NULL AND revoked_at IS NULL;
";

/// What a key signs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AuthorityKeyRole {
    DispatchAuthority,
    FamiliarBinding,
    ThreadsDecision,
    TerminalObserver,
}

impl AuthorityKeyRole {
    pub(crate) const ALL: [Self; 4] = [
        Self::DispatchAuthority,
        Self::FamiliarBinding,
        Self::ThreadsDecision,
        Self::TerminalObserver,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DispatchAuthority => "dispatch-authority",
            Self::FamiliarBinding => "familiar-binding",
            Self::ThreadsDecision => "threads-decision",
            Self::TerminalObserver => "terminal-observer",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|role| role.as_str() == value)
            .with_context(|| format!("unknown authority key role `{value}`"))
    }
}

/// One key's public record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorityKeyRecord {
    pub key_id: String,
    pub role: AuthorityKeyRole,
    pub proof_ref: String,
    pub producer_component: String,
    pub producer_instance_id: String,
    pub public_key_der_hex: String,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revocation_reason: Option<String>,
}

/// A role's current key, able to sign.
pub(crate) struct RoleSigningKey {
    record: AuthorityKeyRecord,
    key_pair: Ed25519KeyPair,
}

impl std::fmt::Debug for RoleSigningKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print private key material.
        formatter
            .debug_struct("RoleSigningKey")
            .field("record", &self.record)
            .finish_non_exhaustive()
    }
}

impl RoleSigningKey {
    pub(crate) fn record(&self) -> &AuthorityKeyRecord {
        &self.record
    }

    /// Signs the raw 32 bytes of a contract `signedDigest`, which is how the
    /// authority and terminal-evidence contracts are authenticated, and returns
    /// the 128-character lowercase hex signature they carry.
    pub(crate) fn sign_digest(&self, signed_digest: &[u8; 32]) -> String {
        encode_lower_hex(self.key_pair.sign(signed_digest).as_ref())
    }
}

pub(crate) fn ensure_authority_keys_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(AUTOMATION_AUTHORITY_KEYS_SCHEMA_SQL)
        .context("failed to initialize automation authority keys schema")
}

/// The role's current key, created on first use.
pub(crate) fn current_signing_key(
    conn: &Connection,
    coven_home: &Path,
    role: AuthorityKeyRole,
    now: DateTime<Utc>,
) -> Result<RoleSigningKey> {
    let now = persisted(now)?;
    ensure_authority_keys_schema(conn)?;
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .context("failed to begin authority key transaction")?;
    let (key, created) = match current_record(&transaction, role)? {
        Some(record) => (load_signing_key(coven_home, record)?, false),
        None => (
            create_signing_key(&transaction, coven_home, role, now)?,
            true,
        ),
    };
    commit_or_discard(transaction, coven_home, &key, created)?;
    Ok(key)
}

/// Commits a key-creating transaction. A new key whose record did not commit
/// takes its private file with it, so no key exists without a record.
fn commit_or_discard(
    transaction: rusqlite::Transaction<'_>,
    coven_home: &Path,
    key: &RoleSigningKey,
    created: bool,
) -> Result<()> {
    let committed = transaction
        .commit()
        .context("failed to commit authority key");
    if committed.is_err() && created {
        let _ = remove_private_key(coven_home, &key.record().key_id);
    }
    committed
}

/// Closes the role's current key at `now` and creates its successor. Every
/// retired or revoked private key of the role is then deleted, including any
/// an earlier, interrupted rotation left behind.
pub(crate) fn rotate_signing_key(
    conn: &Connection,
    coven_home: &Path,
    role: AuthorityKeyRole,
    now: DateTime<Utc>,
) -> Result<RoleSigningKey> {
    let now = persisted(now)?;
    ensure_authority_keys_schema(conn)?;
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .context("failed to begin authority key rotation")?;
    let retired = current_record(&transaction, role)?;
    if let Some(record) = &retired {
        ensure!(
            now > record.valid_from,
            "authority key `{}` cannot be rotated before it becomes valid",
            record.key_id
        );
        transaction
            .execute(
                "UPDATE automation_authority_keys SET valid_until = ?2 WHERE key_id = ?1",
                params![record.key_id, timestamp(now)],
            )
            .context("failed to close the rotated authority key")?;
    }
    let key = create_signing_key(&transaction, coven_home, role, now)?;
    commit_or_discard(transaction, coven_home, &key, true)?;
    remove_retired_private_keys(conn, coven_home, role)?;
    Ok(key)
}

/// Deletes the private file of every retired or revoked key of `role`. It is
/// idempotent, so a cleanup that failed part-way finishes on the next call.
fn remove_retired_private_keys(
    conn: &Connection,
    coven_home: &Path,
    role: AuthorityKeyRole,
) -> Result<()> {
    let mut first_error = None;
    for record in key_records(conn)? {
        let retired = record.valid_until.is_some() || record.revoked_at.is_some();
        if record.role == role && retired {
            if let Err(error) = remove_private_key(coven_home, &record.key_id) {
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Revokes one key so it authenticates nothing, whenever evidence claims it
/// was signed. Returns false if it was already revoked.
pub(crate) fn revoke_key(
    conn: &Connection,
    coven_home: &Path,
    key_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    ensure!(
        !reason.trim().is_empty(),
        "an authority key revocation needs a reason"
    );
    let now = persisted(now)?;
    ensure_authority_keys_schema(conn)?;
    let changed = conn
        .execute(
            "UPDATE automation_authority_keys
             SET revoked_at = ?2, revocation_reason = ?3
             WHERE key_id = ?1 AND revoked_at IS NULL",
            params![key_id, timestamp(now), reason],
        )
        .context("failed to revoke authority key")?;
    if changed == 0 {
        ensure!(
            record_by_id(conn, key_id)?.is_some(),
            "authority key `{key_id}` does not exist"
        );
    }
    // Also on a repeat, so a deletion that failed the first time is retried.
    remove_private_key(coven_home, key_id)?;
    Ok(changed > 0)
}

/// Every recorded key, oldest first.
pub(crate) fn key_records(conn: &Connection) -> Result<Vec<AuthorityKeyRecord>> {
    ensure_authority_keys_schema(conn)?;
    let mut statement = conn
        .prepare(&format!(
            "SELECT {RECORD_COLUMNS} FROM automation_authority_keys
             ORDER BY valid_from, key_id"
        ))
        .context("failed to prepare authority key query")?;
    let rows = statement
        .query_map([], record_from_row)
        .context("failed to read authority keys")?;
    rows.map(|row| {
        row.context("failed to read authority key")
            .and_then(|record| record)
    })
    .collect()
}

/// The trust set for evidence that `role` signs: that role's keys only,
/// scoped to their producer, inside their windows, and revoked where recorded.
/// Every role shares one producer identity, so a verifier must be given the
/// set for the role that signs what it checks. Runtime terminal evidence is
/// checked against `TerminalObserver`. Execution bindings and receipt
/// authority evidence are checked against `DispatchAuthority`.
pub(crate) fn trusted_keys(conn: &Connection, role: AuthorityKeyRole) -> Result<TrustedKeys> {
    let mut keys = TrustedKeys::default();
    for record in key_records(conn)?
        .into_iter()
        .filter(|record| record.role == role)
    {
        let key = TrustedKey::from_spki_der_hex(
            &record.key_id,
            &record.proof_ref,
            &record.public_key_der_hex,
            record.valid_from,
            record.valid_until,
        )
        .map_err(|error| {
            anyhow::anyhow!("authority key `{}` is invalid: {error:?}", record.key_id)
        })?
        .for_producer(&record.producer_component, &record.producer_instance_id);
        let key = if record.revoked_at.is_some() {
            key.revoked()
        } else {
            key
        };
        keys.insert(key).map_err(|error| {
            anyhow::anyhow!("authority key `{}` is duplicated: {error:?}", record.key_id)
        })?;
    }
    Ok(keys)
}

const RECORD_COLUMNS: &str = "key_id, role, proof_ref, producer_component, producer_instance_id,
    public_key_der_hex, valid_from, valid_until, revoked_at, revocation_reason";

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<AuthorityKeyRecord>> {
    let role: String = row.get(1)?;
    let valid_from: String = row.get(6)?;
    let valid_until: Option<String> = row.get(7)?;
    let revoked_at: Option<String> = row.get(8)?;
    let parsed = (|| {
        Ok(AuthorityKeyRecord {
            key_id: row.get(0)?,
            role: AuthorityKeyRole::parse(&role)?,
            proof_ref: row.get(2)?,
            producer_component: row.get(3)?,
            producer_instance_id: row.get(4)?,
            public_key_der_hex: row.get(5)?,
            valid_from: parse_timestamp(&valid_from)?,
            valid_until: valid_until.as_deref().map(parse_timestamp).transpose()?,
            revoked_at: revoked_at.as_deref().map(parse_timestamp).transpose()?,
            revocation_reason: row.get(9)?,
        })
    })();
    Ok(parsed)
}

fn current_record(conn: &Connection, role: AuthorityKeyRole) -> Result<Option<AuthorityKeyRecord>> {
    conn.query_row(
        &format!(
            "SELECT {RECORD_COLUMNS} FROM automation_authority_keys
             WHERE role = ?1 AND valid_until IS NULL AND revoked_at IS NULL"
        ),
        [role.as_str()],
        record_from_row,
    )
    .optional()
    .context("failed to read the current authority key")?
    .transpose()
}

fn record_by_id(conn: &Connection, key_id: &str) -> Result<Option<AuthorityKeyRecord>> {
    conn.query_row(
        &format!("SELECT {RECORD_COLUMNS} FROM automation_authority_keys WHERE key_id = ?1"),
        [key_id],
        record_from_row,
    )
    .optional()
    .context("failed to read authority key")?
    .transpose()
}

fn create_signing_key(
    conn: &Connection,
    coven_home: &Path,
    role: AuthorityKeyRole,
    now: DateTime<Utc>,
) -> Result<RoleSigningKey> {
    let random = SystemRandom::new();
    let mut suffix = [0_u8; 16];
    random
        .fill(&mut suffix)
        .map_err(|_| anyhow::anyhow!("failed to draw an authority key identifier"))?;
    let suffix = encode_lower_hex(&suffix);
    let key_id = format!("coven-local:{}:{suffix}", role.as_str());
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&random)
        .map_err(|_| anyhow::anyhow!("failed to generate an authority key"))?;
    let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| anyhow::anyhow!("failed to load the generated authority key"))?;
    let record = AuthorityKeyRecord {
        proof_ref: format!("coven-local:key-record:{suffix}"),
        role,
        producer_component: PRODUCER_COMPONENT.to_owned(),
        producer_instance_id: PRODUCER_INSTANCE_ID.to_owned(),
        public_key_der_hex: public_key_der_hex(&key_pair),
        valid_from: now,
        valid_until: None,
        revoked_at: None,
        revocation_reason: None,
        key_id,
    };
    // The private key is written before its record, so a recorded current key
    // always has one. A record that fails to commit takes its file with it.
    let path = write_private_key(coven_home, &record.key_id, pkcs8.as_ref())?;
    let inserted = conn
        .execute(
            "INSERT INTO automation_authority_keys
                (key_id, role, proof_ref, producer_component, producer_instance_id,
                 public_key_der_hex, valid_from, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                record.key_id,
                role.as_str(),
                record.proof_ref,
                record.producer_component,
                record.producer_instance_id,
                record.public_key_der_hex,
                timestamp(now),
            ],
        )
        .context("failed to record authority key");
    if let Err(error) = inserted {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(RoleSigningKey { record, key_pair })
}

fn load_signing_key(coven_home: &Path, record: AuthorityKeyRecord) -> Result<RoleSigningKey> {
    let path = private_key_path(coven_home, &record.key_id)?;
    let encoded = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "authority key `{}` has no private key; rotate the {} key",
            record.key_id,
            record.role.as_str()
        )
    })?;
    let pkcs8 = decode_lower_hex(encoded.trim()).with_context(|| {
        format!(
            "authority key `{}` private key is unreadable",
            record.key_id
        )
    })?;
    let key_pair = Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map_err(|_| anyhow::anyhow!("authority key `{}` private key is invalid", record.key_id))?;
    if public_key_der_hex(&key_pair) != record.public_key_der_hex {
        bail!(
            "authority key `{}` private key does not match its record; rotate the {} key",
            record.key_id,
            record.role.as_str()
        );
    }
    Ok(RoleSigningKey { record, key_pair })
}

fn private_key_path(coven_home: &Path, key_id: &str) -> Result<PathBuf> {
    // `coven-local:<role>:<suffix>` becomes `<role>-<suffix>.key`, so the file
    // name never carries a separator from the identifier.
    let mut parts = key_id.splitn(3, ':');
    let (Some("coven-local"), Some(role), Some(suffix)) =
        (parts.next(), parts.next(), parts.next())
    else {
        bail!("authority key `{key_id}` is not a local key");
    };
    ensure!(
        AuthorityKeyRole::parse(role).is_ok()
            && suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "authority key `{key_id}` is not a local key"
    );
    Ok(coven_home
        .join(KEY_DIRECTORY)
        .join(format!("{role}-{suffix}.key")))
}

fn write_private_key(coven_home: &Path, key_id: &str, pkcs8: &[u8]) -> Result<PathBuf> {
    let directory = coven_home.join(KEY_DIRECTORY);
    create_private_directory(&directory)?;
    let path = private_key_path(coven_home, key_id)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("failed to create authority key file {}", path.display()))?;
    let persisted = (|| -> Result<()> {
        file.write_all(format!("{}\n", encode_lower_hex(pkcs8)).as_bytes())
            .and_then(|()| file.sync_all())
            .context("failed to write authority key file")?;
        #[cfg(unix)]
        std::fs::File::open(&directory)
            .and_then(|directory| directory.sync_all())
            .context("failed to persist the authority key directory")?;
        Ok(())
    })();
    // Any failure after the file exists removes it, so no key outlives a
    // write that did not complete.
    if let Err(error) = persisted {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

fn create_private_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(directory)
            .context("failed to create the authority key directory")?;
        // An existing directory may predate this code; keep it owner-only.
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .context("failed to protect the authority key directory")?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(directory).context("failed to create the authority key directory")?;
    Ok(())
}

fn remove_private_key(coven_home: &Path, key_id: &str) -> Result<()> {
    let path = private_key_path(coven_home, key_id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to delete retired authority key {}", path.display())),
    }
}

fn public_key_der_hex(key_pair: &Ed25519KeyPair) -> String {
    let mut der = ED25519_SPKI_PREFIX.to_vec();
    der.extend_from_slice(key_pair.public_key().as_ref());
    encode_lower_hex(&der)
}

/// The store keeps millisecond timestamps. Lifecycle times are reduced to that
/// precision first, so returned records equal the persisted ones and window
/// comparisons agree with the stored `CHECK`.
fn persisted(value: DateTime<Utc>) -> Result<DateTime<Utc>> {
    use chrono::DurationRound;
    value
        .duration_trunc(chrono::TimeDelta::milliseconds(1))
        .context("authority key time cannot be stored")
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("authority key timestamp `{value}` is invalid"))?
        .with_timezone(&Utc))
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_lower_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::ed25519_trust::KeyRefusal;
    use chrono::TimeZone;

    fn store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let conn = Connection::open(temp.path().join("store.sqlite")).unwrap();
        (temp, conn)
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 3, hour, 0, 0).unwrap()
    }

    fn digest(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    fn authenticate(
        keys: &TrustedKeys,
        key: &RoleSigningKey,
        signed_at: DateTime<Utc>,
        seed: u8,
    ) -> Result<(), KeyRefusal> {
        keys.authenticate(
            &key.record().key_id,
            &key.record().proof_ref,
            (PRODUCER_COMPONENT, PRODUCER_INSTANCE_ID),
            signed_at,
            &encode_lower_hex(&digest(seed)),
            &key.sign_digest(&digest(seed)),
        )
    }

    fn opaque(value: &str) -> bool {
        let mut bytes = value.bytes();
        value.len() <= 256
            && bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || b"._:@-".contains(&byte))
    }

    #[test]
    fn each_role_gets_one_persistent_key_that_the_trust_set_authenticates() {
        let (temp, conn) = store();
        let mut seen = Vec::new();
        for role in AuthorityKeyRole::ALL {
            let key = current_signing_key(&conn, temp.path(), role, at(9)).unwrap();
            let again = current_signing_key(&conn, temp.path(), role, at(10)).unwrap();
            assert_eq!(again.record(), key.record(), "{role:?}");
            assert!(opaque(&key.record().key_id) && opaque(&key.record().proof_ref));
            assert_eq!(key.sign_digest(&digest(1)).len(), 128);
            seen.push(key);
        }
        let mut distinct: Vec<_> = seen
            .iter()
            .map(|key| &key.record().public_key_der_hex)
            .collect();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 4, "each role has its own key");

        for key in &seen {
            let keys = trusted_keys(&conn, key.record().role).unwrap();
            assert_eq!(authenticate(&keys, key, at(11), 7), Ok(()));
            // A key authenticates only its own producer.
            assert_eq!(
                keys.authenticate(
                    &key.record().key_id,
                    &key.record().proof_ref,
                    ("other-component", PRODUCER_INSTANCE_ID),
                    at(11),
                    &encode_lower_hex(&digest(7)),
                    &key.sign_digest(&digest(7)),
                ),
                Err(KeyRefusal::Untrusted)
            );
            // A signature never vouches for another digest.
            assert_eq!(
                keys.authenticate(
                    &key.record().key_id,
                    &key.record().proof_ref,
                    (PRODUCER_COMPONENT, PRODUCER_INSTANCE_ID),
                    at(11),
                    &encode_lower_hex(&digest(8)),
                    &key.sign_digest(&digest(7)),
                ),
                Err(KeyRefusal::Invalid)
            );
        }
        // Another role's key cannot answer for this role's identifier.
        let keys = trusted_keys(&conn, seen[0].record().role).unwrap();
        assert_eq!(
            keys.authenticate(
                &seen[0].record().key_id,
                &seen[0].record().proof_ref,
                (PRODUCER_COMPONENT, PRODUCER_INSTANCE_ID),
                at(11),
                &encode_lower_hex(&digest(7)),
                &seen[1].sign_digest(&digest(7)),
            ),
            Err(KeyRefusal::Invalid)
        );
    }

    #[test]
    fn a_trust_set_authenticates_only_its_own_role() {
        let (temp, conn) = store();
        let keys: Vec<_> = AuthorityKeyRole::ALL
            .into_iter()
            .map(|role| current_signing_key(&conn, temp.path(), role, at(9)).unwrap())
            .collect();
        for verifier in AuthorityKeyRole::ALL {
            let trusted = trusted_keys(&conn, verifier).unwrap();
            for signer in &keys {
                // The signer's own identifiers and a valid signature: only the
                // role the verifier checks for may pass.
                let expected = if signer.record().role == verifier {
                    Ok(())
                } else {
                    Err(KeyRefusal::Untrusted)
                };
                assert_eq!(
                    authenticate(&trusted, signer, at(10), 5),
                    expected,
                    "{:?} evidence checked as {verifier:?}",
                    signer.record().role
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn private_keys_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (temp, conn) = store();
        let key = current_signing_key(&conn, temp.path(), AuthorityKeyRole::ThreadsDecision, at(9))
            .unwrap();
        let directory = temp.path().join(KEY_DIRECTORY);
        let path = private_key_path(temp.path(), &key.record().key_id).unwrap();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!format!("{key:?}").contains(&std::fs::read_to_string(&path).unwrap().trim()[..32]));
    }

    #[test]
    fn rotation_retires_the_old_key_after_its_window() {
        let (temp, conn) = store();
        let role = AuthorityKeyRole::TerminalObserver;
        let old = current_signing_key(&conn, temp.path(), role, at(9)).unwrap();
        let old_path = private_key_path(temp.path(), &old.record().key_id).unwrap();
        let new = rotate_signing_key(&conn, temp.path(), role, at(12)).unwrap();
        assert_ne!(new.record().key_id, old.record().key_id);
        assert_eq!(
            current_signing_key(&conn, temp.path(), role, at(13))
                .unwrap()
                .record(),
            new.record()
        );
        assert!(!old_path.exists(), "a retired key can no longer sign");

        let keys = trusted_keys(&conn, role).unwrap();
        // Evidence the old key signed inside its window still verifies.
        assert_eq!(authenticate(&keys, &old, at(11), 3), Ok(()));
        assert_eq!(authenticate(&keys, &old, at(12), 3), Err(KeyRefusal::Stale));
        assert_eq!(authenticate(&keys, &new, at(12), 3), Ok(()));
        assert_eq!(authenticate(&keys, &new, at(11), 3), Err(KeyRefusal::Stale));
        assert!(
            rotate_signing_key(&conn, temp.path(), role, at(12)).is_err(),
            "a key cannot be rotated before it becomes valid"
        );
    }

    #[test]
    fn revocation_authenticates_nothing_and_a_fresh_key_follows() {
        let (temp, conn) = store();
        let role = AuthorityKeyRole::FamiliarBinding;
        let key = current_signing_key(&conn, temp.path(), role, at(9)).unwrap();
        assert!(revoke_key(&conn, temp.path(), &key.record().key_id, " ", at(10)).is_err());
        assert!(revoke_key(
            &conn,
            temp.path(),
            &key.record().key_id,
            "suspected exposure",
            at(10)
        )
        .unwrap());
        assert!(!revoke_key(&conn, temp.path(), &key.record().key_id, "again", at(11)).unwrap());
        assert!(revoke_key(
            &conn,
            temp.path(),
            "coven-local:familiar-binding:0123",
            "x",
            at(11)
        )
        .is_err());
        assert!(!private_key_path(temp.path(), &key.record().key_id)
            .unwrap()
            .exists());

        let keys = trusted_keys(&conn, role).unwrap();
        // Revocation is not a window: earlier signatures stop verifying too.
        assert_eq!(authenticate(&keys, &key, at(9), 4), Err(KeyRefusal::Stale));
        let record = key_records(&conn)
            .unwrap()
            .into_iter()
            .find(|record| record.key_id == key.record().key_id)
            .unwrap();
        assert_eq!(
            record.revocation_reason.as_deref(),
            Some("suspected exposure")
        );

        let fresh = current_signing_key(&conn, temp.path(), role, at(12)).unwrap();
        assert_ne!(fresh.record().key_id, key.record().key_id);
        assert_eq!(
            authenticate(&trusted_keys(&conn, role).unwrap(), &fresh, at(12), 4),
            Ok(())
        );
    }

    #[test]
    fn a_missing_or_substituted_private_key_is_refused_not_regenerated() {
        let (temp, conn) = store();
        let role = AuthorityKeyRole::ThreadsDecision;
        let key = current_signing_key(&conn, temp.path(), role, at(9)).unwrap();
        let path = private_key_path(temp.path(), &key.record().key_id).unwrap();

        let other = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        std::fs::write(&path, format!("{}\n", encode_lower_hex(other.as_ref()))).unwrap();
        let substituted = current_signing_key(&conn, temp.path(), role, at(10)).unwrap_err();
        assert!(
            format!("{substituted:#}").contains("does not match its record"),
            "{substituted:#}"
        );

        std::fs::remove_file(&path).unwrap();
        let missing = current_signing_key(&conn, temp.path(), role, at(10)).unwrap_err();
        assert!(
            format!("{missing:#}").contains("has no private key"),
            "{missing:#}"
        );
        assert_eq!(
            key_records(&conn).unwrap().len(),
            1,
            "nothing was silently replaced"
        );

        let rotated = rotate_signing_key(&conn, temp.path(), role, at(11)).unwrap();
        assert_eq!(
            current_signing_key(&conn, temp.path(), role, at(12))
                .unwrap()
                .record(),
            rotated.record()
        );
    }

    #[test]
    fn cleanup_finishes_deletions_that_failed_earlier() {
        let (temp, conn) = store();
        let role = AuthorityKeyRole::TerminalObserver;
        let first = current_signing_key(&conn, temp.path(), role, at(9)).unwrap();
        let first_path = private_key_path(temp.path(), &first.record().key_id).unwrap();
        rotate_signing_key(&conn, temp.path(), role, at(10)).unwrap();
        // As if that rotation committed but failed to delete the retired file.
        std::fs::write(&first_path, "left behind\n").unwrap();
        rotate_signing_key(&conn, temp.path(), role, at(11)).unwrap();
        assert!(
            !first_path.exists(),
            "a later rotation finishes the deletion"
        );

        let revoked = current_signing_key(&conn, temp.path(), role, at(12)).unwrap();
        let revoked_path = private_key_path(temp.path(), &revoked.record().key_id).unwrap();
        assert!(revoke_key(
            &conn,
            temp.path(),
            &revoked.record().key_id,
            "rotated out",
            at(13)
        )
        .unwrap());
        // As if the first revocation committed but failed to delete the file.
        std::fs::write(&revoked_path, "left behind\n").unwrap();
        assert!(!revoke_key(
            &conn,
            temp.path(),
            &revoked.record().key_id,
            "again",
            at(14)
        )
        .unwrap());
        assert!(
            !revoked_path.exists(),
            "a repeated revocation finishes the deletion"
        );
    }

    #[test]
    fn lifecycle_times_are_kept_at_the_stored_precision() {
        let (temp, conn) = store();
        let role = AuthorityKeyRole::DispatchAuthority;
        let created_at = at(9) + chrono::TimeDelta::microseconds(123_100);
        let key = current_signing_key(&conn, temp.path(), role, created_at).unwrap();
        assert_eq!(
            key.record().valid_from,
            at(9) + chrono::TimeDelta::milliseconds(123)
        );
        // The returned record is exactly what a reload reads back.
        assert_eq!(&key_records(&conn).unwrap()[0], key.record());
        // Within the same stored millisecond, rotation is refused cleanly
        // rather than by the store's window CHECK.
        let same_millisecond = at(9) + chrono::TimeDelta::microseconds(123_900);
        let refused = rotate_signing_key(&conn, temp.path(), role, same_millisecond).unwrap_err();
        assert!(
            format!("{refused:#}").contains("cannot be rotated before it becomes valid"),
            "{refused:#}"
        );
        let next_millisecond = at(9) + chrono::TimeDelta::microseconds(124_000);
        let rotated = rotate_signing_key(&conn, temp.path(), role, next_millisecond).unwrap();
        let records = key_records(&conn).unwrap();
        assert_eq!(records[0].valid_until, Some(rotated.record().valid_from));
        assert_eq!(&records[1], rotated.record());
    }

    #[test]
    fn one_current_key_per_role_is_enforced_by_the_store() {
        let (temp, conn) = store();
        current_signing_key(
            &conn,
            temp.path(),
            AuthorityKeyRole::TerminalObserver,
            at(9),
        )
        .unwrap();
        let duplicate = conn.execute(
            "INSERT INTO automation_authority_keys
                (key_id, role, proof_ref, producer_component, producer_instance_id,
                 public_key_der_hex, valid_from, created_at)
             VALUES ('coven-local:terminal-observer:x', 'terminal-observer', 'p', 'c', 'i', 'k',
                     '2026-10-03T09:00:00.000Z', '2026-10-03T09:00:00.000Z')",
            [],
        );
        assert!(duplicate.is_err());
    }
}
