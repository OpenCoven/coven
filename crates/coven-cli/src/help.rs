use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Write};

use anyhow::{anyhow, Context, Result};
use clap::{Command, CommandFactory, Parser};
use serde::Serialize;

pub(crate) const TOP_LEVEL_HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}

Arguments:
{positionals}

Options:
{options}
{after-help}";

pub(crate) const TOP_LEVEL_AFTER_HELP: &str = "\
Core commands:
  doctor    Check local setup and print next steps (exits 1 when a blocking problem is found)
  setup     Run provider-owned login with explicit consent and direct terminal access
  run       Launch a project-scoped harness session
  sessions  List or search recent Coven sessions
  attach    Replay/follow a session and forward input to live daemon sessions
  daemon    Manage the local Coven daemon
  status    Show what's happening across your coven: daemon, sessions, familiars, skills, research, hub
  help      Show concise help, every public command, or help for one command

Use `coven help --all` to browse every public command grouped by task.
Use `coven help <command>` or `coven <command> --help` for command-specific help.";

const DOCS_BASE_URL: &str = "https://docs.opencoven.ai/docs";
const HELP_COMMAND_SUMMARY: &str =
    "Show concise help, every public command, or help for one command";
const HELP_COMMAND_AFTER_HELP: &str = "Examples:
  coven help
  coven help run
  coven help sessions show
  coven help --all
  coven help --all --json";

#[derive(Parser, Debug)]
#[command(name = "coven help")]
#[command(about = HELP_COMMAND_SUMMARY)]
#[command(after_help = HELP_COMMAND_AFTER_HELP)]
struct ProgressiveHelpArgs {
    #[arg(
        long,
        value_name = "WHEN",
        value_parser = ["auto", "always", "never"],
        default_value = "auto",
        help = "Control ANSI color output; auto honors NO_COLOR and CLICOLOR_FORCE"
    )]
    color: String,
    #[arg(
        long,
        conflicts_with = "command_path",
        help = "Show every public command grouped by task"
    )]
    all: bool,
    #[arg(
        long,
        requires = "all",
        help = "Print grouped public help as JSON (machine-readable)"
    )]
    json: bool,
    #[arg(
        value_name = "COMMAND",
        num_args = 0..,
        help = "Command path to explain, for example `run` or `sessions show`"
    )]
    command_path: Vec<String>,
}

