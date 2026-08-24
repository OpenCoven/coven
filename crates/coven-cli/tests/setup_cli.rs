use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context, Result};

#[path = "../src/setup/mod.rs"]
mod setup;

use setup::{claude, codex, copilot};
use setup::{
    render_human, run_setup, Clock, CommandSpec, Confirmer, ConsentDecision, ConsentRequest,
    ExecutableDiscovery, LaunchRequest, Outcome, ProcessExit, ProcessLauncher, ProviderDescriptor,
    ProviderId, ResolvedExecutable, RunningProcess, Selector, SetupError, SetupMode, SetupOptions,
    SetupRuntime, SystemClock, SystemExecutableDiscovery, SystemProcessLauncher, TerminalState,
};

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn test_tempdir() -> Result<tempfile::TempDir> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/setup-cli-tests");
    fs::create_dir_all(&root)?;
    Ok(tempfile::Builder::new().prefix("setup-").tempdir_in(root)?)
}

fn descriptor(id: ProviderId) -> ProviderDescriptor {
    ProviderDescriptor::new(
        id,
        format!("fake-{}", id.as_str()),
        "Install this CLI only from its official provider documentation.",
    )
    .with_login(CommandSpec::new(["login"]))
    .with_verification(CommandSpec::new(["verify"]))
}

fn options(selector: Selector) -> SetupOptions {
    SetupOptions {
        selector,
        mode: SetupMode::Login,
        timeout: Duration::from_secs(2),
        report_json: None,
        candidate_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
    }
}

