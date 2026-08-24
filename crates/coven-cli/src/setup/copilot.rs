use super::{CommandSpec, ProviderDescriptor, ProviderId};

pub const EXECUTABLE: &str = "copilot";
pub const INSTALL_GUIDANCE: &str = "Install GitHub Copilot CLI with `npm install -g @github/copilot` or `brew install --cask copilot-cli`; if it is already installed, make sure `copilot` is on PATH and run `copilot login` to authenticate, then retry `coven doctor`.";

pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor::new(ProviderId::Copilot, EXECUTABLE, INSTALL_GUIDANCE)
        .with_login(CommandSpec::new(["login"]))
}
