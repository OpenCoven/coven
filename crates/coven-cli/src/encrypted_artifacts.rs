use std::path::Path;

use anyhow::{anyhow, Context, Result};
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    XChaCha20Poly1305, XNonce,
};

const KEY_DIR_NAME: &str = "keys";
const KEY_FILE_NAME: &str = "session-artifacts.key";
const KEY_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedPayload {
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

#[derive(Debug)]
pub struct SensitiveArtifactStore {
    key: [u8; KEY_LEN],
}

impl SensitiveArtifactStore {
    pub fn load(coven_home: &Path) -> Result<Self> {
        let key_path = coven_home.join(KEY_DIR_NAME).join(KEY_FILE_NAME);
        let key = match std::fs::read_to_string(&key_path) {
            Ok(raw) => decode_key(raw.trim())
                .with_context(|| "failed to load artifact encryption key".to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = XChaCha20Poly1305::generate_key(&mut OsRng);
                let bytes: [u8; KEY_LEN] = key
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow!("failed to generate artifact encryption key material"))?;
                write_key_file(&key_path, &bytes)?;
                bytes
            }
            Err(error) => {
                return Err(error).with_context(|| "failed to load artifact encryption key");
            }
        };
        Ok(Self { key })
    }

    pub fn encrypt(
        &self,
        session_id: &str,
        event_id: &str,
        kind: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedPayload> {
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|_| anyhow!("failed to initialize artifact encryption"))?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let aad = artifact_aad(session_id, event_id, kind);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("failed to encrypt sensitive artifact"))?;
        Ok(EncryptedPayload {
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    pub fn decrypt(
        &self,
        session_id: &str,
        event_id: &str,
        kind: &str,
        payload: &EncryptedPayload,
    ) -> Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|_| anyhow!("failed to initialize artifact encryption"))?;
        if payload.nonce.len() != 24 {
            anyhow::bail!("sensitive artifact nonce is invalid");
        }
        let aad = artifact_aad(session_id, event_id, kind);
        cipher
            .decrypt(
                XNonce::from_slice(&payload.nonce),
                Payload {
                    msg: payload.ciphertext.as_slice(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("failed to decrypt sensitive artifact"))
    }
}

fn artifact_aad(session_id: &str, event_id: &str, kind: &str) -> String {
    format!("coven.session-artifact.v1:{session_id}:{event_id}:{kind}")
}

fn write_key_file(path: &Path, key: &[u8; KEY_LEN]) -> Result<()> {
    let parent = path
        .parent()
        .context("artifact encryption key path has no parent")?;
    std::fs::create_dir_all(parent)
        .context("failed to create artifact encryption key directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .context("failed to protect artifact encryption key directory")?;
    }
    std::fs::write(path, encode_key(key)).context("failed to write artifact encryption key")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .context("failed to protect artifact encryption key")?;
    }
    Ok(())
}

fn encode_key(key: &[u8; KEY_LEN]) -> String {
    let mut out = String::with_capacity(KEY_LEN * 2 + 1);
    for byte in key {
        out.push_str(&format!("{byte:02x}"));
    }
    out.push('\n');
    out
}

fn decode_key(raw: &str) -> Result<[u8; KEY_LEN]> {
    if raw.len() != KEY_LEN * 2 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("artifact encryption key is invalid");
    }
    let mut key = [0_u8; KEY_LEN];
    for (idx, chunk) in raw.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(chunk).context("artifact encryption key is invalid")?;
        key[idx] = u8::from_str_radix(pair, 16).context("artifact encryption key is invalid")?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_payload() -> Vec<u8> {
        [
            b"{\"data\":\"".as_slice(),
            b"fake-private-session-payload".as_slice(),
            b"\"}".as_slice(),
        ]
        .concat()
    }

    #[test]
    fn encrypt_decrypt_round_trip_uses_nonce_and_aad() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = SensitiveArtifactStore::load(temp.path())?;
        let plaintext = fake_payload();

        let encrypted = store.encrypt("session-1", "event-1", "output", &plaintext)?;
        assert_ne!(encrypted.ciphertext, plaintext);
        assert_eq!(encrypted.nonce.len(), 24);

        let decrypted = store.decrypt("session-1", "event-1", "output", &encrypted)?;
        assert_eq!(decrypted, plaintext);

        let wrong_aad = store.decrypt("session-1", "event-2", "output", &encrypted);
        assert!(wrong_aad.is_err());
        Ok(())
    }

    #[test]
    fn invalid_key_file_fails_closed_without_plaintext() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let keys = temp.path().join("keys");
        std::fs::create_dir_all(&keys)?;
        std::fs::write(keys.join("session-artifacts.key"), "not-a-valid-key")?;

        let error = SensitiveArtifactStore::load(temp.path()).unwrap_err();
        let message = error.to_string();

        assert!(message.contains("artifact encryption key"));
        assert!(!message.contains("not-a-valid-key"));
        Ok(())
    }
}