#[test]
fn setup_cli_parses_all_selectors_and_verification_modes() -> Result<()> {
    for selector in ["codex", "claude", "copilot", "all"] {
        let output = Command::new(coven_bin())
            .args(["setup", selector, "--help"])
            .output()?;
        assert!(
            output.status.success(),
            "selector {selector} was rejected:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let invalid = Command::new(coven_bin())
        .args(["setup", "other"])
        .output()?;
    assert!(!invalid.status.success());

    let conflicting = Command::new(coven_bin())
        .args(["setup", "codex", "--verify", "--verify-only"])
        .output()?;
    assert!(!conflicting.status.success());

    let report_without_verification = Command::new(coven_bin())
        .args(["setup", "codex", "--report-json", "report.json"])
        .output()?;
    assert!(!report_without_verification.status.success());
    Ok(())
}

#[test]
fn setup_cli_refuses_non_tty_without_waiting_or_launching() -> Result<()> {
    let temp = test_tempdir()?;
    let output = Command::new(coven_bin())
        .args(["setup", "codex"])
        .env("COVEN_HOME", temp.path().join("home"))
        .stdin(Stdio::null())
        .output()?;

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "Codex: non_tty"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("Error:"),
        "non-TTY refusal should be a closed setup result, not an internal error"
    );
    Ok(())
}

#[test]
fn unregistered_provider_is_rejected_before_runtime_actions() -> Result<()> {
    let providers = [codex::descriptor()];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let error = run_setup(&options(Selector::Claude), &providers, &mut runtime)
        .expect_err("an unregistered provider must fail explicitly");

    assert!(matches!(error, SetupError::UnsupportedProvider));
    assert!(discovery.calls.borrow().is_empty());
    assert!(confirmer.requests.is_empty());
    assert!(launcher.launches.is_empty());
    Ok(())
}

#[test]
fn codex_provider_launches_exact_login_command() -> Result<()> {
    let provider = codex::descriptor();
    let providers = [provider];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([LaunchBehavior::exit(0)]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&options(Selector::Codex), &providers, &mut runtime)?;

    assert!(summary.completed());
    assert_eq!(confirmer.requests.len(), 1);
    assert_eq!(confirmer.requests[0].command, "codex login");
    assert_eq!(launcher.launches.len(), 1);
    assert_eq!(
        launcher.launches[0].executable,
        PathBuf::from("/fixture/codex")
    );
    assert_eq!(launcher.launches[0].args, vec![OsString::from("login")]);
    Ok(())
}

#[test]
fn codex_provider_preserves_failure_and_cancellation_outcomes() -> Result<()> {
    assert_eq!(
        run_codex(
            FixedTerminal(true),
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::exit(7),
        )?
        .0,
        Outcome::ProviderFailed
    );
    assert_eq!(
        run_codex(
            FixedTerminal(true),
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::signalled(),
        )?
        .0,
        Outcome::Cancelled
    );
    Ok(())
}

#[test]
fn codex_provider_handles_missing_declined_and_non_tty_without_launching() -> Result<()> {
    let (outcome, launches, rendered) = run_codex(
        FixedTerminal(true),
        None,
        ConsentDecision::Accepted,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::NotInstalled);
    assert_eq!(launches, 0);
    assert_eq!(
        rendered,
        format!("Codex: not_installed\n{}\n", codex::INSTALL_GUIDANCE)
    );

    let (outcome, launches, _) = run_codex(
        FixedTerminal(true),
        Some("1.2.3"),
        ConsentDecision::Declined,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::Declined);
    assert_eq!(launches, 0);

    let (outcome, launches, _) = run_codex(
        FixedTerminal(false),
        Some("1.2.3"),
        ConsentDecision::Accepted,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::NonTty);
    assert_eq!(launches, 0);
    Ok(())
}

#[test]
fn claude_provider_launches_exact_auth_login_command_and_never_doctor() -> Result<()> {
    let provider = claude::descriptor();
    let providers = [provider];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([LaunchBehavior::exit(0)]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&options(Selector::Claude), &providers, &mut runtime)?;

    assert!(summary.completed());
    assert_eq!(confirmer.requests.len(), 1);
    assert_eq!(confirmer.requests[0].command, "claude auth login");
    assert!(!confirmer.requests[0].command.contains("doctor"));
    assert_eq!(launcher.launches.len(), 1);
    assert_eq!(
        launcher.launches[0].executable,
        PathBuf::from("/fixture/claude")
    );
    assert_eq!(
        launcher.launches[0].args,
        vec![OsString::from("auth"), OsString::from("login")]
    );
    assert!(!launcher.launches[0]
        .args
        .iter()
        .any(|argument| argument == "doctor"));
    Ok(())
}

#[test]
fn claude_provider_preserves_failure_and_cancellation_outcomes() -> Result<()> {
    assert_eq!(
        run_claude(
            FixedTerminal(true),
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::exit(7),
        )?
        .0,
        Outcome::ProviderFailed
    );
    assert_eq!(
        run_claude(
            FixedTerminal(true),
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::signalled(),
        )?
        .0,
        Outcome::Cancelled
    );
    Ok(())
}

#[test]
fn claude_provider_handles_missing_declined_and_non_tty_without_launching() -> Result<()> {
    let (outcome, launches, rendered) = run_claude(
        FixedTerminal(true),
        None,
        ConsentDecision::Accepted,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::NotInstalled);
    assert_eq!(launches, 0);
    assert_eq!(
        rendered,
        format!("Claude Code: not_installed\n{}\n", claude::INSTALL_GUIDANCE)
    );
    assert!(rendered.contains("claude auth login"));
    assert!(!rendered.contains("claude doctor"));

    let (outcome, launches, _) = run_claude(
        FixedTerminal(true),
        Some("1.2.3"),
        ConsentDecision::Declined,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::Declined);
    assert_eq!(launches, 0);

    let (outcome, launches, _) = run_claude(
        FixedTerminal(false),
        Some("1.2.3"),
        ConsentDecision::Accepted,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::NonTty);
    assert_eq!(launches, 0);
    Ok(())
}

#[test]
fn copilot_provider_launches_exact_login_command() -> Result<()> {
    let provider = copilot::descriptor();
    let providers = [provider];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([LaunchBehavior::exit(0)]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&options(Selector::Copilot), &providers, &mut runtime)?;

    assert!(summary.completed());
    assert_eq!(confirmer.requests.len(), 1);
    assert_eq!(confirmer.requests[0].command, "copilot login");
    assert_eq!(launcher.launches.len(), 1);
    assert_eq!(
        launcher.launches[0].executable,
        PathBuf::from("/fixture/copilot")
    );
    assert_eq!(launcher.launches[0].args, vec![OsString::from("login")]);
    Ok(())
}

#[test]
fn copilot_provider_preserves_failure_and_cancellation_outcomes() -> Result<()> {
    assert_eq!(
        run_copilot(
            FixedTerminal(true),
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::exit(7),
        )?
        .0,
        Outcome::ProviderFailed
    );
    assert_eq!(
        run_copilot(
            FixedTerminal(true),
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::signalled(),
        )?
        .0,
        Outcome::Cancelled
    );
    Ok(())
}

#[test]
fn copilot_provider_handles_missing_declined_and_non_tty_without_launching() -> Result<()> {
    let (outcome, launches, rendered) = run_copilot(
        FixedTerminal(true),
        None,
        ConsentDecision::Accepted,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::NotInstalled);
    assert_eq!(launches, 0);
    assert_eq!(
        rendered,
        format!(
            "GitHub Copilot CLI: not_installed\n{}\n",
            copilot::INSTALL_GUIDANCE
        )
    );
    assert!(rendered.contains("copilot login"));

    let (outcome, launches, _) = run_copilot(
        FixedTerminal(true),
        Some("1.2.3"),
        ConsentDecision::Declined,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::Declined);
    assert_eq!(launches, 0);

    let (outcome, launches, _) = run_copilot(
        FixedTerminal(false),
        Some("1.2.3"),
        ConsentDecision::Accepted,
        LaunchBehavior::exit(0),
    )?;
    assert_eq!(outcome, Outcome::NonTty);
    assert_eq!(launches, 0);
    Ok(())
}

#[test]
fn all_discovers_and_confirms_each_provider_once_before_launch() -> Result<()> {
    let providers = [
        descriptor(ProviderId::Codex),
        descriptor(ProviderId::Claude),
        descriptor(ProviderId::Copilot),
    ];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([
        ConsentDecision::Accepted,
        ConsentDecision::Accepted,
        ConsentDecision::Accepted,
    ]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([
        LaunchBehavior::exit(0),
        LaunchBehavior::exit(0),
        LaunchBehavior::exit(0),
    ]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&options(Selector::All), &providers, &mut runtime)?;

    assert!(summary.completed());
    assert_eq!(
        summary
            .results
            .iter()
            .map(|result| (result.provider, result.outcome))
            .collect::<Vec<_>>(),
        vec![
            (ProviderId::Codex, Outcome::Completed),
            (ProviderId::Claude, Outcome::Completed),
            (ProviderId::Copilot, Outcome::Completed),
        ]
    );
    assert_eq!(
        discovery.calls.borrow().as_slice(),
        &[ProviderId::Codex, ProviderId::Claude, ProviderId::Copilot]
    );
    assert_eq!(confirmer.requests.len(), 3);
    assert_eq!(launcher.launches.len(), 3);
    for ((request, launch), provider) in confirmer.requests.iter().zip(&launcher.launches).zip([
        ProviderId::Codex,
        ProviderId::Claude,
        ProviderId::Copilot,
    ]) {
        assert_eq!(request.provider, provider);
        assert_eq!(request.command, format!("fake-{} login", provider.as_str()));
        assert_eq!(launch.args, vec![OsString::from("login")]);
    }
    Ok(())
}

#[test]
fn verification_after_login_requires_a_second_explicit_consent() -> Result<()> {
    let providers = [descriptor(ProviderId::Codex)];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted, ConsentDecision::Accepted]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([LaunchBehavior::exit(0), LaunchBehavior::exit(0)]);
    let mut setup_options = options(Selector::Codex);
    setup_options.mode = SetupMode::LoginAndVerify;
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&setup_options, &providers, &mut runtime)?;

    assert!(summary.completed());
    assert_eq!(confirmer.requests.len(), 2);
    assert_eq!(confirmer.requests[0].action, setup::SetupAction::Login);
    assert_eq!(
        confirmer.requests[1].action,
        setup::SetupAction::Verification
    );
    assert!(
        confirmer.requests[1].notice.contains("network access")
            && confirmer.requests[1].notice.contains("usage or cost"),
        "verification consent must disclose network and cost: {:?}",
        confirmer.requests[1]
    );
    assert_eq!(launcher.launches.len(), 2);
    Ok(())
}

