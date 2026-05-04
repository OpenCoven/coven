use std::path::Path;

use anyhow::{Context, Result};

use crate::patch::VerificationProfile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationResult {
    pub command: String,
    pub status: VerificationStatus,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationStatus {
    Passed,
    Failed(i32),
}

pub fn commands_for_profile(profile: &VerificationProfile) -> Vec<VerificationCommand> {
    let diff_check = VerificationCommand {
        program: "git".to_string(),
        args: vec!["diff".to_string(), "--check".to_string()],
    };

    match profile {
        VerificationProfile::Auto
        | VerificationProfile::DiffOnly
        | VerificationProfile::TargetedTest => {
            vec![diff_check]
        }
        VerificationProfile::PnpmCheck => vec![
            diff_check,
            VerificationCommand {
                program: "pnpm".to_string(),
                args: vec!["check".to_string()],
            },
        ],
    }
}

pub fn run_verification(
    repo_root: &Path,
    profile: &VerificationProfile,
) -> Result<Vec<VerificationResult>> {
    commands_for_profile(profile)
        .into_iter()
        .map(|command| run_command(repo_root, command))
        .collect()
}

fn run_command(repo_root: &Path, command: VerificationCommand) -> Result<VerificationResult> {
    let output = std::process::Command::new(&command.program)
        .args(&command.args)
        .current_dir(repo_root)
        .output()
        .with_context(|| {
            format!(
                "failed to run verification command `{}`",
                format_command(&command)
            )
        })?;
    let code = output.status.code().unwrap_or(1);
    let status = if output.status.success() {
        VerificationStatus::Passed
    } else {
        VerificationStatus::Failed(code)
    };

    Ok(VerificationResult {
        command: format_command(&command),
        status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn format_command(command: &VerificationCommand) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[test]
    fn auto_runs_diff_check_only_as_safe_default() {
        let commands = commands_for_profile(&VerificationProfile::Auto);

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].program, "git");
        assert_eq!(commands[0].args, vec!["diff", "--check"]);
    }

    #[test]
    fn pnpm_check_runs_diff_check_then_pnpm_check() {
        let commands = commands_for_profile(&VerificationProfile::PnpmCheck);

        assert_eq!(commands[0].args, vec!["diff", "--check"]);
        assert_eq!(commands[1].program, "pnpm");
        assert_eq!(commands[1].args, vec!["check"]);
    }

    #[test]
    fn diff_only_runs_only_diff_check() {
        let commands = commands_for_profile(&VerificationProfile::DiffOnly);

        assert_eq!(
            commands,
            vec![VerificationCommand {
                program: "git".to_string(),
                args: vec!["diff".to_string(), "--check".to_string()],
            }]
        );
    }

    #[test]
    fn run_verification_passes_on_clean_repo() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        run_git_for_test(repo, &["init"])?;

        let results = run_verification(repo, &VerificationProfile::DiffOnly)?;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].command, "git diff --check");
        assert!(matches!(results[0].status, VerificationStatus::Passed));
        Ok(())
    }

    fn run_git_for_test(repo_root: &Path, args: &[&str]) -> Result<()> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .output()?;
        if !output.status.success() {
            anyhow::bail!(
                "git test command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}
