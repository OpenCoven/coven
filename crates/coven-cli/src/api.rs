use serde::{Deserialize, Serialize};

use crate::daemon::DaemonStatus;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub daemon: Option<DaemonStatus>,
}

pub fn health_response(daemon: Option<DaemonStatus>) -> HealthResponse {
    HealthResponse { ok: true, daemon }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_health_response() {
        let response = health_response(None);

        assert!(response.ok);
        assert_eq!(response.daemon, None);
    }
}