#[test]
fn verify_only_uses_adapter_commands_and_ephemeral_coven_state() -> Result<()> {
    let providers = [
        codex::descriptor(),
        claude::descriptor(),
        copilot::descriptor(),
    ];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([
        ConsentDecision::Accepted,
        ConsentDecision::Accepted,
        ConsentDecision::Accepted,
    ]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([
        LaunchBehavior::exit(0),
        LaunchBehavior::exit(0),
        LaunchBehavior::exit(0),
    ]);
    let mut setup_options = options(Selector::All);
    setup_options.mode = SetupMode::VerifyOnly;
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&setup_options, &providers, &mut runtime)?;

    assert!(summary.completed());
    assert_eq!(
        confirmer
            .requests
            .iter()
            .map(|request| request.command.as_str())
            .collect::<Vec<_>>(),
        vec![
            "codex exec --skip-git-repo-check --color never -- Reply with OK.",
            "claude --print -- Reply with OK.",
            "copilot --no-color --prompt=Reply with OK.",
        ]
    );
    assert_eq!(launcher.launches.len(), 3);
    for launch in &launcher.launches {
        let cwd = launch
            .current_dir
            .as_deref()
            .context("verification must use an ephemeral working directory")?;
        let coven_home = launch
            .env_overrides
            .iter()
            .find_map(|(name, value)| (name == "COVEN_HOME").then_some(value.as_deref()).flatten())
            .context("verification must isolate COVEN_HOME")?;
        assert!(PathBuf::from(coven_home).starts_with(cwd));
        assert!(
            !cwd.exists(),
            "ephemeral verification state should be removed after the provider exits: {cwd:?}"
        );
    }
    Ok(())
}

#[test]
fn missing_executable_prints_only_the_descriptor_official_guidance() -> Result<()> {
    let provider = descriptor(ProviderId::Codex);
    let providers = [provider.clone()];
    let discovery = FixedDiscovery(None);
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&options(Selector::Codex), &providers, &mut runtime)?;
    let mut rendered = Vec::new();
    render_human(&summary, &mut rendered)?;
    assert_eq!(
        String::from_utf8(rendered)?,
        concat!(
            "Codex: not_installed\n",
            "Install this CLI only from its official provider documentation.\n"
        )
    );
    assert!(confirmer.requests.is_empty());
    assert!(launcher.launches.is_empty());
    Ok(())
}

#[test]
fn decline_and_non_tty_never_launch_a_provider() -> Result<()> {
    let provider = descriptor(ProviderId::Codex);
    let providers = [provider.clone()];

    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Declined]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let declined = run_setup(&options(Selector::Codex), &providers, &mut runtime)?;
    assert_eq!(declined.results[0].outcome, Outcome::Declined);
    assert!(launcher.launches.is_empty());

    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(false);
    let mut confirmer = FakeConfirmer::new([]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let non_tty = run_setup(&options(Selector::Codex), &providers, &mut runtime)?;
    assert_eq!(non_tty.results[0].outcome, Outcome::NonTty);
    assert!(discovery.calls.borrow().is_empty());
    assert!(confirmer.requests.is_empty());
    assert!(launcher.launches.is_empty());
    Ok(())
}

#[test]
fn setup_classifies_the_closed_outcome_set() -> Result<()> {
    assert_eq!(
        run_single(
            SetupMode::Login,
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::exit(0),
        )?,
        Outcome::Completed
    );
    assert_eq!(
        run_single(
            SetupMode::Login,
            None,
            ConsentDecision::Accepted,
            LaunchBehavior::exit(0),
        )?,
        Outcome::NotInstalled
    );
    assert_eq!(
        run_single(
            SetupMode::Login,
            Some("1.2.3"),
            ConsentDecision::Declined,
            LaunchBehavior::exit(0),
        )?,
        Outcome::Declined
    );
    assert_eq!(
        run_single(
            SetupMode::Login,
            Some("1.2.3"),
            ConsentDecision::Cancelled,
            LaunchBehavior::exit(0),
        )?,
        Outcome::Cancelled
    );
    assert_eq!(
        run_single(
            SetupMode::Login,
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::exit(7),
        )?,
        Outcome::ProviderFailed
    );
    assert_eq!(
        run_single(
            SetupMode::Login,
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::signalled(),
        )?,
        Outcome::Cancelled
    );
    assert_eq!(
        run_single(
            SetupMode::VerifyOnly,
            Some("1.2.3"),
            ConsentDecision::Accepted,
            LaunchBehavior::exit(9),
        )?,
        Outcome::VerificationFailed
    );
    Ok(())
}

