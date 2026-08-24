use super::{verify, CommandSpec, ProviderDescriptor, ProviderId};

pub const EXECUTABLE: &str = "copilot";
pub const INSTALL_GUIDANCE: &str = "Install GitHub Copilot CLI with `npm install -g @github/copilot` or `brew install --cask copilot-cli`; if it is already installed, make sure `copilot` is on PATH and run `copilot login` to authenticate, then retry `coven doctor`.";
pub const NON_INTERACTIVE_PREFIX_ARGS: &[&str] = &["--no-color"];
pub const PROMPT_FLAG: &str = "--prompt";

pub fn verification_prompt_arg(prompt: &str) -> String {
    format!("{PROMPT_FLAG}={prompt}")
}

pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor::new(ProviderId::Copilot, EXECUTABLE, INSTALL_GUIDANCE)
        .with_login(CommandSpec::new(["login"]))
        .with_verification(verify::command(ProviderId::Copilot))
}
