//! Typed owner-local host sync operations. File bytes travel directly to the peer.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostInit {
    pub root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostConnect {
    pub url: String,
    pub token_file: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostResolution {
    Local,
    Remote,
    Merged(PathBuf),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostResolve {
    pub path: String,
    pub resolution: HostResolution,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRun {
    pub id: String,
}

pub enum HostOperation {
    Init(HostInit),
    Connect(HostConnect),
    Sync,
    Result(HostRun),
    Status,
    Watch,
    Resolve(HostResolve),
}