#[test]
fn timeout_uses_the_injected_deadline_and_cleans_the_process_tree() -> Result<()> {
    let provider = descriptor(ProviderId::Codex);
    let providers = [provider];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = FakeClock::default();
    let terminated = Rc::new(Cell::new(false));
    let mut launcher = FakeLauncher::new([LaunchBehavior::never(Rc::clone(&terminated))]);
    let mut setup_options = options(Selector::Codex);
    setup_options.timeout = Duration::from_millis(25);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&setup_options, &providers, &mut runtime)?;

    assert_eq!(summary.results[0].outcome, Outcome::TimedOut);
    assert!(terminated.get(), "timed out child tree was not terminated");
    assert!(clock.now() >= Duration::from_millis(25));
    Ok(())
}

#[cfg(unix)]
#[test]
fn system_launcher_timeout_reaps_descendants() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let temp = test_tempdir()?;
    let pid_file = temp.path().join("descendant.pid");
    let provider = ProviderDescriptor::new(
        ProviderId::Codex,
        "sh",
        "Install the shell from its official source.",
    )
    .with_login(CommandSpec::new([
        "-c",
        "sleep 30 & child=$!; printf '%s' \"$child\" > \"$1\"; wait \"$child\"",
        "setup-timeout",
        pid_file.to_str().context("test path must be valid UTF-8")?,
    ]));
    let providers = [provider];
    let discovery = FixedDiscovery(Some(ResolvedExecutable {
        path: PathBuf::from("/bin/sh"),
        version: Some("1.0.0".to_owned()),
    }));
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = SystemClock::new();
    let mut launcher = SystemProcessLauncher;
    let mut setup_options = options(Selector::Codex);
    setup_options.timeout = Duration::from_millis(250);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&setup_options, &providers, &mut runtime)?;
    assert_eq!(summary.results[0].outcome, Outcome::TimedOut);
    let pid = fs::read_to_string(&pid_file)?
        .trim()
        .parse::<u32>()
        .context("descendant pid")?;
    wait_for_process_exit(pid)?;

    let mode = fs::metadata(temp.path())?.permissions().mode();
    assert_ne!(mode & 0o700, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn inherited_terminal_io_stays_separate_from_the_redacted_report() -> Result<()> {
    let temp = test_tempdir()?;
    let stdout_path = temp.path().join("terminal.stdout");
    let stderr_path = temp.path().join("terminal.stderr");
    let report_path = temp.path().join("report.json");
    let private_path = temp.path().join("private");
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "inherited_terminal_and_report_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("COVEN_SETUP_INHERITED_HELPER", "1")
        .env("COVEN_SETUP_PRIVATE_PATH", &private_path)
        .env("COVEN_SETUP_REPORT_PATH", &report_path)
        .stdin(Stdio::piped())
        .stdout(fs::File::create(&stdout_path)?)
        .stderr(fs::File::create(&stderr_path)?)
        .spawn()?;
    child
        .stdin
        .as_mut()
        .context("helper stdin")?
        .write_all(b"terminal-input\n")?;
    drop(child.stdin.take());
    let status = child.wait()?;
    assert!(status.success(), "helper failed with {status}");

    let stdout = fs::read_to_string(stdout_path)?;
    let stderr = fs::read_to_string(stderr_path)?;
    assert!(
        stdout.contains("provider-stdout:terminal-input"),
        "{stdout:?}"
    );
    assert!(
        stderr.contains(&format!(
            "oauth account token model {}",
            private_path.display()
        )),
        "{stderr:?}"
    );

    let report_bytes = fs::read(&report_path)?;
    let report: serde_json::Value = serde_json::from_slice(&report_bytes)?;
    assert_eq!(report["harness"], "codex");
    assert_eq!(report["exit_class"], "completed");
    assert_eq!(report["completed"], true);
    let report_text = String::from_utf8(report_bytes)?;
    for forbidden in [
        "provider-stdout",
        "oauth",
        "account",
        "token",
        "model",
        "\u{1b}",
    ] {
        assert!(
            !report_text.contains(forbidden),
            "report leaked {forbidden:?}: {report_text}"
        );
    }
    assert!(
        !report_text.contains(&private_path.to_string_lossy().into_owned()),
        "report leaked the private path: {report_text}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn inherited_terminal_and_report_helper() -> Result<()> {
    if std::env::var_os("COVEN_SETUP_INHERITED_HELPER").is_none() {
        return Ok(());
    }
    let report_path =
        PathBuf::from(std::env::var_os("COVEN_SETUP_REPORT_PATH").context("report path")?);
    std::env::var_os("COVEN_SETUP_PRIVATE_PATH").context("private path")?;
    let provider = ProviderDescriptor::new(
        ProviderId::Codex,
        "sh",
        "Install the shell from its official source.",
    )
    .with_verification(CommandSpec::new([
        "-c",
        concat!(
            "IFS= read -r line; ",
            "printf 'provider-stdout:%s\\n' \"$line\"; ",
            "printf 'provider-stderr:oauth account token model %s \\033[31m\\n' ",
            "\"$COVEN_SETUP_PRIVATE_PATH\" >&2"
        ),
    ]));
    let providers = [provider];
    let discovery = FixedDiscovery(Some(ResolvedExecutable {
        path: PathBuf::from("/bin/sh"),
        version: Some("1.2.3".to_owned()),
    }));
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = SystemClock::new();
    let mut launcher = SystemProcessLauncher;
    let mut setup_options = options(Selector::Codex);
    setup_options.mode = SetupMode::VerifyOnly;
    setup_options.report_json = Some(report_path);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&setup_options, &providers, &mut runtime)?;
    render_human(&summary, &mut io::stdout().lock())?;
    assert!(summary.completed());
    Ok(())
}

#[cfg(unix)]
#[test]
fn copilot_fixture_inherits_terminal_streams_and_records_exact_login() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let temp = test_tempdir()?;
    let fake_copilot = temp.path().join("copilot");
    let args_path = temp.path().join("args.txt");
    let stdout_path = temp.path().join("terminal.stdout");
    let stderr_path = temp.path().join("terminal.stderr");
    fs::write(
        &fake_copilot,
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$@\" > \"$COVEN_SETUP_ARGS_PATH\"\n",
            "IFS= read -r line\n",
            "printf 'copilot-stdout:%s\\n' \"$line\"\n",
            "printf 'copilot-stderr:%s\\n' \"$line\" >&2\n",
        ),
    )?;
    let mut permissions = fs::metadata(&fake_copilot)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_copilot, permissions)?;

    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "copilot_inherited_terminal_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("COVEN_SETUP_COPILOT_HELPER", "1")
        .env("COVEN_SETUP_COPILOT_PATH", &fake_copilot)
        .env("COVEN_SETUP_ARGS_PATH", &args_path)
        .stdin(Stdio::piped())
        .stdout(fs::File::create(&stdout_path)?)
        .stderr(fs::File::create(&stderr_path)?)
        .spawn()?;
    child
        .stdin
        .as_mut()
        .context("helper stdin")?
        .write_all(b"terminal-input\n")?;
    drop(child.stdin.take());
    let status = child.wait()?;
    assert!(status.success(), "helper failed with {status}");

    assert_eq!(fs::read_to_string(args_path)?, "login\n");
    assert!(
        fs::read_to_string(stdout_path)?.contains("copilot-stdout:terminal-input"),
        "Copilot stdout should remain attached to the inherited terminal"
    );
    assert!(
        fs::read_to_string(stderr_path)?.contains("copilot-stderr:terminal-input"),
        "Copilot stderr should remain attached to the inherited terminal"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn copilot_inherited_terminal_helper() -> Result<()> {
    if std::env::var_os("COVEN_SETUP_COPILOT_HELPER").is_none() {
        return Ok(());
    }
    let executable =
        PathBuf::from(std::env::var_os("COVEN_SETUP_COPILOT_PATH").context("Copilot path")?);
    let provider = copilot::descriptor();
    let providers = [provider];
    let discovery = FixedDiscovery(Some(ResolvedExecutable {
        path: executable,
        version: Some("1.2.3".to_owned()),
    }));
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = SystemClock::new();
    let mut launcher = SystemProcessLauncher;
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&options(Selector::Copilot), &providers, &mut runtime)?;

    assert!(summary.completed());
    Ok(())
}

