#[cfg(unix)]
mod unix {
    use anyhow::Result;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn coven_bin() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_coven"))
    }

    fn stop_daemon(coven_home: &Path) {
        let _ = Command::new(coven_bin())
            .args(["daemon", "stop"])
            .env("COVEN_HOME", coven_home)
            .output();
    }

    struct DaemonGuard(PathBuf);

    impl Drop for DaemonGuard {
        fn drop(&mut self) {
            stop_daemon(&self.0);
        }
    }

    #[test]
    fn memory_open_starts_daemon_before_dashboard() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let coven_home = temp.path().join("coven-home");
        fs::create_dir_all(&coven_home)?;
        let _daemon_guard = DaemonGuard(coven_home.clone());
        let marker = coven_home.join("dashboard-launched");
        let dashboard = temp.path().join("fake-dashboard");
        fs::write(
            &dashboard,
            "#!/bin/sh\n\
             test -S \"$COVEN_HOME/coven.sock\" || exit 42\n\
             printf launched > \"$COVEN_TEST_DASHBOARD_MARKER\"\n",
        )?;
        fs::set_permissions(&dashboard, fs::Permissions::from_mode(0o755))?;

        let output = Command::new(coven_bin())
            .args(["memory", "open"])
            .env("COVEN_HOME", &coven_home)
            .env("COVEN_MEMORY_DASHBOARD_BIN", &dashboard)
            .env("COVEN_TEST_DASHBOARD_MARKER", &marker)
            .output()?;

        assert!(
            output.status.success(),
            "memory open failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(marker)?, "launched");
        Ok(())
    }
}
