use std::{fs, path::Path, process::Command};

use serde_json::json;

fn relay(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_coven"))
        .env("COVEN_HOME", home)
        .args(["memory", "mobile", "relay"])
        .args(args)
        .output()
        .unwrap()
}

fn private_file(home: &Path, name: &str, bytes: &[u8]) {
    let mobile = home.join("mobile");
    fs::create_dir_all(&mobile).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mobile, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = mobile.join(name);
    fs::write(&path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn reset_requires_confirmation_and_repairs_invalid_state_without_touching_device_material() {
    for bytes in [
        b"{broken".to_vec(),
        vec![b' '; 16 * 1024 + 1],
        br#"{"version":2,"enabled":false,"endpoints":[]}"#.to_vec(),
        br#"{"version":1,"enabled":true,"endpoints":[{"url":"wss://relay.example.test/ws","caCertificates":[]}]}"#.to_vec(),
    ] {
        let home = tempfile::tempdir().unwrap();
        private_file(home.path(), "relay.json", &bytes);
        // Preservation sentinels: reset must neither read nor rewrite these files.
        let retained = [
            "host-key.pem",
            "host-identity.json",
            "devices.json",
            "transport-11111111-1111-4111-8111-111111111111.json",
            "gateway.json",
        ];
        for name in retained {
            private_file(home.path(), name, name.as_bytes());
        }
        let refused = relay(home.path(), &["reset"]);
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("--yes"),
            "{refused:?}"
        );
        assert_eq!(
            fs::read(home.path().join("mobile/relay.json")).unwrap(),
            bytes
        );

        let reset = relay(home.path(), &["reset", "--yes"]);
        assert!(reset.status.success(), "{reset:?}");
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(home.path().join("mobile/relay.json")).unwrap())
                .unwrap();
        assert_eq!(saved, json!({"version":1,"enabled":false,"endpoints":[]}));
        let status = relay(home.path(), &["status", "--json"]);
        assert!(status.status.success(), "{status:?}");
        let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(status["enabled"], false);
        assert_eq!(status["endpointCount"], 0);
        for name in retained {
            assert_eq!(
                fs::read(home.path().join("mobile").join(name)).unwrap(),
                name.as_bytes()
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(home.path().join("mobile/relay.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn unreadable_relay_errors_name_the_confirmed_recovery_command() {
    for bytes in [
        b"{broken".to_vec(),
        vec![b' '; 16 * 1024 + 1],
        br#"{"version":2,"enabled":false,"endpoints":[]}"#.to_vec(),
    ] {
        let home = tempfile::tempdir().unwrap();
        private_file(home.path(), "relay.json", &bytes);
        for args in [
            vec!["status"],
            vec!["disable"],
            vec!["add", "--endpoint", "wss://relay.example.test/ws"],
            vec!["remove", "--endpoint", "wss://relay.example.test/ws"],
        ] {
            let output = relay(home.path(), &args);
            assert!(!output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("coven memory mobile relay reset --yes"),
                "{output:?}"
            );
            assert_eq!(
                fs::read(home.path().join("mobile/relay.json")).unwrap(),
                bytes
            );
        }
    }
}

#[test]
fn reset_is_idempotent_and_does_not_create_identity_or_devices() {
    let home = tempfile::tempdir().unwrap();
    assert!(!relay(home.path(), &["reset"]).status.success());
    assert!(!home.path().join("mobile").exists());
    for _ in 0..2 {
        let output = relay(home.path(), &["reset", "--yes"]);
        assert!(output.status.success(), "{output:?}");
        let mut files: Vec<_> = fs::read_dir(home.path().join("mobile"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        files.sort();
        assert_eq!(files, [".relay.lock", "relay.json"]);
    }
}

#[cfg(unix)]
#[test]
fn reset_refuses_symlink_configuration_and_preserves_its_target() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};
    let home = tempfile::tempdir().unwrap();
    let mobile = home.path().join("mobile");
    fs::create_dir(&mobile).unwrap();
    fs::set_permissions(&mobile, fs::Permissions::from_mode(0o700)).unwrap();
    let target = home.path().join("retained");
    fs::write(&target, b"retained").unwrap();
    symlink(&target, mobile.join("relay.json")).unwrap();
    let output = relay(home.path(), &["reset", "--yes"]);
    assert!(!output.status.success());
    assert!(fs::symlink_metadata(mobile.join("relay.json"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(target).unwrap(), b"retained");
}

#[test]
fn reset_refuses_a_directory_in_place_of_configuration() {
    let home = tempfile::tempdir().unwrap();
    private_file(home.path(), "retained", b"retained");
    let config = home.path().join("mobile/relay.json");
    fs::create_dir(&config).unwrap();
    let output = relay(home.path(), &["reset", "--yes"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(config.is_dir());
    assert_eq!(
        fs::read(home.path().join("mobile/retained")).unwrap(),
        b"retained"
    );
}

#[cfg(unix)]
#[test]
fn reset_refuses_unsafe_file_or_directory_permissions() {
    use std::os::unix::fs::PermissionsExt;
    for unsafe_directory in [false, true] {
        let home = tempfile::tempdir().unwrap();
        private_file(home.path(), "relay.json", b"{broken");
        let config = home.path().join("mobile/relay.json");
        let unsafe_path = if unsafe_directory {
            home.path().join("mobile")
        } else {
            config.clone()
        };
        let mode = if unsafe_directory { 0o755 } else { 0o644 };
        fs::set_permissions(&unsafe_path, fs::Permissions::from_mode(mode)).unwrap();
        let output = relay(home.path(), &["reset", "--yes"]);
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(fs::read(config).unwrap(), b"{broken");
        assert_eq!(
            fs::metadata(unsafe_path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
}
