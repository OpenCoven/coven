use super::{CommandSpec, ProviderDescriptor, ProviderId};

pub const EXECUTABLE: &str = "claude";
pub const INSTALL_GUIDANCE: &str = "Install Claude Code with `npm install -g @anthropic-ai/claude-code`; if it is already installed, make sure `claude` is on PATH and run `claude auth login` to authenticate, then retry `coven doctor`.";

pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor::new(ProviderId::Claude, EXECUTABLE, INSTALL_GUIDANCE)
        .with_login(CommandSpec::new(["auth", "login"]))
}
