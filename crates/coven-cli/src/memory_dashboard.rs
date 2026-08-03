use anyhow::{anyhow, bail, Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

const ENTRY_ENV: &str = "COVEN_MEMORY_DASHBOARD_ENTRY";
const NODE_ENV: &str = "COVEN_MEMORY_DASHBOARD_NODE";
const BIN_ENV: &str = "COVEN_MEMORY_DASHBOARD_BIN";

#[derive(Debug, PartialEq, Eq)]
struct LaunchCommand {
    program: PathBuf,
    args: Vec<OsString>,
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn resolve_from(
    node: Option<&OsStr>,
    entry: Option<&OsStr>,
    override_bin: Option<&OsStr>,
    path_var: Option<&OsStr>,
) -> Option<LaunchCommand> {
    if let (Some(node), Some(entry)) = (node, entry) {
        let node = PathBuf::from(node);
        let entry = PathBuf::from(entry);
        if executable(&node) && entry.is_file() {
            return Some(LaunchCommand {
                program: node,
                args: vec![entry.into_os_string()],
            });
        }
    }

    if let Some(override_bin) = override_bin {
        let path = PathBuf::from(override_bin);
        if executable(&path) {
            return Some(LaunchCommand {
                program: path,
                args: Vec::new(),
            });
        }
    }

    let path_var = path_var?;
    for directory in std::env::split_paths(path_var) {
        #[cfg(windows)]
        let names = [
            "coven-memory-dashboard.exe",
            "coven-memory-dashboard.cmd",
            "coven-memory-dashboard.bat",
        ];
        #[cfg(not(windows))]
        let names = ["coven-memory-dashboard"];

        for name in names {
            let candidate = directory.join(name);
            if executable(&candidate) {
                return Some(LaunchCommand {
                    program: candidate,
                    args: Vec::new(),
                });
            }
        }
    }
    None
}

fn resolve() -> Option<LaunchCommand> {
    resolve_from(
        std::env::var_os(NODE_ENV).as_deref(),
        std::env::var_os(ENTRY_ENV).as_deref(),
        std::env::var_os(BIN_ENV).as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
}

fn dashboard_not_installed_error() -> anyhow::Error {
    anyhow!(
        "The Coven Memory dashboard is not installed.\n\n  \
         npm install -g @opencoven/coven-memory-dashboard\n\n\
         Then rerun: coven memory open"
    )
}

fn ensure_daemon_ready() -> Result<()> {
    let coven_home = crate::coven_home_dir()?;
    let current_exe = std::env::current_exe().context("failed to resolve current executable")?;
    crate::daemon::ensure_background_server(&coven_home, &current_exe, crate::current_timestamp())?;
    Ok(())
}

fn prepare_open_with(
    resolve_launch: impl FnOnce() -> Option<LaunchCommand>,
    ensure_daemon: impl FnOnce() -> Result<()>,
) -> Result<LaunchCommand> {
    let launch = resolve_launch().ok_or_else(dashboard_not_installed_error)?;
    ensure_daemon().context("failed to start or reach Coven daemon for Memory")?;
    Ok(launch)
}

pub fn run_open() -> Result<()> {
    let launch = prepare_open_with(resolve, ensure_daemon_ready)?;
    let status = Command::new(&launch.program)
        .args(&launch.args)
        .status()
        .with_context(|| {
            format!(
                "failed to launch Coven Memory with {}",
                launch.program.display()
            )
        })?;
    if !status.success() {
        bail!("Coven Memory exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn resolves_wrapper_supplied_node_entrypoint_first() {
        let temp = tempfile::tempdir().unwrap();
        let node = temp.path().join("node");
        let entry = temp.path().join("dashboard.mjs");
        fs::write(&node, "").unwrap();
        fs::write(&entry, "").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();

        let resolved =
            resolve_from(Some(node.as_os_str()), Some(entry.as_os_str()), None, None).unwrap();

        assert_eq!(resolved.program, node);
        assert_eq!(resolved.args, vec![entry.into_os_string()]);
    }

    #[test]
    fn rejects_an_incomplete_wrapper_contract() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("dashboard.mjs");
        fs::write(&entry, "").unwrap();

        assert!(resolve_from(None, Some(entry.as_os_str()), None, None).is_none());
    }

    #[test]
    fn missing_dashboard_does_not_start_daemon() {
        let daemon_called = std::cell::Cell::new(false);

        let error = prepare_open_with(
            || None,
            || {
                daemon_called.set(true);
                Ok(())
            },
        )
        .expect_err("missing dashboard must fail");

        assert!(!daemon_called.get());
        assert!(error.to_string().contains("dashboard is not installed"));
    }

    #[test]
    fn daemon_failure_prevents_dashboard_preparation() {
        let launch = LaunchCommand {
            program: PathBuf::from("dashboard"),
            args: Vec::new(),
        };

        let error = prepare_open_with(
            || Some(launch),
            || anyhow::bail!("socket did not become ready"),
        )
        .expect_err("daemon failure must stop launch preparation");

        assert!(error
            .to_string()
            .contains("failed to start or reach Coven daemon for Memory"));
    }

    #[test]
    fn ready_daemon_returns_the_resolved_dashboard() {
        let launch = LaunchCommand {
            program: PathBuf::from("dashboard"),
            args: vec![OsString::from("entry")],
        };

        let prepared = prepare_open_with(
            || {
                Some(LaunchCommand {
                    program: launch.program.clone(),
                    args: launch.args.clone(),
                })
            },
            || Ok(()),
        )
        .expect("ready daemon permits dashboard launch");

        assert_eq!(prepared, launch);
    }
}
