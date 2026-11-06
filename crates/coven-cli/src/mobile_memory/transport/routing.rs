//! Version 2 rendezvous derivation and checked counter transitions.
//!
//! Arithmetic transitions do not authenticate peers or persist state. Callers
//! must verify possession before a transition and durably save its result before
//! emitting the next protocol message. Routing never grants application authority.
use anyhow::{ensure, Result};
use ring::hkdf;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const WINDOW: u64 = 4;

/// V2 peer handshake API; persistence remains owned by the durable state helpers.
pub mod channel;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum Transport {
    Direct,
    Relay,
}

/// This function permits all u64 encodings for fixture/interoperability use.
/// Listener admission and consumption must separately reject overflowing ranges.
pub fn derive(
    seed: &[u8; 32],
    transport: Transport,
    generation: u64,
    counter: u64,
) -> Result<([u8; 32], Zeroizing<[u8; 32]>)> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"coven.mobile.rendezvous.v2");
    let key = salt.extract(seed);
    let name: &[u8] = match transport {
        Transport::Direct => b"direct",
        Transport::Relay => b"relay",
    };
    let derive = |label: &[u8]| -> Result<[u8; 32]> {
        let mut info = Vec::with_capacity(2 + name.len() + label.len() + 16);
        info.push(u8::try_from(name.len())?);
        info.extend_from_slice(name);
        info.push(u8::try_from(label.len())?);
        info.extend_from_slice(label);
        info.extend_from_slice(&generation.to_be_bytes());
        info.extend_from_slice(&counter.to_be_bytes());
        let info = [info.as_slice()];
        let output = key
            .expand(&info, hkdf::HKDF_SHA256)
            .map_err(|_| anyhow::anyhow!("rendezvous derivation failed"))?;
        let mut value = [0; 32];
        output
            .fill(&mut value)
            .map_err(|_| anyhow::anyhow!("rendezvous derivation failed"))?;
        Ok(value)
    };
    Ok((derive(b"room")?, Zeroizing::new(derive(b"credential")?)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostProof {
    pub generation: u64,
    pub host_floor: u64,
    pub counter: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, zeroize::Zeroize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceState {
    pub generation: u64,
    pub next: u64,
    pub floor: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostState {
    pub generation: u64,
    pub floor: u64,
}

impl HostProof {
    fn validate(&self) -> Result<()> {
        let end = self
            .host_floor
            .checked_add(WINDOW)
            .ok_or_else(|| anyhow::anyhow!("routing generation handover required"))?;
        ensure!(
            self.counter >= self.host_floor && self.counter < end,
            "routing room outside authenticated window"
        );
        self.counter
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("routing generation handover required"))?;
        Ok(())
    }
}

impl HostState {
    pub fn proof(&self, counter: u64) -> Result<HostProof> {
        let proof = HostProof {
            generation: self.generation,
            host_floor: self.floor,
            counter,
        };
        proof.validate()?;
        Ok(proof)
    }

    /// Call only under the durable consumption lock after possession and current
    /// registry/grant checks. Persist the returned state before completion.
    pub fn consume(&self, proof: HostProof) -> Result<Self> {
        proof.validate()?;
        ensure!(
            proof.generation == self.generation,
            "routing generation changed"
        );
        ensure!(proof.host_floor <= self.floor, "routing floor regressed");
        self.proof(proof.counter)?;
        Ok(Self {
            generation: self.generation,
            floor: proof
                .counter
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("routing generation handover required"))?,
        })
    }
}
impl DeviceState {
    pub fn room(&self) -> Result<u64> {
        let distance = self
            .next
            .checked_sub(self.floor)
            .ok_or_else(|| anyhow::anyhow!("invalid device routing state"))?;
        ensure!(distance < WINDOW, "direct routing recovery required");
        self.next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("routing generation handover required"))?;
        Ok(self.next)
    }

    /// Call only after verifying the host's signature over this exact proof.
    /// Persist the returned state before sending the device possession proof.
    pub fn consume(&self, proof: HostProof) -> Result<Self> {
        proof.validate()?;
        ensure!(
            proof.generation == self.generation,
            "routing generation changed"
        );
        ensure!(proof.counter == self.room()?, "unexpected routing room");
        ensure!(
            proof.host_floor >= self.floor,
            "authenticated routing floor regressed"
        );
        Ok(Self {
            generation: self.generation,
            next: proof
                .counter
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("routing generation handover required"))?,
            floor: self.floor.max(proof.host_floor),
        })
    }
}

#[cfg(test)]
mod tests;