const HELP_GROUPS: &[HelpGroupSpec] = &[
    HelpGroupSpec {
        id: "start-and-launch",
        title: "Start and launch",
        commands: &[
            HelpCommandSpec::new("doctor", "/cli/doctor"),
            HelpCommandSpec::new("setup", "/reference/cli-setup"),
            HelpCommandSpec::new("run", "/cli/run"),
            HelpCommandSpec::new("sessions", "/cli/sessions"),
            HelpCommandSpec::new("attach", "/cli/sessions#attach"),
            HelpCommandSpec::new("daemon", "/cli/daemon"),
            HelpCommandSpec::new("status", "/cli/observe"),
            HelpCommandSpec::new("chat", "/cli/interactive"),
            HelpCommandSpec::new("tui", "/cli/interactive"),
            HelpCommandSpec::synthetic("help", "/cli/interactive", HELP_COMMAND_SUMMARY),
        ],
    },
    HelpGroupSpec {
        id: "configure-and-extend",
        title: "Configure and extend",
        commands: &[
            HelpCommandSpec::new("config", "/daemon/configuration"),
            HelpCommandSpec::new("completions", "/cli"),
            HelpCommandSpec::new("adapter", "/cli/repo-workflow"),
            HelpCommandSpec::new("engine", "/cli/engine-auth"),
            HelpCommandSpec::new("auth", "/cli/engine-auth"),
            HelpCommandSpec::new("models", "/cli/engine-auth"),
            HelpCommandSpec::new("acp", "/cli/engine-auth"),
            HelpCommandSpec::new("code", "/cli/engine-auth"),
        ],
    },
    HelpGroupSpec {
        id: "session-lifecycle",
        title: "Manage session lifecycle",
        commands: &[
            HelpCommandSpec::new("summon", "/cli/sessions#summon"),
            HelpCommandSpec::new("archive", "/cli/sessions#archive"),
            HelpCommandSpec::new("sacrifice", "/cli/sessions#sacrifice"),
            HelpCommandSpec::new("kill", "/cli/sessions"),
        ],
    },
    HelpGroupSpec {
        id: "observe-your-coven",
        title: "Observe your coven",
        commands: &[
            HelpCommandSpec::new("familiars", "/cli/observe"),
            HelpCommandSpec::new("skills", "/cli/observe"),
            HelpCommandSpec::new("memory", "/memory-models"),
            HelpCommandSpec::new("research", "/cli/observe"),
            HelpCommandSpec::new("calls", "/cli/observe"),
            HelpCommandSpec::new("hub", "/cli/hub-scheduler"),
            HelpCommandSpec::new("scheduler", "/cli/hub-scheduler"),
            HelpCommandSpec::new("travel", "/cli/hub-scheduler"),
        ],
    },
    HelpGroupSpec {
        id: "coordinate-parallel-work",
        title: "Coordinate parallel work",
        commands: &[
            HelpCommandSpec::new("wt", "/cli/repo-workflow"),
            HelpCommandSpec::new("claim", "/cli/repo-workflow"),
            HelpCommandSpec::new("maintenance", "/cli/repo-workflow"),
            HelpCommandSpec::new("hooks", "/cli/repo-workflow"),
        ],
    },
    HelpGroupSpec {
        id: "repair-and-administer",
        title: "Repair and administer",
        commands: &[
            HelpCommandSpec::new("logs", "/cli/observe"),
            HelpCommandSpec::new("vacuum", "/cli/observe"),
            HelpCommandSpec::new("reset", "/cli"),
            HelpCommandSpec::new("patch", "/cli/patch-openclaw"),
            HelpCommandSpec::new("pc", "/cli/pc"),
            HelpCommandSpec::new("ward", "/cli/repo-workflow"),
            HelpCommandSpec::new("executor", "/cli/hub-scheduler"),
        ],
    },
];

pub(crate) fn maybe_run_from_raw_args(raw_args: &[OsString]) -> Option<Result<()>> {
    let (filtered_args, color_args) = split_global_color_args(raw_args)?;
    let route = classify_help_route(&filtered_args)?;
    Some(match route {
        HelpRoute::Root { trailing_args } => run_root_help_route(&trailing_args, &color_args),
        HelpRoute::Nested { command_path } => {
            let args = parse_progressive_help_args(&[], &color_args);
            apply_color_choice(&args.color);
            write_command_help_with_color(&command_path, &color_args)
        }
    })
}

pub(crate) fn completion_command() -> Command {
    build_public_completion_command(&crate::Cli::command(), CompletionHelpSurface::Root)
}

fn run(all: bool, json: bool, command_path: &[String], color_args: &[OsString]) -> Result<()> {
    if all {
        if json {
            return write_public_help_json();
        }
        return write_public_help();
    }
    if command_path.is_empty() {
        return render_help_for_args([OsString::from("coven"), OsString::from("--help")]);
    }
    if command_path.len() == 1 && command_path[0] == "help" {
        return render_progressive_help_command(color_args);
    }
    write_command_help_with_color(command_path, color_args)
}

fn build_public_completion_command(
    source: &Command,
    help_surface: CompletionHelpSurface,
) -> Command {
    let mut command = clone_completion_command(source);
    let public_subcommands = source
        .get_subcommands()
        .filter(|candidate| is_public_completion_subcommand(candidate))
        .map(|candidate| build_public_completion_command(candidate, CompletionHelpSurface::Nested))
        .collect::<Vec<_>>();

    for subcommand in &public_subcommands {
        command = command.subcommand(subcommand.clone());
    }
    if !public_subcommands.is_empty() {
        command = command.subcommand(build_help_completion_subcommand(help_surface, source));
    }
    command
}

fn clone_completion_command(source: &Command) -> Command {
    let about = source.get_about().map(ToString::to_string);
    let mut command =
        Command::new(leak_command_name(source.get_name())).disable_help_subcommand(true);
    if let Some(about) = about {
        command = command.about(about);
    }
    for arg in source.get_arguments().filter(|arg| !arg.is_hide_set()) {
        command = command.arg(arg.clone());
    }
    command
}

