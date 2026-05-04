use std::path::PathBuf;

use crate::openclaw_repo::{GitState, OpenClawRepo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationProfile {
    Auto,
    PnpmCheck,
    TargetedTest,
    DiffOnly,
}

impl VerificationProfile {
    pub fn parse(value: Option<&str>) -> anyhow::Result<Self> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "pnpm-check" => Ok(Self::PnpmCheck),
            "targeted-test" => Ok(Self::TargetedTest),
            "diff-only" => Ok(Self::DiffOnly),
            other => anyhow::bail!("unknown verification profile `{other}`"),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::PnpmCheck => "pnpm-check",
            Self::TargetedTest => "targeted-test",
            Self::DiffOnly => "diff-only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOpenClawRequest {
    pub repo: OpenClawRepo,
    pub git_state: GitState,
    pub issue: String,
    pub harness_id: String,
    pub verification_profile: VerificationProfile,
    pub non_interactive: bool,
    pub dry_run: bool,
    pub keep_session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOpenClawReport {
    pub status: String,
    pub session_id: String,
    pub changed_files: Vec<String>,
    pub verification: Vec<String>,
}

pub fn build_repair_brief(request: &PatchOpenClawRequest) -> String {
    let dirty = if request.git_state.dirty_files.is_empty() {
        "none".to_string()
    } else {
        request.git_state.dirty_files.join(", ")
    };
    let untracked = if request.git_state.untracked_files.is_empty() {
        "none".to_string()
    } else {
        request.git_state.untracked_files.join(", ")
    };

    format!(
        "You are repairing a local OpenClaw source checkout through Coven.\n\n\
        Repository: {repo}\n\
        Branch: {branch}\n\
        HEAD: {head}\n\
        Existing modified files: {dirty}\n\
        Existing untracked files: {untracked}\n\n\
        Issue to repair:\n{issue}\n\n\
        Instructions:\n\
        - Investigate root cause before changing code.\n\
        - Make the smallest targeted patch that fixes the root cause.\n\
        - Add or update tests when meaningful.\n\
        - Run `git diff --check` before reporting success.\n\
        - Run targeted tests for touched behavior when possible.\n\
        - Do not commit.\n\
        - Do not push.\n\
        - Do not run destructive git commands.\n\
        - Respect existing uncommitted changes and do not clobber them.\n\
        - Finish with a concise summary, changed files, and verification output.\n",
        repo = request.repo.root.display(),
        branch = request.git_state.branch,
        head = request.git_state.head,
        dirty = dirty,
        untracked = untracked,
        issue = request.issue.trim()
    )
}

pub fn summarize_patch_plan(request: &PatchOpenClawRequest) -> String {
    format!(
        "Coven will patch OpenClaw at {} using harness `{}` with verification `{}`.\n\
        Issue: {}\n\
        Nothing will be committed or pushed.",
        request.repo.root.display(),
        request.harness_id,
        request.verification_profile.as_str(),
        request.issue.trim()
    )
}

pub fn summarize_patch_report(report: &PatchOpenClawReport) -> String {
    let changed_files = if report.changed_files.is_empty() {
        "none".to_string()
    } else {
        report.changed_files.join("\n- ")
    };
    let verification = if report.verification.is_empty() {
        "not run".to_string()
    } else {
        report.verification.join("\n- ")
    };

    format!(
        "Coven patch status: {status}\n\
        Session: {session_id}\n\
        Changed files:\n- {changed_files}\n\
        Verification:\n- {verification}\n\
        Nothing was committed or pushed.",
        status = report.status,
        session_id = report.session_id,
        changed_files = changed_files,
        verification = verification
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(issue: &str) -> PatchOpenClawRequest {
        PatchOpenClawRequest {
            repo: OpenClawRepo {
                root: PathBuf::from("/repo/openclaw"),
                package_name: Some("openclaw".to_string()),
            },
            git_state: GitState {
                branch: "fix/auth".to_string(),
                head: "abc1234".to_string(),
                dirty_files: vec!["CHANGELOG.md".to_string()],
                untracked_files: vec![],
            },
            issue: issue.to_string(),
            harness_id: "codex".to_string(),
            verification_profile: VerificationProfile::Auto,
            non_interactive: false,
            dry_run: false,
            keep_session: false,
        }
    }

    #[test]
    fn repair_brief_requires_root_cause_tests_and_no_commits() {
        let brief = build_repair_brief(&request("fix invalidated Codex auth profile order"));

        assert!(brief.contains("fix invalidated Codex auth profile order"));
        assert!(brief.contains("Investigate root cause before changing code"));
        assert!(brief.contains("Do not commit"));
        assert!(brief.contains("Do not push"));
        assert!(brief.contains("Respect existing uncommitted changes"));
        assert!(brief.contains("CHANGELOG.md"));
        assert!(brief.contains("git diff --check"));
    }

    #[test]
    fn patch_plan_summary_names_repo_harness_and_verification() {
        let summary = summarize_patch_plan(&request("fix auth"));

        assert!(summary.contains("/repo/openclaw"));
        assert!(summary.contains("codex"));
        assert!(summary.contains("auto"));
        assert!(summary.contains("fix auth"));
    }

    #[test]
    fn parses_verification_profiles() -> anyhow::Result<()> {
        assert_eq!(VerificationProfile::parse(None)?, VerificationProfile::Auto);
        assert_eq!(
            VerificationProfile::parse(Some("pnpm-check"))?,
            VerificationProfile::PnpmCheck
        );
        assert_eq!(
            VerificationProfile::parse(Some("targeted-test"))?,
            VerificationProfile::TargetedTest
        );
        assert_eq!(
            VerificationProfile::parse(Some("diff-only"))?,
            VerificationProfile::DiffOnly
        );
        assert!(VerificationProfile::parse(Some("everything")).is_err());
        Ok(())
    }

    #[test]
    fn patch_report_reminds_user_nothing_was_committed_or_pushed() {
        let report = PatchOpenClawReport {
            status: "patched".to_string(),
            session_id: "session-1".to_string(),
            changed_files: vec!["src/file.rs".to_string()],
            verification: vec!["git diff --check passed".to_string()],
        };

        let summary = summarize_patch_report(&report);

        assert!(summary.contains("patched"));
        assert!(summary.contains("src/file.rs"));
        assert!(summary.contains("git diff --check passed"));
        assert!(summary.contains("Nothing was committed or pushed"));
    }
}
