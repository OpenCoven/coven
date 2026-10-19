//! Private standalone helper; no target/private text is written to diagnostics.
fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    {
        // This hook is local to the helper binary, never Coven's production host.
        coven_conformance_supervisor::panic_site::install_hook();
        let args: Vec<_> = std::env::args_os().skip(1).take(3).collect();
        if args.len() != 2 {
            return std::process::ExitCode::from(2);
        }
        let pipes = match coven_conformance_supervisor::lease::native::inherited_pipes() {
            Ok(pipes) => pipes,
            Err(_) => return std::process::ExitCode::from(2),
        };
        std::process::ExitCode::from(coven_conformance_supervisor::coordinator::run(
            args[0].clone(),
            args[1].clone(),
            pipes,
        ))
    }
    #[cfg(not(windows))]
    std::process::ExitCode::from(2)
}
