//! Routine postconditions (coven#1818).
//!
//! A runtime that exits 0 has only said it stopped cleanly, not that it did
//! its job. A routine may declare what a successful run leaves behind; plain
//! settlement then records a run that exits 0 without meeting it as `failed`
//! with a `postcondition_unmet:` or `blocked:` reason instead of `succeeded`.
//! Routines that declare no postcondition keep exit-code success.

use std::path::{Component, Path};

use chrono::{DateTime, Days, NaiveDate, Utc};
use regex::{Regex, RegexBuilder};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::definition::RoutineTimezone;

/// Reason prefix for a run whose declared output is missing or wrong.
pub const POSTCONDITION_UNMET: &str = "postcondition_unmet";
/// Reason prefix for a run whose runtime could not act at all. It is a
/// reason, not a status: the run still ends `failed`.
pub const BLOCKED: &str = "blocked";

pub const POSTCONDITION_PATH_MAX_CHARS: usize = 1024;
pub const MUST_MATCH_LINES_MAX: usize = 16;
pub const MUST_MATCH_LINE_MAX_CHARS: usize = 512;
/// Larger files are not read; the postcondition is unmet.
pub const POSTCONDITION_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;
const PATTERN_SIZE_LIMIT: usize = 1 << 20;

const CWD: &str = "{cwd}";
const LOCAL_TODAY: &str = "{localToday}";
const LOCAL_YESTERDAY: &str = "{localYesterday}";
const NOT_LOGGED_IN: &str = "Not logged in";
const FAILED_PREFIX: &str = "failed:";
const LEFT_AS_WRITTEN_PREFIX: &str = "left as written ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PostconditionKind {
    /// One file the run must leave behind.
    File,
}

/// What a successful run must leave behind. `path` and each `mustMatchLines`
/// pattern may use `{cwd}`, `{localToday}` and `{localYesterday}`: the
/// routine's `cwd`, and the run's start date and the day before it in the
/// routine's timezone. Substituted values match literally inside patterns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutinePostcondition {
    pub kind: PostconditionKind,
    /// Absolute after substitution, with no `..` component.
    pub path: String,
    /// Each pattern must match at least one line of the file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub must_match_lines: Vec<String>,
    /// The file's modification time must not be earlier than the run's start.
    #[serde(default, skip_serializing_if = "is_false")]
    pub modified_during_run: bool,
    /// A final output line of exactly `left as written <path>` waives
    /// `modifiedDuringRun`: the run found the file already written.
    #[serde(default, skip_serializing_if = "is_false")]
    pub accept_left_as_written: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The facts a finished run is judged against.
pub(crate) struct RunContext<'a> {
    pub cwd: Option<&'a str>,
    pub timezone: RoutineTimezone,
    pub started_at: DateTime<Utc>,
    /// The session's clean output.
    pub output: &'a str,
}

impl RoutinePostcondition {
    /// Checks the declaration itself; `cwd` is the routine's.
    pub(crate) fn validate(&self, cwd: Option<&str>) -> Result<(), String> {
        let _ = cwd;
        Ok(())
    }

    /// Why a run that exited 0 must not be recorded `succeeded`, or `None`
    /// when it met the postcondition.
    pub(crate) fn unmet_reason(&self, run: &RunContext<'_>) -> Option<String> {
        let _ = run;
        None
    }
}

/// The run's start date and the day before it, in `timezone`.
fn local_dates(
    timezone: RoutineTimezone,
    started_at: DateTime<Utc>,
) -> Result<(NaiveDate, NaiveDate), String> {
    let _ = (timezone, started_at);
    Err("unimplemented".to_owned())
}

