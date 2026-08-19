use serde::Deserialize;
#[cfg(unix)]
use serde::Serialize;

pub const PROTOCOL_VERSION: &str = "coven.daemon.v1";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub ok: bool,
    pub api_version: String,
    pub coven_version: String,
    pub capabilities: HealthCapabilities,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCapabilities {
    #[serde(default)]
    pub sessions: bool,
    #[serde(default)]
    pub events: bool,
    #[serde(default)]
    pub event_cursor: Option<String>,
    #[serde(default)]
    pub structured_errors: bool,
}

#[derive(Clone, Debug)]
pub enum ReadEndpoint {
    Session {
        session_id: String,
    },
    Sessions {
        limit: Option<u16>,
    },
    Events {
        session_id: String,
        after_seq: Option<i64>,
        limit: Option<i64>,
    },
}

#[derive(Clone, Debug)]
pub enum WriteEndpoint {
    Sessions,
    SessionInput { session_id: String },
    SessionKill { session_id: String },
}

#[doc(hidden)]
#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleDaemonStatus {
    pub pid: u32,
    pub started_at: String,
    pub socket: String,
}

#[doc(hidden)]
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnixDaemonShutdown {
    Unavailable,
    IdentityMismatch,
    Exited,
    TimedOut,
}
