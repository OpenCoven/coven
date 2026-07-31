use std::fs;
use std::io;
use std::path::Path;

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::DecodePrivateKey;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::config::{
    atomic_create_private, atomic_replace_private, ensure_private_mobile_dir, validate_private_file,
};

const HOST_IDENTITY_FILE: &str = "host-identity.json";
const HOST_KEY_FILE: &str = "host-key.pem";

pub struct HostIdentity {
    pub certificate_der: Vec<u8>,
    pub private_key_der: Zeroizing<Vec<u8>>,
    pub public_key_fingerprint: [u8; 32],
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredHostIdentity {
    certificate_pem: String,
    public_key_fingerprint: String,
    #[serde(default)]
    subject_alt_name: String,
}

pub fn load_or_create_host_identity(
    coven_home: &Path,
    subject_alt_name: &str,
) -> Result<HostIdentity> {
    let mobile_dir = ensure_private_mobile_dir(coven_home)?;
    let key_path = mobile_dir.join(HOST_KEY_FILE);
    let key_pair = load_or_create_key_pair(&key_path)?;
    let private_key_der = Zeroizing::new(key_pair.serialize_der());
    let public_key_fingerprint = fingerprint_for_private_key(&private_key_der)?;
    let identity_path = mobile_dir.join(HOST_IDENTITY_FILE);

    let certificate_der = match fs::symlink_metadata(&identity_path) {
        Ok(_) => {
            let (certificate, stored_name) =
                load_stored_certificate(&identity_path, public_key_fingerprint)?;
            if stored_name == subject_alt_name {
                certificate
            } else {
                replace_certificate(
                    &identity_path,
                    &key_pair,
                    subject_alt_name,
                    public_key_fingerprint,
                )?
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_or_load_certificate(
            &identity_path,
            &key_pair,
            subject_alt_name,
            public_key_fingerprint,
        )?,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", identity_path.display()));
        }
    };

    Ok(HostIdentity {
        certificate_der,
        private_key_der,
        public_key_fingerprint,
    })
}

fn load_or_create_key_pair(path: &Path) -> Result<KeyPair> {
    match fs::symlink_metadata(path) {
        Ok(_) => load_key_pair(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let generated = KeyPair::generate().context("failed to generate mobile host key")?;
            let pem = generated.serialize_pem();
            if atomic_create_private(path, pem.as_bytes())? {
                Ok(generated)
            } else {
                load_key_pair(path)
            }
        }
        Err(error) => Err(error).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

fn load_key_pair(path: &Path) -> Result<KeyPair> {
    validate_private_file(path)?;
    let pem =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    KeyPair::from_pem(&pem).with_context(|| format!("failed to parse {}", path.display()))
}

fn fingerprint_for_private_key(private_key_der: &[u8]) -> Result<[u8; 32]> {
    let secret_key = p256::SecretKey::from_pkcs8_der(private_key_der)
        .context("mobile host key is not a valid P-256 PKCS#8 key")?;
    let encoded = secret_key.public_key().to_encoded_point(false);
    let digest = Sha256::digest(encoded.as_bytes());
    Ok(digest.into())
}

fn create_or_load_certificate(
    path: &Path,
    key_pair: &KeyPair,
    subject_alt_name: &str,
    public_key_fingerprint: [u8; 32],
) -> Result<Vec<u8>> {
    let mut params = CertificateParams::new(vec![subject_alt_name.to_owned()])
        .context("failed to configure mobile host certificate")?;
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, "Coven Memory");
    params.distinguished_name = distinguished_name;
    let certificate = params
        .self_signed(key_pair)
        .context("failed to create mobile host certificate")?;
    let stored = StoredHostIdentity {
        certificate_pem: certificate.pem(),
        public_key_fingerprint: URL_SAFE_NO_PAD.encode(public_key_fingerprint),
        subject_alt_name: subject_alt_name.to_owned(),
    };
    let mut encoded =
        serde_json::to_vec_pretty(&stored).context("failed to encode mobile host identity")?;
    encoded.push(b'\n');

    if atomic_create_private(path, &encoded)? {
        Ok(certificate.der().to_vec())
    } else {
        let (certificate, stored_name) = load_stored_certificate(path, public_key_fingerprint)?;
        if stored_name == subject_alt_name {
            Ok(certificate)
        } else {
            replace_certificate(path, key_pair, subject_alt_name, public_key_fingerprint)
        }
    }
}

fn replace_certificate(
    path: &Path,
    key_pair: &KeyPair,
    subject_alt_name: &str,
    public_key_fingerprint: [u8; 32],
) -> Result<Vec<u8>> {
    let mut params = CertificateParams::new(vec![subject_alt_name.to_owned()])
        .context("failed to configure mobile host certificate")?;
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, "Coven Memory");
    params.distinguished_name = distinguished_name;
    let certificate = params
        .self_signed(key_pair)
        .context("failed to create mobile host certificate")?;
    let stored = StoredHostIdentity {
        certificate_pem: certificate.pem(),
        public_key_fingerprint: URL_SAFE_NO_PAD.encode(public_key_fingerprint),
        subject_alt_name: subject_alt_name.to_owned(),
    };
    let mut encoded =
        serde_json::to_vec_pretty(&stored).context("failed to encode mobile host identity")?;
    encoded.push(b'\n');
    atomic_replace_private(path, &encoded)?;
    Ok(certificate.der().to_vec())
}

fn load_stored_certificate(
    path: &Path,
    expected_fingerprint: [u8; 32],
) -> Result<(Vec<u8>, String)> {
    validate_private_file(path)?;
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let stored: StoredHostIdentity = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let fingerprint = URL_SAFE_NO_PAD
        .decode(stored.public_key_fingerprint)
        .context("mobile host fingerprint is not valid base64url")?;
    if fingerprint.as_slice() != expected_fingerprint {
        bail!("mobile host identity does not match the private key");
    }

    let certificates = CertificateDer::pem_slice_iter(stored.certificate_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("mobile host certificate PEM is invalid")?;
    if certificates.len() != 1 {
        bail!("mobile host identity must contain exactly one certificate");
    }
    Ok((certificates[0].as_ref().to_vec(), stored.subject_alt_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn host_identity_is_stable_and_private() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let first = load_or_create_host_identity(temp.path(), "192.168.1.10").unwrap();
        let second = load_or_create_host_identity(temp.path(), "192.168.1.10").unwrap();

        assert_eq!(first.certificate_der, second.certificate_der);
        assert_eq!(first.public_key_fingerprint, second.public_key_fingerprint);
        assert_eq!(
            first.private_key_der.as_slice(),
            second.private_key_der.as_slice()
        );

        let mobile = temp.path().join("mobile");
        assert_eq!(
            std::fs::metadata(&mobile).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in ["host-identity.json", "host-key.pem"] {
            assert_eq!(
                std::fs::metadata(mobile.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn endpoint_change_reissues_certificate_without_rotating_host_key() {
        let temp = tempfile::tempdir().unwrap();
        let first = load_or_create_host_identity(temp.path(), "192.168.1.10").unwrap();
        let second = load_or_create_host_identity(temp.path(), "192.168.1.11").unwrap();

        assert_ne!(first.certificate_der, second.certificate_der);
        assert_eq!(first.public_key_fingerprint, second.public_key_fingerprint);
        assert_eq!(
            first.private_key_der.as_slice(),
            second.private_key_der.as_slice()
        );
    }
}