/// Judges a run that exited 0 against the postcondition its pinned definition
/// declares. `Ok(None)` means the run may be recorded `succeeded`.
pub(crate) fn check_run(
    conn: &Connection,
    run_id: &str,
    session_id: Option<&str>,
) -> Result<Option<String>, String> {
    let _ = (conn, run_id, session_id);
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn echo_shape() -> Value {
        json!({
            "kind": "file",
            "path": "{cwd}/memory/dreaming/days/{localYesterday}.md",
            "mustMatchLines": [
                "^<!-- coven:dream:v1 date={localYesterday} status=(dreamed|no-input) -->$",
                "^generatedAt: *\\S"
            ],
            "modifiedDuringRun": true,
            "acceptLeftAsWritten": true
        })
    }

    fn parse(value: Value) -> RoutinePostcondition {
        serde_json::from_value(value).unwrap()
    }

    /// 2026-11-22T18:00Z is 12:00 in Chicago, so yesterday is 2026-11-21.
    fn noon_chicago() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 22, 18, 0, 0).unwrap()
    }

    fn chicago() -> RoutineTimezone {
        RoutineTimezone::Iana("America/Chicago".parse().unwrap())
    }

    struct Workspace {
        _temp: tempfile::TempDir,
        cwd: String,
    }

    impl Workspace {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let cwd = temp.path().to_str().unwrap().to_owned();
            std::fs::create_dir_all(temp.path().join("memory/dreaming/days")).unwrap();
            Self { _temp: temp, cwd }
        }

        fn dream_path(&self) -> String {
            format!("{}/memory/dreaming/days/2026-11-21.md", self.cwd)
        }

        fn write_dream(&self, body: &str) {
            std::fs::write(self.dream_path(), body).unwrap();
        }

        /// A run that started `offset` from now, so a file written now was
        /// written during it (negative) or before it (positive).
        fn run<'a>(&'a self, output: &'a str, offset: chrono::Duration) -> RunContext<'a> {
            RunContext {
                cwd: Some(&self.cwd),
                timezone: RoutineTimezone::Utc,
                started_at: Utc::now() + offset,
                output,
            }
        }
    }

    const DREAM: &str = "---\ngeneratedAt: 2026-11-22T18:00:03Z\n---\n\n<!-- coven:dream:v1 date=2026-11-21 status=dreamed -->\n\nbody\n";

    /// Echo's shape with the date fixed, so the run's own clock does not move
    /// the file under test.
    fn fixed_day() -> RoutinePostcondition {
        let mut value = echo_shape();
        value["path"] = json!("{cwd}/memory/dreaming/days/2026-11-21.md");
        value["mustMatchLines"] = json!([
            "^<!-- coven:dream:v1 date=2026-11-21 status=(dreamed|no-input) -->$",
            "^generatedAt: *\\S"
        ]);
        parse(value)
    }

    fn before_start() -> chrono::Duration {
        chrono::Duration::seconds(60)
    }

    fn after_start() -> chrono::Duration {
        chrono::Duration::seconds(-60)
    }

    fn assert_reason(reason: Option<String>, prefix: &str, detail: &str) {
        let reason = reason.unwrap_or_else(|| panic!("expected a `{prefix}` reason"));
        assert!(reason.starts_with(&format!("{prefix}: ")), "{reason}");
        assert!(reason.contains(detail), "{detail}: {reason}");
    }

    #[test]
    fn echos_placeholder_shape_parses_and_round_trips() {
        let postcondition = parse(echo_shape());
        assert_eq!(postcondition.kind, PostconditionKind::File);
        assert!(postcondition.modified_during_run);
        assert!(postcondition.accept_left_as_written);
        assert_eq!(postcondition.must_match_lines.len(), 2);
        postcondition.validate(Some("/work/echo")).unwrap();
        assert_eq!(serde_json::to_value(&postcondition).unwrap(), echo_shape());
    }

    #[test]
    fn the_declaration_refuses_unknown_members_and_kinds() {
        let mut unknown = echo_shape();
        unknown["expectedOutput"] = json!("notes.md");
        let error = serde_json::from_value::<RoutinePostcondition>(unknown).unwrap_err();
        assert!(error.to_string().contains("unknown field"), "{error}");

        let mut kind = echo_shape();
        kind["kind"] = json!("directory");
        let error = serde_json::from_value::<RoutinePostcondition>(kind).unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }

    #[test]
    fn a_malformed_declaration_is_refused_for_what_is_wrong() {
        let refuse = |change: &dyn Fn(&mut Value), cwd: Option<&str>| {
            let mut value = echo_shape();
            change(&mut value);
            parse(value).validate(cwd).unwrap_err()
        };
        let cases: Vec<(String, &str)> = vec![
            (
                refuse(&|_| {}, None),
                "postcondition.path uses {cwd} but the routine has no cwd",
            ),
            (
                refuse(&|v| v["path"] = json!("notes/{localToday}.md"), None),
                "postcondition.path must be absolute",
            ),
            (
                refuse(
                    &|v| v["path"] = json!("{cwd}/../{localToday}.md"),
                    Some("/w"),
                ),
                "postcondition.path must not contain `..`",
            ),
            (
                refuse(&|v| v["path"] = json!("{cwd}/{today}.md"), Some("/w")),
                "postcondition.path uses unknown placeholder {today}",
            ),
            (
                refuse(&|v| v["path"] = json!(""), Some("/w")),
                "postcondition.path must be 1..=1024 characters",
            ),
            (
                refuse(
                    &|v| v["mustMatchLines"] = json!(["^generatedAt: ("]),
                    Some("/w"),
                ),
                "postcondition.mustMatchLines[0] is not a valid pattern",
            ),
            (
                refuse(&|v| v["mustMatchLines"] = json!(vec!["x"; 17]), Some("/w")),
                "postcondition.mustMatchLines must list at most 16 patterns",
            ),
            (
                refuse(
                    &|v| v["mustMatchLines"] = json!(["x".repeat(513)]),
                    Some("/w"),
                ),
                "postcondition.mustMatchLines[0] must be 1..=512 characters",
            ),
            (
                refuse(&|v| v["modifiedDuringRun"] = json!(false), Some("/w")),
                "postcondition.acceptLeftAsWritten requires modifiedDuringRun",
            ),
        ];
        for (error, expected) in cases {
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn a_file_written_during_the_run_with_every_line_meets_it() {
        let workspace = Workspace::new();
        workspace.write_dream(DREAM);
        let output = format!("wrote {}\n", workspace.dream_path());
        let run = workspace.run(&output, after_start());
        assert_eq!(fixed_day().unmet_reason(&run), None);
    }

    #[test]
    fn exit_zero_with_nothing_written_is_unmet() {
        let workspace = Workspace::new();
        let run = workspace.run("wrote it\n", after_start());
        assert_reason(
            fixed_day().unmet_reason(&run),
            POSTCONDITION_UNMET,
            "the declared file does not exist",
        );
    }

    #[test]
    fn a_file_without_a_required_line_is_unmet_and_names_the_pattern() {
        let workspace = Workspace::new();
        workspace.write_dream("generatedAt: 2026-11-22T18:00:03Z\nno marker\n");
        let run = workspace.run("", after_start());
        assert_reason(
            fixed_day().unmet_reason(&run),
            POSTCONDITION_UNMET,
            "no line matching mustMatchLines[0]",
        );
    }

    #[test]
    fn a_file_from_before_the_run_is_unmet_unless_left_as_written() {
        let workspace = Workspace::new();
        workspace.write_dream(DREAM);
        let run = workspace.run("wrote it\n", before_start());
        assert_reason(
            fixed_day().unmet_reason(&run),
            POSTCONDITION_UNMET,
            "was not modified during this run",
        );

        let output = format!("checked\nleft as written {}\n", workspace.dream_path());
        let run = workspace.run(&output, before_start());
        assert_eq!(fixed_day().unmet_reason(&run), None);

        // The waiver names the declared file exactly, and only when allowed.
        let output = "left as written /somewhere/else.md\n".to_owned();
        let run = workspace.run(&output, before_start());
        assert!(fixed_day().unmet_reason(&run).is_some());
        let mut strict = fixed_day();
        strict.accept_left_as_written = false;
        let output = format!("left as written {}\n", workspace.dream_path());
        let run = workspace.run(&output, before_start());
        assert!(strict.unmet_reason(&run).is_some());
    }

    #[test]
    fn a_final_failed_line_is_unmet_even_when_the_file_is_fine() {
        let workspace = Workspace::new();
        workspace.write_dream(DREAM);
        let output =
            "⚒ Bash(date)\nfailed: step 1–3 shell commands were blocked; nothing written\n\n";
        let run = workspace.run(output, after_start());
        assert_reason(
            fixed_day().unmet_reason(&run),
            POSTCONDITION_UNMET,
            "the final output line reports failure",
        );
        // Only the final line counts.
        let output = "failed: first try\nwrote it\n";
        let run = workspace.run(output, after_start());
        assert_eq!(fixed_day().unmet_reason(&run), None);
    }

    #[test]
    fn not_logged_in_output_is_blocked_before_anything_else() {
        let workspace = Workspace::new();
        workspace.write_dream(DREAM);
        let output = "\u{1b}[31mNot logged in · Please run /login\u{1b}[0m\r\nfailed: no\n";
        let run = workspace.run(output, after_start());
        assert_reason(
            fixed_day().unmet_reason(&run),
            BLOCKED,
            "the runtime is not logged in",
        );
        // Prose that merely mentions it is not the harness's sign-in notice.
        let output = "I noted the user was Not logged in yesterday\nwrote it\n";
        let run = workspace.run(output, after_start());
        assert_eq!(fixed_day().unmet_reason(&run), None);
    }

    #[test]
    fn dates_follow_the_routine_timezone_and_match_literally() {
        let workspace = Workspace::new();
        let postcondition = parse(json!({
            "kind": "file",
            "path": "{cwd}/memory/dreaming/days/{localYesterday}.md",
            "mustMatchLines": ["^date={localYesterday} today={localToday}$"],
        }));
        workspace.write_dream("date=2026-11-21 today=2026-11-22\n");
        let run = RunContext {
            cwd: Some(&workspace.cwd),
            timezone: chicago(),
            started_at: noon_chicago(),
            output: "",
        };
        assert_eq!(postcondition.unmet_reason(&run), None);

        // At 03:00Z it is still the 21st in Chicago, so yesterday is the 20th.
        let early = RunContext {
            started_at: Utc.with_ymd_and_hms(2026, 11, 22, 3, 0, 0).unwrap(),
            ..run
        };
        assert_reason(
            postcondition.unmet_reason(&early),
            POSTCONDITION_UNMET,
            "the declared file does not exist",
        );

        // A substituted cwd is matched literally, not as a pattern.
        let dotted = parse(json!({
            "kind": "file",
            "path": "{cwd}/memory/dreaming/days/2026-11-21.md",
            "mustMatchLines": ["^root={cwd}$"],
        }));
        workspace.write_dream(&format!("root={}\n", workspace.cwd.replace('/', "x")));
        let run = workspace.run("", after_start());
        assert!(dotted.unmet_reason(&run).is_some());
        workspace.write_dream(&format!("root={}\n", workspace.cwd));
        assert_eq!(dotted.unmet_reason(&run), None);
    }

    #[test]
    fn a_directory_or_oversized_file_is_unmet() {
        let workspace = Workspace::new();
        std::fs::create_dir_all(workspace.dream_path()).unwrap();
        let run = workspace.run("", after_start());
        assert_reason(
            fixed_day().unmet_reason(&run),
            POSTCONDITION_UNMET,
            "the declared file is not a regular file",
        );
        std::fs::remove_dir(workspace.dream_path()).unwrap();
        let file = std::fs::File::create(workspace.dream_path()).unwrap();
        file.set_len(POSTCONDITION_FILE_MAX_BYTES + 1).unwrap();
        assert_reason(
            fixed_day().unmet_reason(&run),
            POSTCONDITION_UNMET,
            "the declared file is larger than 4 MiB",
        );
    }

    #[test]
    fn the_local_dates_come_from_the_run_start() {
        assert_eq!(
            local_dates(chicago(), noon_chicago()).unwrap(),
            (
                NaiveDate::from_ymd_opt(2026, 11, 22).unwrap(),
                NaiveDate::from_ymd_opt(2026, 11, 21).unwrap()
            )
        );
    }
}