#[cfg(unix)]
#[test]
fn verify_ephemeral_state_reaches_provider_then_is_removed() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let temp = test_tempdir()?;
    let fake_codex = temp.path().join("codex");
    let observation_path = temp.path().join("verification-state.txt");
    let args_path = temp.path().join("verification-args.txt");
    let report_path = temp.path().join("verification-report.json");
    let stdout_path = temp.path().join("verification.stdout");
    let stderr_path = temp.path().join("verification.stderr");
    fs::write(
        &fake_codex,
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n%s\\n' \"$PWD\" \"$COVEN_HOME\" > \"$COVEN_SETUP_STATE_PATH\"\n",
            "printf '%s\\n' \"$@\" > \"$COVEN_SETUP_ARGS_PATH\"\n",
            "printf 'provider-stdout:oauth token private-path=%s\\n' \"$COVEN_SETUP_STATE_PATH\"\n",
            "printf 'provider-stderr:account model bearer=%s\\n' \"$COVEN_SETUP_STATE_PATH\" >&2\n",
        ),
    )?;
    let mut permissions = fs::metadata(&fake_codex)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions)?;

    let status = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "verify_ephemeral_state_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("COVEN_SETUP_VERIFY_HELPER", "1")
        .env("COVEN_SETUP_VERIFY_EXECUTABLE", &fake_codex)
        .env("COVEN_SETUP_STATE_PATH", &observation_path)
        .env("COVEN_SETUP_ARGS_PATH", &args_path)
        .env("COVEN_SETUP_REPORT_PATH", &report_path)
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout_path)?)
        .stderr(fs::File::create(&stderr_path)?)
        .status()?;
    assert!(
        status.success(),
        "verification helper failed with {status}\nstdout:\n{}\nstderr:\n{}",
        fs::read_to_string(&stdout_path).unwrap_or_default(),
        fs::read_to_string(&stderr_path).unwrap_or_default(),
    );

    let observation = fs::read_to_string(&observation_path)?;
    let mut lines = observation.lines();
    let current_dir = PathBuf::from(lines.next().context("recorded working directory")?);
    let coven_home = PathBuf::from(lines.next().context("recorded COVEN_HOME")?);
    let expected_coven_home = current_dir.join("coven-home");
    let expected_coven_home = expected_coven_home.to_string_lossy();
    assert_eq!(
        coven_home.to_string_lossy(),
        expected_coven_home
            .strip_prefix("/private")
            .unwrap_or(&expected_coven_home),
    );
    assert!(
        !current_dir.exists(),
        "ephemeral verification state should be removed: {current_dir:?}"
    );
    assert_eq!(
        fs::read_to_string(args_path)?,
        concat!(
            "exec\n",
            "--skip-git-repo-check\n",
            "--color\n",
            "never\n",
            "--\n",
            "Reply with OK.\n"
        )
    );

    let stdout = fs::read_to_string(stdout_path)?;
    let stderr = fs::read_to_string(stderr_path)?;
    assert!(stdout.contains("provider-stdout:oauth token private-path="));
    assert!(stderr.contains("provider-stderr:account model bearer="));
    let report = fs::read_to_string(report_path)?;
    for forbidden in [
        "provider-stdout",
        "provider-stderr",
        "oauth",
        "account",
        "token",
        "model",
        "bearer",
        &observation_path.to_string_lossy(),
        &current_dir.to_string_lossy(),
    ] {
        assert!(
            !report.contains(forbidden),
            "verification report leaked {forbidden:?}: {report}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn verification_fails_when_ephemeral_state_cannot_be_removed() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let provider = codex::descriptor();
    let providers = [provider];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = FakeClock::default();
    let blocked_root = Rc::new(RefCell::new(None));
    let mut launcher = CleanupBlockingLauncher {
        blocked_root: Rc::clone(&blocked_root),
    };
    let mut setup_options = options(Selector::Codex);
    setup_options.mode = SetupMode::VerifyOnly;
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&setup_options, &providers, &mut runtime)?;

    assert_eq!(summary.results[0].outcome, Outcome::VerificationFailed);
    let root = blocked_root
        .borrow()
        .clone()
        .context("verification working directory")?;
    let mut permissions = fs::metadata(&root)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&root, permissions)?;
    fs::remove_dir_all(root)?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn verify_ephemeral_state_helper() -> Result<()> {
    if std::env::var_os("COVEN_SETUP_VERIFY_HELPER").is_none() {
        return Ok(());
    }
    let executable = PathBuf::from(
        std::env::var_os("COVEN_SETUP_VERIFY_EXECUTABLE").context("verification executable")?,
    );
    let report_path =
        PathBuf::from(std::env::var_os("COVEN_SETUP_REPORT_PATH").context("report path")?);
    let providers = [codex::descriptor()];
    let discovery = FixedDiscovery(Some(ResolvedExecutable {
        path: executable,
        version: Some("1.2.3".to_owned()),
    }));
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = SystemClock::new();
    let mut launcher = SystemProcessLauncher;
    let mut setup_options = options(Selector::Codex);
    setup_options.mode = SetupMode::VerifyOnly;
    setup_options.report_json = Some(report_path);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };

    let summary = run_setup(&setup_options, &providers, &mut runtime)?;

    assert!(summary.completed());
    Ok(())
}