fn build_help_completion_subcommand(
    help_surface: CompletionHelpSurface,
    source: &Command,
) -> Command {
    let mut help = match help_surface {
        CompletionHelpSurface::Root => root_help_completion_command(),
        CompletionHelpSurface::Nested => nested_help_completion_command(),
    };
    for subcommand in source
        .get_subcommands()
        .filter(|candidate| is_public_completion_subcommand(candidate))
        .map(build_help_target_completion_command)
    {
        help = help.subcommand(subcommand);
    }
    help.subcommand(match help_surface {
        CompletionHelpSurface::Root => root_help_completion_command(),
        CompletionHelpSurface::Nested => nested_help_completion_command(),
    })
}

fn root_help_completion_command() -> Command {
    let mut command = Command::new("help")
        .about(HELP_COMMAND_SUMMARY)
        .disable_help_subcommand(true);
    let help_command = ProgressiveHelpArgs::command();
    for arg in help_command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set())
    {
        let arg = if arg.is_positional() {
            arg.clone().hide(true)
        } else {
            arg.clone()
        };
        command = command.arg(arg);
    }
    command
}

fn nested_help_completion_command() -> Command {
    Command::new("help")
        .about("Print this message or the help of the given subcommand(s)")
        .disable_help_subcommand(true)
}

fn build_help_target_completion_command(source: &Command) -> Command {
    let about = source.get_about().map(ToString::to_string);
    let mut command =
        Command::new(leak_command_name(source.get_name())).disable_help_subcommand(true);
    if let Some(about) = about {
        command = command.about(about);
    }
    for subcommand in source
        .get_subcommands()
        .filter(|candidate| is_public_completion_subcommand(candidate))
        .map(build_help_target_completion_command)
    {
        command = command.subcommand(subcommand);
    }
    command
}

fn is_public_completion_subcommand(command: &Command) -> bool {
    !command.is_hide_set() && command.get_name() != "help"
}

fn leak_command_name(name: &str) -> &'static str {
    Box::leak(name.to_owned().into_boxed_str())
}

fn write_public_help() -> Result<()> {
    let groups = public_help_groups()?;
    let width = groups
        .iter()
        .flat_map(|group| group.commands.iter())
        .map(|command| command.name.len())
        .max()
        .unwrap_or(0);
    let mut stdout = io::stdout().lock();
    ok_if_broken_pipe((|| -> Result<()> {
        writeln!(stdout, "Coven public command guide").context("failed writing help output")?;
        writeln!(stdout).context("failed writing help output")?;
        writeln!(
            stdout,
            "`coven` with no arguments opens the interactive Coven UI."
        )
        .context("failed writing help output")?;
        writeln!(
            stdout,
            "Run `coven help <command>` or `coven <command> --help` for flags and subcommands."
        )
        .context("failed writing help output")?;
        writeln!(stdout).context("failed writing help output")?;
        for group in groups {
            writeln!(stdout, "{}", group.title).context("failed writing help output")?;
            for command in group.commands {
                writeln!(
                    stdout,
                    "  {:width$}  {}",
                    command.name,
                    command.summary,
                    width = width
                )
                .context("failed writing help output")?;
            }
            writeln!(stdout).context("failed writing help output")?;
        }
        writeln!(
            stdout,
            "Use `coven help --all --json` for a machine-readable command catalog."
        )
        .context("failed writing help output")?;
        Ok(())
    })())
}

fn write_public_help_json() -> Result<()> {
    let payload = PublicHelpPayload {
        schema_version: 1,
        groups: public_help_groups()?,
    };
    let mut stdout = io::stdout().lock();
    ok_if_broken_pipe((|| -> Result<()> {
        serde_json::to_writer_pretty(&mut stdout, &payload)
            .context("failed writing JSON help output")?;
        writeln!(stdout).context("failed finishing JSON help output")?;
        Ok(())
    })())
}

fn write_command_help_with_color(command_path: &[String], color_args: &[OsString]) -> Result<()> {
    let canonical_path = resolve_public_command_path(command_path);
    let args = std::iter::once(OsString::from("coven"))
        .chain(color_args.iter().cloned())
        .chain(canonical_path.into_iter().map(OsString::from))
        .chain(std::iter::once(OsString::from("--help")))
        .collect::<Vec<_>>();
    render_help_for_args(args)
}

