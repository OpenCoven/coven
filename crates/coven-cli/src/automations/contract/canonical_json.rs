//! RFC 8785 JSON Canonicalization Scheme helpers.

use anyhow::Context;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// RFC 8785 interoperates through ECMAScript's IEEE-754 number model.
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Serialize `value` according to RFC 8785 / JCS.
///
/// `serde_jcs` rejects values, such as NaN and infinity, that JSON and JCS
/// cannot represent.
pub fn canonicalize<T>(value: &T) -> anyhow::Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let json = serde_json::to_value(value).context("value cannot be represented as JSON")?;
    reject_lossy_integers(&json)?;
    serde_jcs::to_vec(value).context("JCS canonicalization failed")
}

/// Serialize after removing every `integrity` member, as prescribed by the
/// published Automations v1 digest recipe.
pub fn canonicalize_without_integrity(value: &Value) -> anyhow::Result<Vec<u8>> {
    let mut covered = value.clone();
    if let Value::Object(object) = &mut covered {
        object.remove("integrity");
    }
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

fn reject_lossy_integers(value: &Value) -> anyhow::Result<()> {
    match value {
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                anyhow::ensure!(
                    unsigned <= MAX_SAFE_INTEGER,
                    "JCS cannot represent integer {unsigned} without IEEE-754 precision loss"
                );
            } else if let Some(signed) = number.as_i64() {
                anyhow::ensure!(
                    signed.unsigned_abs() <= MAX_SAFE_INTEGER,
                    "JCS cannot represent integer {signed} without IEEE-754 precision loss"
                );
            } else if let Some(float) = number.as_f64() {
                anyhow::ensure!(
                    float.is_finite(),
                    "non-finite values must be rejected by serde_json before JCS canonicalization"
                );
                if float.fract() == 0.0 {
                    anyhow::ensure!(
                        float.abs() <= MAX_SAFE_INTEGER as f64,
                        "JCS cannot represent integer {float} without IEEE-754 precision loss"
                    );
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                reject_lossy_integers(child)?;
            }
        }
        Value::Object(object) => {
            for child in object.values() {
                reject_lossy_integers(child)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
    Ok(())
}