#[test]
fn report_publication_is_fail_if_exists_atomic_and_redaction_closed() -> Result<()> {
    let temp = test_tempdir()?;
    let existing = temp.path().join("existing.json");
    fs::write(&existing, b"keep-me")?;
    let provider = descriptor(ProviderId::Codex);
    let providers = [provider.clone()];
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([LaunchBehavior::exit(0)]);
    let mut setup_options = options(Selector::Codex);
    setup_options.mode = SetupMode::VerifyOnly;
    setup_options.report_json = Some(existing.clone());
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let error = run_setup(&setup_options, &providers, &mut runtime)
        .expect_err("an existing report must be refused");
    assert!(matches!(error, SetupError::Exists));
    assert_eq!(fs::read(&existing)?, b"keep-me");
    assert!(confirmer.requests.is_empty());
    assert!(launcher.launches.is_empty());

    let rejected = temp.path().join("rejected.json");
    let rejected_version = format!(
        "oauth account token model \u{1b}[31m {}",
        temp.path().join("private").display()
    );
    let discovery = FakeDiscovery::all_present(&providers, &rejected_version);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let mut launcher = FakeLauncher::new([LaunchBehavior::exit(0)]);
    setup_options.report_json = Some(rejected.clone());
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let error = run_setup(&setup_options, &providers, &mut runtime)
        .expect_err("unsafe report data must fail closed");
    assert!(matches!(error, SetupError::Rejected));
    assert!(!rejected.exists());

    let unresolved = temp.path().join("unresolved.json");
    let discovery = FixedDiscovery(Some(ResolvedExecutable {
        path: PathBuf::from("fake-codex"),
        version: None,
    }));
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let mut launcher = FakeLauncher::new([LaunchBehavior::exit(0)]);
    setup_options.report_json = Some(unresolved.clone());
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let error = run_setup(&setup_options, &providers, &mut runtime)
        .expect_err("certification requires a resolved CLI version");
    assert!(matches!(error, SetupError::Rejected));
    assert!(!unresolved.exists());

    let published = temp.path().join("published.json");
    let discovery = FakeDiscovery::all_present(&providers, "1.2.3");
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let mut launcher = FakeLauncher::new([LaunchBehavior::exit(0)]);
    setup_options.report_json = Some(published.clone());
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let summary = run_setup(&setup_options, &providers, &mut runtime)?;
    assert!(summary.completed());
    let parsed: serde_json::Value = serde_json::from_slice(&fs::read(&published)?)?;
    assert_eq!(parsed["cli_version"], "1.2.3");
    let mut result_fields = parsed
        .as_object()
        .context("flat report result object")?
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    result_fields.sort_unstable();
    assert_eq!(
        result_fields,
        [
            "candidate_commit",
            "cli_version",
            "completed",
            "duration",
            "exit_class",
            "harness",
            "platform",
        ]
    );
    let entries = fs::read_dir(temp.path())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    assert!(
        entries
            .iter()
            .all(|name| !name.to_string_lossy().contains(".tmp")),
        "atomic report staging file was left behind: {entries:?}"
    );
    Ok(())
}