fn resolve_public_command_path(command_path: &[String]) -> Vec<String> {
    let mut canonical_path = Vec::with_capacity(command_path.len());
    resolve_public_command(crate::Cli::command(), command_path, &mut canonical_path);
    canonical_path
}

fn resolve_public_command(
    command: Command,
    command_path: &[String],
    canonical_path: &mut Vec<String>,
) {
    let Some((segment, remainder)) = command_path.split_first() else {
        return;
    };
    let child = command
        .get_subcommands()
        .find(|candidate| matches_public_name(candidate, segment))
        .cloned()
        .unwrap_or_else(|| unknown_command(segment, &command, canonical_path));
    canonical_path.push(child.get_name().to_owned());
    resolve_public_command(child, remainder, canonical_path)
}

fn matches_public_name(command: &Command, requested: &str) -> bool {
    !command.is_hide_set()
        && (command.get_name() == requested
            || command.get_all_aliases().any(|alias| alias == requested))
}

fn unknown_command(segment: &str, parent: &Command, canonical_path: &[String]) -> ! {
    let mut full_path = vec!["coven".to_owned()];
    full_path.extend(canonical_path.iter().cloned());
    let mut parent = parent.clone().bin_name(full_path.join(" "));
    parent
        .error(
            clap::error::ErrorKind::InvalidSubcommand,
            format!("unrecognized public command `{segment}`"),
        )
        .exit()
}

fn render_help_for_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut command = crate::Cli::command();
    match command.try_get_matches_from_mut(args) {
        Ok(_) => unreachable!("help rendering should not parse into a runnable command"),
        Err(error) if error.exit_code() == 0 => {
            ok_if_broken_pipe(error.print().context("failed writing help output"))
        }
        Err(error) => Err(error.into()),
    }
}

fn render_progressive_help_command(color_args: &[OsString]) -> Result<()> {
    let args = std::iter::once(OsString::from("coven help"))
        .chain(color_args.iter().cloned())
        .chain(std::iter::once(OsString::from("--help")))
        .collect::<Vec<_>>();
    let mut command = ProgressiveHelpArgs::command();
    match command.try_get_matches_from_mut(args) {
        Ok(_) => unreachable!("progressive help rendering should display help"),
        Err(error) if error.exit_code() == 0 => {
            ok_if_broken_pipe(error.print().context("failed writing help output"))
        }
        Err(error) => Err(error.into()),
    }
}

fn ok_if_broken_pipe(result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if is_broken_pipe(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|io_error| io_error.kind() == io::ErrorKind::BrokenPipe)
            || cause
                .downcast_ref::<serde_json::Error>()
                .is_some_and(|json_error| {
                    json_error.io_error_kind() == Some(io::ErrorKind::BrokenPipe)
                })
    })
}

fn split_global_color_args(raw_args: &[OsString]) -> Option<(Vec<OsString>, Vec<OsString>)> {
    let mut filtered_args = Vec::with_capacity(raw_args.len());
    let mut color_args = Vec::new();
    let mut index = 0;
    while index < raw_args.len() {
        match raw_args[index].to_str() {
            Some("--color") => {
                let value = raw_args.get(index + 1)?.clone();
                color_args.push(raw_args[index].clone());
                color_args.push(value);
                index += 2;
            }
            Some(arg) if arg.starts_with("--color=") => {
                color_args.push(raw_args[index].clone());
                index += 1;
            }
            _ => {
                filtered_args.push(raw_args[index].clone());
                index += 1;
            }
        }
    }
    Some((filtered_args, color_args))
}

fn run_root_help_route(trailing_args: &[OsString], color_args: &[OsString]) -> Result<()> {
    let args = parse_progressive_help_args(trailing_args, color_args);
    apply_color_choice(&args.color);
    run(args.all, args.json, &args.command_path, color_args)
}

fn parse_progressive_help_args(
    trailing_args: &[OsString],
    color_args: &[OsString],
) -> ProgressiveHelpArgs {
    let args = std::iter::once(OsString::from("coven help"))
        .chain(color_args.iter().cloned())
        .chain(trailing_args.iter().cloned())
        .collect::<Vec<_>>();
    ProgressiveHelpArgs::try_parse_from(args).unwrap_or_else(|error| error.exit())
}

