use std::process::{Command, Output};

fn coven(home: &std::path::Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_coven"))
        .env("COVEN_HOME", home)
        .args(arguments)
        .output()
        .expect("run coven")
}

#[test]
fn mobile_gateway_is_disabled_by_default_in_a_fresh_home() {
    let home = tempfile::tempdir().unwrap();
    let output = coven(home.path(), &["memory", "mobile", "status", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["configured"], false);
    assert_eq!(status["enabled"], false);
    assert!(!home.path().join("mobile/gateway.json").exists());
}

#[test]
fn mobile_enable_rejects_public_and_wildcard_bindings_without_state() {
    for bind in ["0.0.0.0:7443", "203.0.113.10:7443"] {
        let home = tempfile::tempdir().unwrap();
        let endpoint = format!("https://{bind}");
        let output = coven(
            home.path(),
            &[
                "memory",
                "mobile",
                "enable",
                "--bind",
                bind,
                "--endpoint",
                &endpoint,
            ],
        );
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("private-network"),
            "{output:?}"
        );
        assert!(!home.path().join("mobile/gateway.json").exists());
    }
}

#[test]
fn mobile_restore_install_short_hyphen_pin_reaches_strict_validation_without_state() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let home = tempfile::tempdir().unwrap();
    let snapshot = tempfile::tempdir().unwrap();
    let authority = snapshot.path().join("missing-authority.json");
    let pin = URL_SAFE_NO_PAD.encode([0xf8]);
    assert!(pin.starts_with('-'));
    let output = coven(
        home.path(),
        &[
            "memory",
            "mobile",
            "restore",
            "install",
            "--authority",
            authority.to_str().unwrap(),
            "--snapshot-home",
            snapshot.path().to_str().unwrap(),
            "--installation-fingerprint",
            &pin,
            "--operation-id",
            "00000000-0000-0000-0000-000000000001",
            "--json",
        ],
    );
    assert!(!output.status.success(), "{output:?}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("installation fingerprint must be 32 bytes"),
        "{output:?}"
    );
    assert!(!error.contains("unexpected argument"), "{output:?}");
    assert!(!home.path().join("mobile").exists());
    assert!(!authority.exists());
}

