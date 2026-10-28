//! Install a fail-closed boundary for the legacy `log` facade before WSS use.
//!
//! Tungstenite trace events contain serialized HTTP requests, including bearer
//! credentials; marking HeaderValue sensitive does not redact those bytes.
use anyhow::{ensure, Result};
use std::sync::OnceLock;

struct EndpointLogger;
static LOGGER: EndpointLogger = EndpointLogger;
static INSTALLED: OnceLock<bool> = OnceLock::new();

impl log::Log for EndpointLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("coven_cli::") && metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{}: {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

/// The CLI currently has no other legacy-facade logger. If another owner is
/// introduced, refuse transport startup rather than trusting an unknown filter.
/// Keep structured application diagnostics independent from dependency events.
pub fn initialize() -> Result<()> {
    let installed = *INSTALLED.get_or_init(|| {
        if log::set_logger(&LOGGER).is_err() {
            return false;
        }
        log::set_max_level(log::LevelFilter::Info);
        true
    });
    ensure!(installed, "private transport diagnostics unavailable");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;
    #[test]
    fn dependency_targets_remain_disabled_at_every_level() {
        for target in [
            "tungstenite::handshake::client",
            "tungstenite::protocol",
            "rustls",
            "snow",
        ] {
            for level in [
                log::Level::Error,
                log::Level::Warn,
                log::Level::Info,
                log::Level::Debug,
                log::Level::Trace,
            ] {
                assert!(
                    !LOGGER.enabled(&log::Metadata::builder().target(target).level(level).build())
                );
            }
        }
    }
}