#[test]
fn executable_discovery_is_injectable_and_rejects_non_executables() -> Result<()> {
    let temp = test_tempdir()?;
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin)?;
    let executable = bin.join(if cfg!(windows) {
        "fake-cli.exe"
    } else {
        "fake-cli"
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&executable, b"#!/bin/sh\nprintf 'fake-cli 1.2.3\\n'\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    fs::write(&executable, b"fixture")?;
    #[cfg(windows)]
    fs::write(bin.join("fake-cli"), b"npm shell shim")?;
    let path = std::env::join_paths([&bin])?;
    let discovery = SystemExecutableDiscovery::from_path(Some(path), Some(OsString::from(".EXE")));
    let _environment_discovery = SystemExecutableDiscovery::from_environment();
    let _system_confirmer = setup::SystemConfirmer;
    let _system_terminal = setup::SystemTerminalState;
    let provider =
        ProviderDescriptor::new(ProviderId::Codex, "fake-cli", "Official install guidance.");
    let resolved = discovery
        .discover(&provider)?
        .context("executable should be discovered")?;
    #[cfg(windows)]
    assert!(resolved
        .path
        .to_string_lossy()
        .eq_ignore_ascii_case(&executable.to_string_lossy()));
    #[cfg(not(windows))]
    assert_eq!(resolved.path, executable);
    #[cfg(unix)]
    assert_eq!(resolved.version.as_deref(), Some("1.2.3"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&resolved.path, fs::Permissions::from_mode(0o644))?;
        assert!(discovery.discover(&provider)?.is_none());
    }
    Ok(())
}

#[test]
fn human_summary_never_echoes_paths_launch_errors_or_terminal_bytes() -> Result<()> {
    let temp = test_tempdir()?;
    let private_path = temp.path().join("private");
    let provider = descriptor(ProviderId::Codex);
    let providers = [provider];
    let discovery = FixedDiscovery(Some(ResolvedExecutable {
        path: private_path.join("oauth-token"),
        version: Some("1.2.3".to_owned()),
    }));
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([ConsentDecision::Accepted]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([LaunchBehavior::Error(format!(
        "account token model \u{1b}[31m {}",
        private_path.display()
    ))]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let summary = run_setup(&options(Selector::Codex), &providers, &mut runtime)?;
    let mut rendered = Vec::new();
    render_human(&summary, &mut rendered)?;
    let rendered = String::from_utf8(rendered)?;
    assert_eq!(rendered, "Codex: provider_failed\n");
    let debug = format!("{summary:?}");
    assert!(!debug.contains("oauth-token"), "{debug}");
    assert!(
        !debug.contains(&private_path.to_string_lossy().into_owned()),
        "{debug}"
    );
    Ok(())
}

fn run_single(
    mode: SetupMode,
    version: Option<&str>,
    consent: ConsentDecision,
    behavior: LaunchBehavior,
) -> Result<Outcome> {
    let provider = descriptor(ProviderId::Codex);
    let providers = [provider];
    let discovery = FixedDiscovery(version.map(|version| ResolvedExecutable {
        path: PathBuf::from("fake-codex"),
        version: Some(version.to_owned()),
    }));
    let terminal = FixedTerminal(true);
    let mut confirmer = FakeConfirmer::new([consent]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([behavior]);
    let mut setup_options = options(Selector::Codex);
    setup_options.mode = mode;
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    Ok(run_setup(&setup_options, &providers, &mut runtime)?.results[0].outcome)
}

fn run_codex(
    terminal: FixedTerminal,
    version: Option<&str>,
    consent: ConsentDecision,
    behavior: LaunchBehavior,
) -> Result<(Outcome, usize, String)> {
    let provider = codex::descriptor();
    let providers = [provider];
    let discovery = FixedDiscovery(version.map(|version| ResolvedExecutable {
        path: PathBuf::from("fake-codex"),
        version: Some(version.to_owned()),
    }));
    let mut confirmer = FakeConfirmer::new([consent]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([behavior]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let summary = run_setup(&options(Selector::Codex), &providers, &mut runtime)?;
    let mut rendered = Vec::new();
    render_human(&summary, &mut rendered)?;
    Ok((
        summary.results[0].outcome,
        launcher.launches.len(),
        String::from_utf8(rendered)?,
    ))
}

fn run_claude(
    terminal: FixedTerminal,
    version: Option<&str>,
    consent: ConsentDecision,
    behavior: LaunchBehavior,
) -> Result<(Outcome, usize, String)> {
    let provider = claude::descriptor();
    let providers = [provider];
    let discovery = FixedDiscovery(version.map(|version| ResolvedExecutable {
        path: PathBuf::from("fake-claude"),
        version: Some(version.to_owned()),
    }));
    let mut confirmer = FakeConfirmer::new([consent]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([behavior]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let summary = run_setup(&options(Selector::Claude), &providers, &mut runtime)?;
    let mut rendered = Vec::new();
    render_human(&summary, &mut rendered)?;
    Ok((
        summary.results[0].outcome,
        launcher.launches.len(),
        String::from_utf8(rendered)?,
    ))
}

fn run_copilot(
    terminal: FixedTerminal,
    version: Option<&str>,
    consent: ConsentDecision,
    behavior: LaunchBehavior,
) -> Result<(Outcome, usize, String)> {
    let provider = copilot::descriptor();
    let providers = [provider];
    let discovery = FixedDiscovery(version.map(|version| ResolvedExecutable {
        path: PathBuf::from("fake-copilot"),
        version: Some(version.to_owned()),
    }));
    let mut confirmer = FakeConfirmer::new([consent]);
    let clock = FakeClock::default();
    let mut launcher = FakeLauncher::new([behavior]);
    let mut runtime = SetupRuntime {
        discovery: &discovery,
        confirmer: &mut confirmer,
        terminal: &terminal,
        clock: &clock,
        launcher: &mut launcher,
    };
    let summary = run_setup(&options(Selector::Copilot), &providers, &mut runtime)?;
    let mut rendered = Vec::new();
    render_human(&summary, &mut rendered)?;
    Ok((
        summary.results[0].outcome,
        launcher.launches.len(),
        String::from_utf8(rendered)?,
    ))
}

struct FixedTerminal(bool);

impl TerminalState for FixedTerminal {
    fn is_interactive(&self) -> bool {
        self.0
    }
}

struct FixedDiscovery(Option<ResolvedExecutable>);

impl ExecutableDiscovery for FixedDiscovery {
    fn discover(&self, _provider: &ProviderDescriptor) -> io::Result<Option<ResolvedExecutable>> {
        Ok(self.0.clone())
    }
}

struct FakeDiscovery {
    entries: Vec<(ProviderId, Option<ResolvedExecutable>)>,
    calls: RefCell<Vec<ProviderId>>,
}

impl FakeDiscovery {
    fn all_present(providers: &[ProviderDescriptor], version: &str) -> Self {
        Self {
            entries: providers
                .iter()
                .map(|provider| {
                    (
                        provider.id,
                        Some(ResolvedExecutable {
                            path: PathBuf::from(format!("/fixture/{}", provider.executable)),
                            version: Some(version.to_owned()),
                        }),
                    )
                })
                .collect(),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl ExecutableDiscovery for FakeDiscovery {
    fn discover(&self, provider: &ProviderDescriptor) -> io::Result<Option<ResolvedExecutable>> {
        self.calls.borrow_mut().push(provider.id);
        Ok(self
            .entries
            .iter()
            .find(|(id, _)| *id == provider.id)
            .and_then(|(_, executable)| executable.clone()))
    }
}

struct FakeConfirmer {
    decisions: VecDeque<ConsentDecision>,
    requests: Vec<ConsentRequest>,
}

impl FakeConfirmer {
    fn new(decisions: impl IntoIterator<Item = ConsentDecision>) -> Self {
        Self {
            decisions: decisions.into_iter().collect(),
            requests: Vec::new(),
        }
    }
}

impl Confirmer for FakeConfirmer {
    fn confirm(&mut self, request: &ConsentRequest) -> io::Result<ConsentDecision> {
        self.requests.push(request.clone());
        Ok(self
            .decisions
            .pop_front()
            .unwrap_or(ConsentDecision::Cancelled))
    }
}

#[derive(Default)]
struct FakeClock {
    now: Cell<Duration>,
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.now.get()
    }

    fn sleep(&self, duration: Duration) {
        self.now.set(self.now.get().saturating_add(duration));
    }
}

enum LaunchBehavior {
    Exit {
        polls_before_exit: usize,
        exit: ProcessExit,
        terminated: Rc<Cell<bool>>,
    },
    Error(String),
}

impl LaunchBehavior {
    fn exit(code: i32) -> Self {
        Self::Exit {
            polls_before_exit: 0,
            exit: ProcessExit::Exited(code),
            terminated: Rc::new(Cell::new(false)),
        }
    }

    fn signalled() -> Self {
        Self::Exit {
            polls_before_exit: 0,
            exit: ProcessExit::Signalled,
            terminated: Rc::new(Cell::new(false)),
        }
    }

    fn never(terminated: Rc<Cell<bool>>) -> Self {
        Self::Exit {
            polls_before_exit: usize::MAX,
            exit: ProcessExit::Signalled,
            terminated,
        }
    }
}

struct FakeLauncher {
    behaviors: VecDeque<LaunchBehavior>,
    launches: Vec<LaunchRequest>,
}

impl FakeLauncher {
    fn new(behaviors: impl IntoIterator<Item = LaunchBehavior>) -> Self {
        Self {
            behaviors: behaviors.into_iter().collect(),
            launches: Vec::new(),
        }
    }
}

#[cfg(unix)]
struct CleanupBlockingLauncher {
    blocked_root: Rc<RefCell<Option<PathBuf>>>,
}

#[cfg(unix)]
impl ProcessLauncher for CleanupBlockingLauncher {
    fn launch(&mut self, request: &LaunchRequest) -> io::Result<Box<dyn RunningProcess>> {
        use std::os::unix::fs::PermissionsExt;

        let root = request
            .current_dir
            .clone()
            .ok_or_else(|| io::Error::other("verification working directory was not set"))?;
        let mut permissions = fs::metadata(&root)?.permissions();
        permissions.set_mode(0o500);
        fs::set_permissions(&root, permissions)?;
        self.blocked_root.replace(Some(root));
        Ok(Box::new(FakeProcess {
            polls_before_exit: 0,
            exit: ProcessExit::Exited(0),
            terminated: Rc::new(Cell::new(false)),
        }))
    }
}

impl ProcessLauncher for FakeLauncher {
    fn launch(&mut self, request: &LaunchRequest) -> io::Result<Box<dyn RunningProcess>> {
        self.launches.push(request.clone());
        match self
            .behaviors
            .pop_front()
            .unwrap_or_else(|| LaunchBehavior::exit(0))
        {
            LaunchBehavior::Exit {
                polls_before_exit,
                exit,
                terminated,
            } => Ok(Box::new(FakeProcess {
                polls_before_exit,
                exit,
                terminated,
            })),
            LaunchBehavior::Error(message) => Err(io::Error::other(message)),
        }
    }
}

struct FakeProcess {
    polls_before_exit: usize,
    exit: ProcessExit,
    terminated: Rc<Cell<bool>>,
}

impl RunningProcess for FakeProcess {
    fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        if self.polls_before_exit == 0 {
            Ok(Some(self.exit))
        } else {
            self.polls_before_exit = self.polls_before_exit.saturating_sub(1);
            Ok(None)
        }
    }

    fn terminate_tree(&mut self) -> io::Result<()> {
        self.terminated.set(true);
        Ok(())
    }

    fn wait(&mut self) -> io::Result<ProcessExit> {
        Ok(self.exit)
    }
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn wait_for_process_exit(pid: u32) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while process_is_alive(pid) {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "timed out waiting for descendant {pid} to exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
