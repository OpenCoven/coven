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

pub fn run_open() -> Result<()> {
    let launch = resolve().ok_or_else(|| {
        anyhow!(
            "The Coven Memory dashboard is not installed.\n\n  \
             npm install -g @opencoven/coven-memory-dashboard\n\n\
             Then rerun: coven memory open"
        )
    })?;
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
}
