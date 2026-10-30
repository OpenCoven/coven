//! Canonical digest of proposal authority JSON: a proposal's revision. Objects
//! hash with sorted, re-encoded keys; arrays keep their order; scalars hash as
//! written. Pure: no I/O, policy, or persistence. Input has already passed
//! envelope preflight, so the depth bound only re-checks that guarantee.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::proposal_preflight::PROPOSAL_JSON_MAX_DEPTH;

fn json_value_end(raw: &[u8], start: usize, depth: usize) -> Result<usize> {
    anyhow::ensure!(
        depth <= PROPOSAL_JSON_MAX_DEPTH,
        "canonical proposal JSON exceeded the validated depth"
    );
    let mut offset = start;
    while raw
        .get(offset)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        offset += 1;
    }
    match raw.get(offset).copied() {
        Some(b'"') => {
            offset += 1;
            loop {
                match raw.get(offset).copied() {
                    Some(b'"') => return Ok(offset + 1),
                    Some(b'\\') => {
                        offset += 2;
                    }
                    Some(_) => offset += 1,
                    None => {
                        anyhow::bail!("canonical proposal JSON contains an unterminated string")
                    }
                }
            }
        }
        Some(b'{') => {
            offset += 1;
            loop {
                while raw
                    .get(offset)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    offset += 1;
                }
                if raw.get(offset) == Some(&b'}') {
                    return Ok(offset + 1);
                }
                offset = json_value_end(raw, offset, depth + 1)?;
                while raw
                    .get(offset)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    offset += 1;
                }
                anyhow::ensure!(
                    raw.get(offset) == Some(&b':'),
                    "canonical proposal JSON object is missing `:`"
                );
                offset = json_value_end(raw, offset + 1, depth + 1)?;
                while raw
                    .get(offset)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    offset += 1;
                }
                match raw.get(offset) {
                    Some(b',') => offset += 1,
                    Some(b'}') => return Ok(offset + 1),
                    _ => anyhow::bail!("canonical proposal JSON object is not terminated"),
                }
            }
        }
        Some(b'[') => {
            offset += 1;
            loop {
                while raw
                    .get(offset)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    offset += 1;
                }
                if raw.get(offset) == Some(&b']') {
                    return Ok(offset + 1);
                }
                offset = json_value_end(raw, offset, depth + 1)?;
                while raw
                    .get(offset)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    offset += 1;
                }
                match raw.get(offset) {
                    Some(b',') => offset += 1,
                    Some(b']') => return Ok(offset + 1),
                    _ => anyhow::bail!("canonical proposal JSON array is not terminated"),
                }
            }
        }
        Some(_) => {
            while raw.get(offset).is_some_and(|byte| {
                !byte.is_ascii_whitespace() && !matches!(byte, b',' | b']' | b'}')
            }) {
                offset += 1;
            }
            anyhow::ensure!(
                offset > start,
                "canonical proposal JSON contains an empty scalar"
            );
            Ok(offset)
        }
        None => anyhow::bail!("canonical proposal JSON ended before a value"),
    }
}

pub(super) fn hash_canonical_json(raw: &[u8], hasher: &mut Sha256, depth: usize) -> Result<()> {
    anyhow::ensure!(
        depth <= PROPOSAL_JSON_MAX_DEPTH,
        "canonical proposal JSON exceeded the validated depth"
    );
    let start = raw
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .context("canonical proposal JSON is empty")?;
    let end = raw
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .context("canonical proposal JSON is empty")?;
    let raw = &raw[start..end];
    match raw.first().copied() {
        Some(b'{') => {
            let object: BTreeMap<String, &serde_json::value::RawValue> =
                serde_json::from_slice(raw).context("parsing canonical proposal object")?;
            hasher.update(b"{");
            for (index, (key, value)) in object.into_iter().enumerate() {
                if index != 0 {
                    hasher.update(b",");
                }
                hasher.update(
                    &serde_json::to_vec(&key).context("serializing canonical proposal key")?,
                );
                hasher.update(b":");
                hash_canonical_json(value.get().as_bytes(), hasher, depth + 1)?;
            }
            hasher.update(b"}");
        }
        Some(b'[') => {
            hasher.update(b"[");
            let mut offset = 1;
            let mut index = 0;
            loop {
                while raw
                    .get(offset)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    offset += 1;
                }
                if raw.get(offset) == Some(&b']') {
                    break;
                }
                let value_end = json_value_end(raw, offset, depth + 1)?;
                if index != 0 {
                    hasher.update(b",");
                }
                hash_canonical_json(&raw[offset..value_end], hasher, depth + 1)?;
                index += 1;
                offset = value_end;
                while raw
                    .get(offset)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    offset += 1;
                }
                match raw.get(offset) {
                    Some(b',') => offset += 1,
                    Some(b']') => break,
                    _ => anyhow::bail!("canonical proposal array is not terminated"),
                }
            }
            hasher.update(b"]");
        }
        Some(_) => hasher.update(raw),
        None => anyhow::bail!("canonical proposal JSON is empty"),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
