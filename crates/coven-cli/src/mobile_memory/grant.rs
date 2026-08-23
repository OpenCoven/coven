use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const DEVICE_GRANT_VERSION: u16 = 1;
pub const DEVICE_ACTION_VERSION: u16 = 1;
const MAX_ACTION_LIFETIME_SECONDS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceScope {
    MemoryRead,
    SessionMetadataRead,
    ConversationRead,
    MessageSend,
    ToolInvocationRequest,
    ToolExecutionApprove,
    SecretsRead,
    FamiliarMemoryAdmin,
    DeviceAdmin,
    IdentityAdmin,
    MemoryExport,
    IdentityExport,
}

impl DeviceScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MemoryRead => "memory_read",
            Self::SessionMetadataRead => "session_metadata_read",
            Self::ConversationRead => "conversation_read",
            Self::MessageSend => "message_send",
            Self::ToolInvocationRequest => "tool_invocation_request",
            Self::ToolExecutionApprove => "tool_execution_approve",
            Self::SecretsRead => "secrets_read",
            Self::FamiliarMemoryAdmin => "familiar_memory_admin",
            Self::DeviceAdmin => "device_admin",
            Self::IdentityAdmin => "identity_admin",
            Self::MemoryExport => "memory_export",
            Self::IdentityExport => "identity_export",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssuranceLevel {
    Possession,
    RecentUserVerification,
    FreshUserVerification,
    FreshBiometric,
    StepUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceGrantAudience {
    LocalCovenAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantTransportConstraint {
    AnyAuthenticated,
    DirectOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceGrantRestrictions {
    pub transport: GrantTransportConstraint,
    pub require_fresh_user_verification_for: Vec<DeviceScope>,
}

impl Default for DeviceGrantRestrictions {
    fn default() -> Self {
        Self {
            transport: GrantTransportConstraint::AnyAuthenticated,
            require_fresh_user_verification_for: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceGrant {
    pub version: u16,
    pub id: Uuid,
    pub subject_key_id: String,
    pub audience: DeviceGrantAudience,
    pub scopes: Vec<DeviceScope>,
    pub restrictions: DeviceGrantRestrictions,
    pub minimum_assurance: AssuranceLevel,
    pub issued_at: DateTime<Utc>,
    pub not_before: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revocation_epoch: u64,
}

impl DeviceGrant {
    pub fn for_device(
        device_id: Uuid,
        public_key_x963: &str,
        scopes: Vec<DeviceScope>,
        issued_at: DateTime<Utc>,
    ) -> Result<Self, GrantError> {
        let grant = Self {
            version: DEVICE_GRANT_VERSION,
            id: Uuid::new_v5(&device_id, b"coven-device-grant-v1"),
            subject_key_id: subject_key_id(public_key_x963)?,
            audience: DeviceGrantAudience::LocalCovenAuthority,
            scopes,
            restrictions: DeviceGrantRestrictions::default(),
            minimum_assurance: AssuranceLevel::Possession,
            issued_at,
            not_before: issued_at,
            expires_at: None,
            revocation_epoch: 0,
        };
        grant.validate(public_key_x963)?;
        Ok(grant)
    }

    pub fn validate(&self, public_key_x963: &str) -> Result<(), GrantError> {
        if self.version != DEVICE_GRANT_VERSION {
            return Err(GrantError::InvalidVersion);
        }
        if self.subject_key_id != subject_key_id(public_key_x963)? {
            return Err(GrantError::SubjectMismatch);
        }
        validate_scope_set(&self.scopes)?;
        validate_scope_set_allow_empty(&self.restrictions.require_fresh_user_verification_for)?;
        if !self
            .restrictions
            .require_fresh_user_verification_for
            .iter()
            .all(|scope| self.scopes.binary_search(scope).is_ok())
        {
            return Err(GrantError::InvalidRestrictions);
        }
        if self.not_before < self.issued_at
            || self
                .expires_at
                .as_ref()
                .is_some_and(|expires_at| expires_at <= &self.not_before)
        {
            return Err(GrantError::InvalidTimeWindow);
        }
        Ok(())
    }

    pub fn authorize(
        &self,
        required_scope: Option<DeviceScope>,
        presented_assurance: AssuranceLevel,
        now: DateTime<Utc>,
    ) -> Result<(), GrantError> {
        if now < self.not_before
            || self
                .expires_at
                .as_ref()
                .is_some_and(|expires_at| &now >= expires_at)
        {
            return Err(GrantError::Inactive);
        }
        let required_assurance = match required_scope {
            Some(scope) => {
                if self.scopes.binary_search(&scope).is_err() {
                    return Err(GrantError::ScopeDenied);
                }
                if self
                    .restrictions
                    .require_fresh_user_verification_for
                    .binary_search(&scope)
                    .is_ok()
                {
                    self.minimum_assurance
                        .max(AssuranceLevel::FreshUserVerification)
                } else {
                    self.minimum_assurance
                }
            }
            None => self.minimum_assurance,
        };
        if presented_assurance < required_assurance {
            return Err(GrantError::AssuranceRequired);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantError {
    InvalidVersion,
    InvalidSubject,
    SubjectMismatch,
    InvalidScopeSet,
    InvalidRestrictions,
    InvalidTimeWindow,
    Inactive,
    ScopeDenied,
    AssuranceRequired,
}

impl fmt::Display for GrantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidVersion => "unsupported device grant version",
            Self::InvalidSubject => "device grant subject key id is invalid",
            Self::SubjectMismatch => "device grant is bound to another key",
            Self::InvalidScopeSet => "device grant scope set is not canonical",
            Self::InvalidRestrictions => "device grant restrictions are invalid",
            Self::InvalidTimeWindow => "device grant time window is invalid",
            Self::Inactive => "device grant is not active",
            Self::ScopeDenied => "device grant does not authorize this scope",
            Self::AssuranceRequired => "device grant requires stronger assurance",
        })
    }
}

impl std::error::Error for GrantError {}

pub fn validate_scope_set(scopes: &[DeviceScope]) -> Result<(), GrantError> {
    if scopes.is_empty() {
        return Err(GrantError::InvalidScopeSet);
    }
    validate_scope_set_allow_empty(scopes)
}

fn validate_scope_set_allow_empty(scopes: &[DeviceScope]) -> Result<(), GrantError> {
    if scopes.windows(2).any(|window| window[0] >= window[1]) {
        return Err(GrantError::InvalidScopeSet);
    }
    Ok(())
}

fn subject_key_id(public_key_x963: &str) -> Result<String, GrantError> {
    let key = URL_SAFE_NO_PAD
        .decode(public_key_x963)
        .map_err(|_| GrantError::InvalidSubject)?;
    if URL_SAFE_NO_PAD.encode(&key) != public_key_x963 {
        return Err(GrantError::InvalidSubject);
    }
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(key)))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceActionIntent {
    pub version: u16,
    pub scope: DeviceScope,
    pub operation: String,
    pub target: String,
    pub effect_digest: String,
    pub nonce: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl DeviceActionIntent {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ActionIntentError> {
        self.validate()?;
        let issued_at = self.issued_at.to_rfc3339_opts(SecondsFormat::Millis, true);
        let expires_at = self.expires_at.to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut encoded = b"COVEN-ACTION/1\0".to_vec();
        for field in [
            self.scope.as_str().as_bytes(),
            self.operation.as_bytes(),
            self.target.as_bytes(),
            self.effect_digest.as_bytes(),
            self.nonce.as_bytes(),
            issued_at.as_bytes(),
            expires_at.as_bytes(),
        ] {
            encoded.extend_from_slice(&(field.len() as u32).to_be_bytes());
            encoded.extend_from_slice(field);
        }
        Ok(encoded)
    }

    fn validate(&self) -> Result<(), ActionIntentError> {
        if self.version != DEVICE_ACTION_VERSION {
            return Err(ActionIntentError::InvalidVersion);
        }
        if self.operation.is_empty()
            || self.operation.len() > 64
            || !self.operation.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(ActionIntentError::InvalidOperation);
        }
        if self.target.is_empty()
            || self.target.trim() != self.target
            || self.target.len() > 512
            || self.target.chars().any(char::is_control)
        {
            return Err(ActionIntentError::InvalidTarget);
        }
        validate_canonical_32_byte_base64url(&self.effect_digest)?;
        validate_canonical_32_byte_base64url(&self.nonce)?;
        let lifetime = self.expires_at.signed_duration_since(self.issued_at);
        if lifetime.num_seconds() <= 0 || lifetime.num_seconds() > MAX_ACTION_LIFETIME_SECONDS {
            return Err(ActionIntentError::InvalidTimeWindow);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionIntentError {
    InvalidVersion,
    InvalidOperation,
    InvalidTarget,
    InvalidDigest,
    InvalidTimeWindow,
}

impl fmt::Display for ActionIntentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidVersion => "unsupported action intent version",
            Self::InvalidOperation => "action intent operation is invalid",
            Self::InvalidTarget => "action intent target is invalid",
            Self::InvalidDigest => "action intent digest or nonce is invalid",
            Self::InvalidTimeWindow => "action intent time window is invalid",
        })
    }
}

impl std::error::Error for ActionIntentError {}

fn validate_canonical_32_byte_base64url(value: &str) -> Result<(), ActionIntentError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ActionIntentError::InvalidDigest)?;
    if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(decoded) != value {
        return Err(ActionIntentError::InvalidDigest);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    fn public_key(seed: u8) -> String {
        let signing_key = p256::SecretKey::from_slice(&[seed; 32]).unwrap();
        URL_SAFE_NO_PAD.encode(signing_key.public_key().to_encoded_point(false).as_bytes())
    }

    fn intent() -> DeviceActionIntent {
        let issued_at = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        DeviceActionIntent {
            version: DEVICE_ACTION_VERSION,
            scope: DeviceScope::ToolExecutionApprove,
            operation: "deploy.production".to_owned(),
            target: "OpenCoven/psyche@synthetic".to_owned(),
            effect_digest: URL_SAFE_NO_PAD.encode([7_u8; 32]),
            nonce: URL_SAFE_NO_PAD.encode([9_u8; 32]),
            issued_at,
            expires_at: issued_at + Duration::seconds(60),
        }
    }

    #[test]
    fn device_grant_is_bound_to_the_enrolled_key() {
        let now = Utc::now();
        let first = public_key(1);
        let second = public_key(2);
        let grant = DeviceGrant::for_device(
            Uuid::from_u128(1),
            &first,
            vec![DeviceScope::MemoryRead],
            now,
        )
        .unwrap();
        grant.validate(&first).unwrap();
        assert_eq!(grant.validate(&second), Err(GrantError::SubjectMismatch));
    }

    #[test]
    fn grant_scope_sets_are_sorted_unique_and_fail_closed() {
        let now = Utc::now();
        let key = public_key(3);
        assert_eq!(
            DeviceGrant::for_device(
                Uuid::from_u128(2),
                &key,
                vec![DeviceScope::MessageSend, DeviceScope::MemoryRead],
                now,
            )
            .unwrap_err(),
            GrantError::InvalidScopeSet
        );
        assert_eq!(
            DeviceGrant::for_device(
                Uuid::from_u128(3),
                &key,
                vec![DeviceScope::MemoryRead, DeviceScope::MemoryRead],
                now,
            )
            .unwrap_err(),
            GrantError::InvalidScopeSet
        );
    }

    #[test]
    fn grant_enforces_scope_time_and_assurance() {
        let now = Utc::now();
        let key = public_key(4);
        let mut grant = DeviceGrant::for_device(
            Uuid::from_u128(4),
            &key,
            vec![DeviceScope::MemoryRead, DeviceScope::ToolExecutionApprove],
            now,
        )
        .unwrap();
        grant.restrictions.require_fresh_user_verification_for =
            vec![DeviceScope::ToolExecutionApprove];
        grant.expires_at = Some(now + Duration::minutes(1));
        grant.validate(&key).unwrap();

        grant
            .authorize(
                Some(DeviceScope::MemoryRead),
                AssuranceLevel::Possession,
                now,
            )
            .unwrap();
        assert_eq!(
            grant.authorize(
                Some(DeviceScope::ToolExecutionApprove),
                AssuranceLevel::Possession,
                now,
            ),
            Err(GrantError::AssuranceRequired)
        );
        assert_eq!(
            grant.authorize(
                Some(DeviceScope::ConversationRead),
                AssuranceLevel::FreshBiometric,
                now,
            ),
            Err(GrantError::ScopeDenied)
        );
        assert_eq!(
            grant.authorize(
                Some(DeviceScope::MemoryRead),
                AssuranceLevel::Possession,
                now + Duration::minutes(1),
            ),
            Err(GrantError::Inactive)
        );
    }

    #[test]
    fn action_intent_canonicalization_binds_every_material_field() {
        let base = intent();
        let canonical = base.canonical_bytes().unwrap();
        let mut changed = base.clone();
        changed.target.push_str("-other");
        assert_ne!(canonical, changed.canonical_bytes().unwrap());
        let mut changed = base.clone();
        changed.effect_digest = URL_SAFE_NO_PAD.encode([8_u8; 32]);
        assert_ne!(canonical, changed.canonical_bytes().unwrap());
        let mut changed = base;
        changed.expires_at += Duration::seconds(1);
        assert_ne!(canonical, changed.canonical_bytes().unwrap());
    }
}
