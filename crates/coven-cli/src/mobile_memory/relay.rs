//! Operator-owned relay configuration. None of these operations enroll a peer.
use super::{
    identity,
    transport::{storage::RelayConfig, websocket::EndpointConfig},
};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use serde::Serialize;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const MAX_PEM_BYTES: usize = 64 * 1024;

fn endpoint(url: &str, certificates: &[PathBuf]) -> Result<EndpointConfig> {
    ensure!(certificates.len() <= 4, "too many relay certificate files");
    let mut endpoint = EndpointConfig {
        url: url.to_owned(),
        ca_certificates: Vec::new(),
    };
    endpoint.parse()?;
    endpoint.url = url::Url::parse(url)?.to_string();
    for path in certificates {
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(path)
            .context("cannot open relay certificate file")?
            .take((MAX_PEM_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .context("cannot read relay certificate file")?;
        ensure!(
            bytes.len() <= MAX_PEM_BYTES,
            "relay certificate file too large"
        );
        let before = endpoint.ca_certificates.len();
        for certificate in CertificateDer::pem_slice_iter(&bytes) {
            let certificate =
                certificate.map_err(|_| anyhow::anyhow!("invalid relay certificate PEM"))?;
            ensure!(
                endpoint.ca_certificates.len() < 4,
                "too many relay certificate authorities"
            );
            endpoint
                .ca_certificates
                .push(URL_SAFE_NO_PAD.encode(certificate));
        }
        ensure!(
            endpoint.ca_certificates.len() > before,
            "relay certificate file contains no certificates"
        );
    }
    endpoint.parse()?;
    Ok(endpoint)
}

fn add(home: &Path, url: &str, certificates: &[PathBuf]) -> Result<()> {
    let endpoint = endpoint(url, certificates)?;
    RelayConfig::update(home, |config| {
        if let Some(existing) = config.endpoints.iter_mut().find(|existing| {
            url::Url::parse(&existing.url).ok() == url::Url::parse(&endpoint.url).ok()
        }) {
            *existing = endpoint;
        } else {
            config.endpoints.push(endpoint);
        }
        Ok(())
    })
}

fn remove(home: &Path, url: &str) -> Result<()> {
    let endpoint = endpoint(url, &[])?;
    RelayConfig::update(home, |config| {
        let count = config.endpoints.len();
        config.endpoints.retain(|existing| {
            url::Url::parse(&existing.url).ok() != url::Url::parse(&endpoint.url).ok()
        });
        ensure!(
            config.endpoints.len() < count,
            "relay endpoint is not configured"
        );
        if config.endpoints.is_empty() {
            config.enabled = false;
        }
        Ok(())
    })
}

fn enable(home: &Path) -> Result<()> {
    identity::load_existing_host_identity(home).context(
        "relay enablement requires an existing host identity; use initial mobile setup first",
    )?;
    RelayConfig::update(home, |config| {
        ensure!(
            !config.endpoints.is_empty(),
            "configure a relay endpoint before enabling access"
        );
        config.enabled = true;
        Ok(())
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    configured: bool,
    enabled: bool,
    endpoint_count: usize,
    custom_trust_endpoint_count: usize,
}

pub fn run_add(url: &str, certificates: &[PathBuf]) -> Result<()> {
    add(&crate::coven_home_dir()?, url, certificates)?;
    println!("Relay endpoint configuration saved. Existing enablement is unchanged.");
    Ok(())
}
pub fn run_remove(url: &str) -> Result<()> {
    remove(&crate::coven_home_dir()?, url)?;
    println!("Relay endpoint removed. Removing the last endpoint disables relay access.");
    Ok(())
}
pub fn run_enable() -> Result<()> {
    enable(&crate::coven_home_dir()?)?;
    println!(
        "Relay enabled in configuration. Restart the Coven daemon to apply initial enablement."
    );
    Ok(())
}
pub fn run_disable() -> Result<()> {
    RelayConfig::disable(&crate::coven_home_dir()?)?;
    println!("Relay disabled in configuration. Running relay connections will close.");
    Ok(())
}
pub fn run_status(json: bool) -> Result<()> {
    let config = RelayConfig::load(&crate::coven_home_dir()?)?;
    let status = Status {
        configured: config.is_some(),
        enabled: config.as_ref().is_some_and(|config| config.enabled),
        endpoint_count: config.as_ref().map_or(0, |config| config.endpoints.len()),
        custom_trust_endpoint_count: config.as_ref().map_or(0, |config| {
            config
                .endpoints
                .iter()
                .filter(|endpoint| !endpoint.ca_certificates.is_empty())
                .count()
        }),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "Relay configuration: {}",
            if status.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!(
            "Endpoints: {} ({} with explicit certificate trust)",
            status.endpoint_count, status.custom_trust_endpoint_count
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_never_enrolls_or_creates_a_host_identity() {
        let home = tempfile::tempdir().unwrap();
        add(home.path(), "wss://relay.example.test/ws", &[]).unwrap();
        assert!(!RelayConfig::load(home.path()).unwrap().unwrap().enabled);
        assert!(enable(home.path()).is_err());
        assert!(!home.path().join("mobile/host-key.pem").exists());
        assert!(!home.path().join("mobile/devices.json").exists());
        let identity = identity::load_or_create_host_identity(home.path(), "192.168.50.1").unwrap();
        let original = std::fs::read(home.path().join("mobile/host-identity.json")).unwrap();
        enable(home.path()).unwrap();
        assert!(RelayConfig::load(home.path()).unwrap().unwrap().enabled);
        assert!(RelayConfig::disable(home.path()).unwrap());
        enable(home.path()).unwrap();
        assert_eq!(
            identity::load_existing_host_identity(home.path())
                .unwrap()
                .public_key_fingerprint,
            identity.public_key_fingerprint
        );
        assert_eq!(
            std::fs::read(home.path().join("mobile/host-identity.json")).unwrap(),
            original
        );
        remove(home.path(), "wss://relay.example.test:443/ws").unwrap();
        let config = RelayConfig::load(home.path()).unwrap().unwrap();
        assert!(!config.enabled && config.endpoints.is_empty());
    }

    #[test]
    fn certificate_errors_and_capacity_do_not_modify_existing_configuration() {
        let home = tempfile::tempdir().unwrap();
        let certificate =
            rcgen::generate_simple_self_signed(vec!["relay.example.test".into()]).unwrap();
        let path = home.path().join("ca.pem");
        std::fs::write(&path, certificate.cert.pem()).unwrap();
        add(
            home.path(),
            "wss://RELAY.example.test:443/ws",
            std::slice::from_ref(&path),
        )
        .unwrap();
        let config = RelayConfig::load(home.path()).unwrap().unwrap();
        assert_eq!(config.endpoints.len(), 1);
        assert_eq!(config.endpoints[0].ca_certificates.len(), 1);
        add(
            home.path(),
            "wss://relay.example.test/ws",
            std::slice::from_ref(&path),
        )
        .unwrap();
        assert_eq!(
            RelayConfig::load(home.path())
                .unwrap()
                .unwrap()
                .endpoints
                .len(),
            1
        );
        let before = std::fs::read(home.path().join("mobile/relay.json")).unwrap();
        std::fs::write(&path, "not a certificate").unwrap();
        assert!(add(home.path(), "wss://relay.example.test/ws", &[path]).is_err());
        assert!(add(home.path(), "ws://relay.example.test/ws", &[]).is_err());
        assert_eq!(
            std::fs::read(home.path().join("mobile/relay.json")).unwrap(),
            before
        );
        for index in 1..=3 {
            add(
                home.path(),
                &format!("wss://relay{index}.example.test/ws"),
                &[],
            )
            .unwrap();
        }
        let before = std::fs::read(home.path().join("mobile/relay.json")).unwrap();
        assert!(add(home.path(), "wss://fifth.example.test/ws", &[]).is_err());
        assert_eq!(
            std::fs::read(home.path().join("mobile/relay.json")).unwrap(),
            before
        );
    }

    #[test]
    fn concurrent_operator_updates_preserve_both_endpoints() {
        let home = tempfile::tempdir().unwrap();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers: Vec<_> = (0..2)
            .map(|index| {
                let home = home.path().to_owned();
                let gate = std::sync::Arc::clone(&gate);
                std::thread::spawn(move || {
                    gate.wait();
                    add(&home, &format!("wss://relay{index}.example.test/ws"), &[]).unwrap();
                })
            })
            .collect();
        gate.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        let config = RelayConfig::load(home.path()).unwrap().unwrap();
        assert_eq!(config.endpoints.len(), 2);
        assert!(!config.enabled);
    }
}
