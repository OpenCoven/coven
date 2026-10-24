//! Reconnection admission against the same durable authority as signed requests.
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};

use super::Pairing;
use crate::mobile_memory::{grant::GrantTransportConstraint, registry::DeviceRegistry};

/// This establishes possession-level channel admission only. It does not assert
/// user verification or authorize an application action; the existing request
/// authenticator remains responsible for capability and assurance enforcement.
pub fn check_current(
    registry: &DeviceRegistry,
    pairing: &Pairing,
    direct: bool,
    now: DateTime<Utc>,
) -> Result<()> {
    registry.reload()?;
    let current = registry
        .authorization_record(pairing.device_id)?
        .context("device unavailable")?;
    ensure!(
        current.device.revoked_at.is_none() && current.device.suspended_at.is_none(),
        "device unavailable"
    );
    ensure!(
        current.grant.id == pairing.grant_id
            && current.grant.revocation_epoch == pairing.revocation_epoch,
        "grant changed"
    );
    let key = URL_SAFE_NO_PAD
        .decode(&current.device.public_key_x963)
        .context("invalid stored device key")?;
    ensure!(
        key == pairing.device_key.to_encoded_point(false).as_bytes(),
        "device key changed"
    );
    current
        .grant
        .validate(&current.device.public_key_x963)
        .context("invalid stored grant")?;
    ensure!(
        now >= current.grant.not_before
            && current.grant.expires_at.is_none_or(|expires| now < expires),
        "grant inactive"
    );
    ensure!(
        direct || current.grant.restrictions.transport != GrantTransportConstraint::DirectOnly,
        "grant requires direct transport"
    );
    Ok(())
}
