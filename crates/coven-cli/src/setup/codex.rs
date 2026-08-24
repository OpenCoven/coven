use super::{verify, CommandSpec, ProviderDescriptor, ProviderId};

pub const EXECUTABLE: &str = "codex";
pub const INSTALL_GUIDANCE: &str = "Install Codex with `npm install -g @openai/codex` or `brew install --cask codex`; if it is already installed, make sure `codex` is on PATH and run `codex login` or `codex` once to authenticate, then retry `coven doctor`.";
pub const NON_INTERACTIVE_PREFIX_ARGS: &[&str] =
    &["exec", "--skip-git-repo-check", "--color", "never"];

pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor::new(ProviderId::Codex, EXECUTABLE, INSTALL_GUIDANCE)
        .with_login(CommandSpec::new(["login"]))
        .with_verification(verify::command(ProviderId::Codex))
}