#[test]
fn restore_authority_export_cli_preserves_revocation_and_reports_exact_artifact_digest() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use p256::ecdsa::SigningKey;
    use sha2::{Digest, Sha256};
    let home = tempfile::tempdir().unwrap();
    let enabled = coven(
        home.path(),
        &[
            "memory",
            "mobile",
            "enable",
            "--bind",
            "192.168.50.1:7443",
            "--endpoint",
            "https://192.168.50.1:7443",
        ],
    );
    assert!(enabled.status.success(), "{enabled:?}");
    let mobile = home.path().join("mobile");
    let identity_before = std::fs::read(mobile.join("host-identity.json")).unwrap();
    // A synthetic current v2 registry fixture. Its independent public-key
    // fingerprint follows the persisted grant format, not the export builder.
    let key = SigningKey::from_slice(&[23; 32]).unwrap();
    let public = key.verifying_key().to_sec1_point(false);
    let public_encoded = URL_SAFE_NO_PAD.encode(public.as_bytes());
    let source = serde_json::json!({
        "version": 2,
        "devices": [{
            "device": {
                "id": "00000000-0000-0000-0000-000000000385",
                "displayName": "Synthetic CLI restore phone",
                "publicKeyX963": public_encoded,
                "pairedAt": "2024-01-01T00:00:00Z",
                "revokedAt": "2024-01-02T00:00:00Z",
                "scopes": ["memory_read"]
            },
            "grant": {
                "version": 1, "id": "00000000-0000-0000-0000-000000000384",
                "subjectKeyId": URL_SAFE_NO_PAD.encode(Sha256::digest(public.as_bytes())),
                "audience": "local_coven_authority", "scopes": ["memory_read"],
                "restrictions": {"transport":"any_authenticated", "requireFreshUserVerificationFor":[]},
                "minimumAssurance": "possession", "issuedAt": "2024-01-01T00:00:00Z",
                "notBefore": "2024-01-01T00:00:00Z", "expiresAt": null, "revocationEpoch": 1
            }
        }]
    });
    let source_bytes = serde_json::to_vec(&source).unwrap();
    let registry_path = mobile.join("devices.json");
    std::fs::write(&registry_path, &source_bytes).unwrap();
    let backups = home.path().join("backups");
    std::fs::create_dir(&backups).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&registry_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let output_path = backups.join("authority.json");
    let arguments = [
        "memory",
        "mobile",
        "restore",
        "export",
        "--output",
        output_path.to_str().unwrap(),
        "--json",
    ];
    let exported = coven(home.path(), &arguments);
    assert!(exported.status.success(), "{exported:?}");
    let report: serde_json::Value = serde_json::from_slice(&exported.stdout).unwrap();
    let bytes = std::fs::read(&output_path).unwrap();
    assert_eq!(report["pairingCount"], 1);
    assert_eq!(report["installationEpoch"], 0);
    use std::fmt::Write as _;
    let mut expected_digest = String::new();
    for byte in Sha256::digest(&bytes) {
        write!(&mut expected_digest, "{byte:02x}").unwrap();
    }
    assert_eq!(report["sha256"], expected_digest);
    assert!(!String::from_utf8_lossy(&exported.stdout).contains(&public_encoded));
    let artifact: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let captured: serde_json::Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(artifact["registry"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(captured, source);
    assert_eq!(std::fs::read(&registry_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read(mobile.join("host-identity.json")).unwrap(),
        identity_before
    );
    let repeated = coven(home.path(), &arguments);
    assert!(!repeated.status.success());
    assert_eq!(std::fs::read(&output_path).unwrap(), bytes);
    let pin: Vec<u8> = serde_json::from_value(artifact["installationFingerprint"].clone()).unwrap();
    let pin = URL_SAFE_NO_PAD.encode(pin);
    let snapshot = tempfile::tempdir().unwrap();
    let snapshot_mobile = snapshot.path().join("mobile");
    std::fs::create_dir(&snapshot_mobile).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&snapshot_mobile, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for name in [
        "host-key.pem",
        "host-identity.json",
        "devices.json",
        "gateway.json",
    ] {
        let destination = snapshot_mobile.join(name);
        std::fs::copy(mobile.join(name), &destination).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let operation = uuid::Uuid::new_v4().to_string();
    let install_args = [
        "memory",
        "mobile",
        "restore",
        "install",
        "--snapshot-home",
        snapshot.path().to_str().unwrap(),
        "--authority",
        output_path.to_str().unwrap(),
        "--installation-fingerprint",
        &pin,
        "--operation-id",
        &operation,
        "--json",
    ];
    // A distinct process uses the same shared state lock as ordinary CLI and
    // daemon users. Installation must refuse before publishing any fence.
    use std::io::BufRead as _;
    struct LockHolder(std::process::Child);
    impl Drop for LockHolder {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut holder = LockHolder(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "restore_shared_state_lock_child",
                "--nocapture",
            ])
            .env("COVEN_TEST_RESTORE_LOCK_HOME", home.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = holder.0.stdout.take().unwrap();
    let (ready, received) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if line.contains("RESTORE_STATE_LOCK_READY") {
                let _ = ready.send(());
            }
        }
    });
    // This is a hang guard, far above normal child startup; readiness is an
    // explicit event and EOF reports early child failure immediately.
    let started = std::time::Instant::now();
    received
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap_or_else(|error| {
            panic!(
                "restore lock child not ready after {:?}: {error}",
                started.elapsed()
            )
        });
    let blocked = coven(home.path(), &install_args);
    assert!(!blocked.status.success(), "{blocked:?}");
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("stop the daemon"),
        "{blocked:?}"
    );
    assert!(!mobile.join("transport-restore.json").exists());
    drop(holder.0.stdin.take());
    assert!(holder.0.wait().unwrap().success());
    reader.join().unwrap();
    let installed = coven(home.path(), &install_args);
    assert!(installed.status.success(), "{installed:?}");
    let receipt: serde_json::Value = serde_json::from_slice(&installed.stdout).unwrap();
    assert_eq!(receipt["operationId"], operation);
    assert_eq!(receipt["installationEpoch"], 1);
    let restored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
    assert_eq!(restored["restoreEpoch"], 1);
    assert!(restored["devices"][0]["device"]["revokedAt"].is_string());
    assert_eq!(restored["devices"][0]["grant"]["revocationEpoch"], 2);
    let repeated = coven(home.path(), &install_args);
    assert!(repeated.status.success(), "{repeated:?}");
    let fresh_destination = tempfile::tempdir().unwrap();
    let restored_fresh = coven(fresh_destination.path(), &install_args);
    assert!(restored_fresh.status.success(), "{restored_fresh:?}");
    assert_eq!(
        std::fs::read(fresh_destination.path().join("mobile/host-identity.json")).unwrap(),
        identity_before
    );
    let restored_config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fresh_destination.path().join("mobile/gateway.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        restored_config,
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(snapshot_mobile.join("gateway.json")).unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        std::fs::read(mobile.join("host-identity.json")).unwrap(),
        identity_before
    );
}

#[test]
#[ignore = "invoked by the native restore parent test with an explicit home"]
fn restore_shared_state_lock_child() {
    use std::io::{Read as _, Write as _};
    let home = std::env::var_os("COVEN_TEST_RESTORE_LOCK_HOME").expect("explicit child home");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(std::path::Path::new(&home).join("state.lock"))
        .unwrap();
    fs2::FileExt::lock_shared(&lock).unwrap();
    println!("RESTORE_STATE_LOCK_READY");
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    let received = std::io::stdin().read(&mut byte).unwrap();
    assert_eq!(
        received, 0,
        "parent releases the child by closing its stdin"
    );
    fs2::FileExt::unlock(&lock).unwrap();
}
