//! Chat's explicit, host-owned per-run workspace authority. This is separate
//! from Full and from the daemon-only unattended launch contract.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, fs, io::Read, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatWorkspacePolicy {
    pub version: u32,
    pub run_id: String,
    pub access: String,
    pub primary_root: String,
    pub writable_roots: Vec<String>,
    pub read_only_roots: Vec<String>,
}
impl ChatWorkspacePolicy {
    pub fn read(file: &Path, cwd: &Path) -> Result<Self> {
        let metadata =
            fs::symlink_metadata(file).context("could not inspect Chat workspace policy")?;
        ensure!(
            metadata.is_file() && metadata.len() <= 65_536,
            "Chat workspace policy must be a regular file no larger than 65536 bytes"
        );
        let mut bytes = Vec::new();
        fs::File::open(file)?.take(65_537).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 65_536, "Chat workspace policy is too large");
        let policy: Self = serde_json::from_slice(&bytes)?;
        policy.validate(cwd)?;
        Ok(policy)
    }
    pub fn validate(&self, cwd: &Path) -> Result<()> {
        ensure!(
            self.version == 1 && self.access == "workspace-write",
            "unsupported Chat workspace policy"
        );
        ensure!(
            !self.run_id.is_empty()
                && self.run_id.len() <= 128
                && self
                    .run_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c)),
            "invalid Chat run identity"
        );
        ensure!(
            self.writable_roots.len() <= 128 && self.read_only_roots.len() <= 128,
            "too many Chat workspace roots"
        );
        let canonical = |value: &str| -> Result<()> {
            let path = Path::new(value);
            ensure!(
                path.is_absolute() && path.is_dir() && path.parent().is_some(),
                "Chat workspace root must be an existing dedicated absolute folder"
            );
            ensure!(
                crate::project::canonical_project_root(path)? == path,
                "Chat workspace roots must be canonical"
            );
            ensure!(
                dirs_next::home_dir().is_none_or(|home| home != path),
                "Chat workspace root cannot be the home folder"
            );
            ensure!(
                !value.chars().any(char::is_control),
                "Chat workspace root contains control characters"
            );
            Ok(())
        };
        canonical(&self.primary_root)?;
        ensure!(
            crate::project::canonical_project_root(cwd)? == Path::new(&self.primary_root),
            "Chat workspace policy differs from launch cwd"
        );
        for roots in [&self.writable_roots, &self.read_only_roots] {
            let sorted: Vec<_> = roots
                .iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .cloned()
                .collect();
            ensure!(
                &sorted == roots,
                "Chat workspace roots must be sorted and unique"
            );
            for root in roots {
                canonical(root)?;
            }
        }
        ensure!(
            self.writable_roots
                .iter()
                .any(|write| Path::new(&self.primary_root).starts_with(write)),
            "Chat workspace policy does not grant the launch folder"
        );
        ensure!(
            !self.read_only_roots.iter().any(|read| self
                .writable_roots
                .iter()
                .any(|write| Path::new(read).starts_with(write))),
            "Chat workspace policy has overlapping read-only and write roots"
        );
        Ok(())
    }
    pub fn require_protected(&self, path: &Path) -> Result<()> {
        let path = fs::canonicalize(path)?;
        ensure!(
            !self.writable_roots.iter().any(|root| path.starts_with(root)),
            "A workspace grant overlaps a file or folder needed to enforce workspace access. Narrow the saved grant before retrying."
        );
        Ok(())
    }
    pub fn verify_receipt(&self, receipt: Option<&Value>) -> Result<()> {
        let mut expected = serde_json::to_value(self)?;
        expected["enforced"] = Value::Bool(true);
        ensure!(receipt == Some(&expected),
            "The engine did not confirm the exact workspace write policy. Update Coven Code before retrying.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, ChatWorkspacePolicy) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let path = workspace.to_str().unwrap().to_owned();
        (
            temp,
            ChatWorkspacePolicy {
                version: 1,
                run_id: "run-1".into(),
                access: "workspace-write".into(),
                primary_root: path.clone(),
                writable_roots: vec![path],
                read_only_roots: vec![],
            },
        )
    }
    #[test]
    fn only_exact_enforced_receipt_admits_a_turn() {
        let (_temp, policy) = fixture();
        let mut receipt = serde_json::to_value(&policy).unwrap();
        assert!(policy.verify_receipt(None).is_err());
        assert!(policy.verify_receipt(Some(&receipt)).is_err());
        receipt["enforced"] = Value::Bool(true);
        assert!(policy.verify_receipt(Some(&receipt)).is_ok());
        receipt["runId"] = Value::String("previous-run".into());
        assert!(policy.verify_receipt(Some(&receipt)).is_err());
    }
    #[test]
    fn full_corrupt_version_and_ungranted_cwd_are_rejected() {
        let (temp, policy) = fixture();
        let outside = temp.path().canonicalize().unwrap();
        assert!(policy.validate(&outside).is_err());
        let mut full = policy.clone();
        full.access = "full".into();
        assert!(full.validate(Path::new(&full.primary_root)).is_err());
        let mut future = policy;
        future.version = 2;
        assert!(future.validate(Path::new(&future.primary_root)).is_err());
    }
}