fn apply_color_choice(color: &str) {
    crate::theme::set_color_choice(color_choice_from_str(color));
}

fn color_choice_from_str(value: &str) -> crate::theme::ColorChoice {
    match value {
        "always" => crate::theme::ColorChoice::Always,
        "never" => crate::theme::ColorChoice::Never,
        _ => crate::theme::ColorChoice::Auto,
    }
}

fn classify_help_route(args: &[OsString]) -> Option<HelpRoute> {
    let (first, trailing) = args.split_first()?;
    if first == "help" {
        return Some(HelpRoute::Root {
            trailing_args: trailing.to_vec(),
        });
    }

    let mut current = crate::Cli::command();
    let mut prefix = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].to_str()?;
        if argument == "help" {
            if prefix.is_empty() || current.get_subcommands().next().is_none() {
                return None;
            }
            let mut command_path = prefix;
            for target in &args[index + 1..] {
                let target = target.to_str()?;
                if target.starts_with('-') {
                    return None;
                }
                command_path.push(target.to_owned());
            }
            return Some(HelpRoute::Nested { command_path });
        }
        if argument.starts_with('-') {
            return None;
        }
        let child = current
            .get_subcommands()
            .find(|candidate| matches_public_name(candidate, argument))
            .cloned()?;
        prefix.push(child.get_name().to_owned());
        current = child;
        index += 1;
    }
    None
}

fn public_help_groups() -> Result<Vec<PublicHelpGroup>> {
    let root = crate::Cli::command();
    let mut summaries = root
        .get_subcommands()
        .filter(|command| !command.is_hide_set())
        .map(|command| {
            let summary = command
                .get_about()
                .map(ToString::to_string)
                .ok_or_else(|| {
                    anyhow!(
                        "public command `{}` is missing an about string",
                        command.get_name()
                    )
                })?;
            Ok((command.get_name().to_owned(), summary))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    let mut groups = Vec::with_capacity(HELP_GROUPS.len());
    for group in HELP_GROUPS {
        let mut commands = Vec::with_capacity(group.commands.len());
        for command in group.commands {
            let summary = match (summaries.remove(command.name), command.summary_override) {
                (Some(summary), _) => summary,
                (None, Some(summary)) => summary.to_owned(),
                (None, None) => {
                    return Err(anyhow!(
                        "public help metadata references unknown command `{}`",
                        command.name
                    ))
                }
            };
            commands.push(PublicHelpCommand {
                name: command.name.to_owned(),
                summary,
                docs_url: format!("{DOCS_BASE_URL}{}", command.docs_path),
            });
        }
        groups.push(PublicHelpGroup {
            id: group.id.to_owned(),
            title: group.title.to_owned(),
            commands,
        });
    }

    if !summaries.is_empty() {
        let missing = summaries.into_keys().collect::<Vec<_>>();
        return Err(anyhow!(
            "public help metadata is missing visible command(s): {}",
            missing.join(", ")
        ));
    }

    Ok(groups)
}

#[derive(Clone, Copy)]
struct HelpGroupSpec {
    id: &'static str,
    title: &'static str,
    commands: &'static [HelpCommandSpec],
}

#[derive(Clone, Copy)]
struct HelpCommandSpec {
    name: &'static str,
    docs_path: &'static str,
    summary_override: Option<&'static str>,
}

enum HelpRoute {
    Root { trailing_args: Vec<OsString> },
    Nested { command_path: Vec<String> },
}

#[derive(Clone, Copy)]
enum CompletionHelpSurface {
    Root,
    Nested,
}

impl HelpCommandSpec {
    const fn new(name: &'static str, docs_path: &'static str) -> Self {
        Self {
            name,
            docs_path,
            summary_override: None,
        }
    }

    const fn synthetic(name: &'static str, docs_path: &'static str, summary: &'static str) -> Self {
        Self {
            name,
            docs_path,
            summary_override: Some(summary),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicHelpPayload {
    schema_version: u8,
    groups: Vec<PublicHelpGroup>,
}

#[derive(Serialize)]
struct PublicHelpGroup {
    id: String,
    title: String,
    commands: Vec<PublicHelpCommand>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicHelpCommand {
    name: String,
    summary: String,
    docs_url: String,
}
