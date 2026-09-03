//! RFC 8785 JSON Canonicalization Scheme helpers.

use anyhow::Context;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Serialize `value` according to RFC 8785 / JCS.
///
/// `serde_jcs` rejects values, such as NaN and infinity, that JSON and JCS
/// cannot represent.
pub fn canonicalize<T>(value: &T) -> anyhow::Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    serde_jcs::to_vec(value).context("JCS canonicalization failed")
}

/// Serialize after removing every `integrity` member, as prescribed by the
/// published Automations v1 digest recipe.
pub fn canonicalize_without_integrity(value: &Value) -> anyhow::Result<Vec<u8>> {
    let mut covered = value.clone();
    remove_integrity_members(&mut covered);
    canonicalize(&covered)
}

/// Return the lowercase SHA-256 digest of RFC 8785 canonical bytes.
pub fn sha256_digest<T>(value: &T) -> anyhow::Result<String>
where
    T: Serialize + ?Sized,
{
    let canonical = canonicalize(value)?;
    Ok(sha256_hex(&canonical))
}

/// Return a lowercase SHA-256 digest for an already canonical byte sequence.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn remove_integrity_members(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("integrity");
            for child in object.values_mut() {
                remove_integrity_members(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                remove_integrity_members(child);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}
