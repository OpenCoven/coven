//! Consumer behavior at the real CLI/process boundary. Real-engine history
//! and side effects are qualified separately with the loopback model fixture.
#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};

const CAPS: &str = r#"{"engine_contract_version":1,"run_budget_version":1,"flag":"--max-turns","minimum":1,"maximum":4294967295,"default":10,"exhausted_outcome":"turn_budget_exhausted","one_shot_exhausted_exit_code":1,"stream_eof_exit_code":0}"#;

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let engine = root.path().join("engine");
        fs::write(&engine, r#"#!/usr/bin/python3
import sys,os,json,pathlib,time
if sys.argv[1:] == ['--version']:
 print('0.9.1'); sys.exit(0)
if sys.argv[1:] == ['--run-budget-capabilities']:
 pathlib.Path('capability-checked').touch()
 print(os.environ.get('TEST_BUDGET_CAPS','{}')); sys.exit(0)
pathlib.Path('argv.json').write_text(json.dumps(sys.argv[1:]))
limit = int(sys.argv[sys.argv.index('--max-turns')+1]) if '--max-turns' in sys.argv else 10
print(json.dumps({'type':'system','subtype':'init','session_id':'engine-session','run_budget_version':1,'turn_budget':{'max_turns':limit,'source':'explicit'}}),flush=True)
print(json.dumps({'type':'text_delta','text':'fixture output'}),flush=True)
if os.environ.get('TEST_RESULT_GATE'):
 pathlib.Path('ready').touch()
 deadline=time.monotonic()+30
 while not pathlib.Path('release').exists():
  if time.monotonic()>deadline:sys.exit(64)
  time.sleep(.01)
normal = {'type':'result','subtype':'success','is_error':False,'session_id':'engine-session','run_budget_version':1,'num_turns':1,'turn_budget':{'max_turns':limit,'source':'explicit','consumed':1},'continuation':{'session_id':'engine-session','resumable':True}}
print(os.environ.get('TEST_BUDGET_RESULT',json.dumps(normal)))
sys.exit(int(os.environ.get('TEST_ENGINE_EXIT','0')))
"#).unwrap();
        fs::set_permissions(engine, fs::Permissions::from_mode(0o755)).unwrap();
        Self(root)
    }

    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_coven"));
        c.env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", self.0.path().join("home"))
            .env("COVEN_HOME", self.0.path().join("state"))
            .env("COVEN_ENGINE_BIN", self.0.path().join("engine"))
            .env("TEST_BUDGET_CAPS", CAPS)
            .current_dir(self.0.path());
        c
    }

    fn args(&self) -> Vec<String> {
        serde_json::from_slice(&fs::read(self.0.path().join("argv.json")).unwrap()).unwrap()
    }
}

fn frames(o: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|s| serde_json::from_str(s).ok())
        .collect()
}

#[test]
fn later_streamed_input_needs_its_own_saved_result_before_continuation() {
    use std::io::Write;
    for (complete, exit) in [(false, 0), (false, 7), (true, 0)] {
        let f = Fixture::new();
        let engine = f.0.path().join("engine");
        let complete_python = if complete { "True" } else { "False" };
        let source = fs::read_to_string(&engine).unwrap().replace(
            "print(os.environ.get('TEST_BUDGET_RESULT',json.dumps(normal)))",
            &format!(
                "sys.stdin.readline()\nprint(json.dumps(normal),flush=True)\nsys.stdin.readline()\npathlib.Path('later-effect').write_text('performed')\nprint(json.dumps({{'type':'text_delta','text':'later partial work'}}),flush=True)\nif {complete_python}: print(json.dumps(normal),flush=True)\n"
            ),
        );
        fs::write(engine, source).unwrap();
        let mut child = f
            .command()
            .env("TEST_ENGINE_EXIT", exit.to_string())
            .args([
                "run",
                "coven-code",
                "--max-turns",
                "12",
                "--stream-json",
                "--stream-json-input",
                "--",
                "work",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = format!("{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{}\"}}}}\n{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"second\"}}}}\n", "x".repeat(8192));
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            fs::read_to_string(f.0.path().join("later-effect")).unwrap(),
            "performed"
        );
        let all = frames(&output);
        let last = all.last().unwrap();
        if complete {
            assert_eq!(last["continuation"]["resumable"], exit == 0, "{output:?}");
        } else {
            assert!(last.get("continuation").is_none(), "{output:?}");
            assert!(last.get("outcome").is_none(), "{output:?}");
        }
        if !complete {
            assert_eq!(last["is_error"], true, "{output:?}");
            fs::remove_file(f.0.path().join("argv.json")).unwrap();
            let resumed = f
                .command()
                .args([
                    "run",
                    "coven-code",
                    "--continue",
                    last["session_id"].as_str().unwrap(),
                    "--max-turns",
                    "12",
                    "--",
                    "resume",
                ])
                .output()
                .unwrap();
            assert!(!resumed.status.success(), "{resumed:?}");
            assert!(!f.0.path().join("argv.json").exists());
        }
    }
}

#[test]
fn a_fresh_budgeted_history_is_pending_before_other_aliases_can_resume_it() {
    use std::io::{BufRead, BufReader, Read};
    let f = Fixture::new();
    let mut child = f
        .command()
        .env("TEST_RESULT_GATE", "1")
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--",
            "work",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let init: Value = serde_json::from_str(&first).unwrap();
    let resumed = f
        .command()
        .args([
            "run",
            "coven-code",
            "--continue",
            init["session_id"].as_str().unwrap(),
            "--",
            "do not replay unfinished work",
        ])
        .output()
        .unwrap();
    fs::write(f.0.path().join("release"), "").unwrap();
    let mut rest = String::new();
    reader.read_to_string(&mut rest).unwrap();
    let _settled = child.wait_with_output().unwrap();
    assert!(!resumed.status.success(), "{resumed:?}");
    assert!(
        String::from_utf8_lossy(&resumed.stderr).contains("not confirmed saved"),
        "{resumed:?}"
    );
}

#[test]
fn explicit_budget_is_forwarded_before_prompt_and_declared_to_consumers() {
    let f = Fixture::new();
    let listed = f
        .command()
        .args(["adapter", "list", "--json"])
        .output()
        .unwrap();
    let list: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let engine = list
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == "coven-code")
        .unwrap();
    assert_eq!(engine["run_budget"]["version"], 1);
    let o = f
        .command()
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--",
            "--max-turns 999 is prompt data",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let args = f.args();
    let at = args.iter().position(|x| x == "--max-turns").unwrap();
    assert_eq!(args[at + 1], "12");
    assert!(at < args.iter().position(|x| x == "--").unwrap());
    assert_eq!(args.last().unwrap(), "--max-turns 999 is prompt data");
}

#[test]
fn invalid_and_unsupported_budgets_never_launch_work() {
    for value in ["0", "-1", "no", "4294967296"] {
        let f = Fixture::new();
        let o = f
            .command()
            .args(["run", "coven-code", "--max-turns", value, "--", "hello"])
            .output()
            .unwrap();
        assert!(!o.status.success());
        assert!(!f.0.path().join("argv.json").exists());
    }
    let f = Fixture::new();
    let o = f
        .command()
        .env("TEST_BUDGET_CAPS", "{}")
        .args(["run", "coven-code", "--max-turns", "12", "--", "hello"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        f.0.path().join("capability-checked").exists(),
        "The resolved engine must be probed: {o:?}"
    );
    assert!(!f.0.path().join("argv.json").exists());
}

#[test]
fn typed_exhaustion_survives_successful_stream_process_exit() {
    use std::io::Write;
    let f = Fixture::new();
    let engine = f.0.path().join("engine");
    let source = fs::read_to_string(&engine).unwrap().replace(
        "print(os.environ.get('TEST_BUDGET_RESULT',json.dumps(normal)))",
        "sys.stdin.readline()\nprint(os.environ.get('TEST_BUDGET_RESULT',json.dumps(normal)),flush=True)",
    );
    fs::write(engine, source).unwrap();
    let receipt = json!({"type":"result","subtype":"error_during_execution","is_error":true,"outcome":"turn_budget_exhausted","run_budget_version":1,"session_id":"engine-session","num_turns":10,"turn_budget":{"max_turns":10,"source":"explicit","consumed":10},"continuation":{"session_id":"engine-session","resumable":true},"error":"unfinished"});
    let mut child = f
        .command()
        .env("TEST_BUDGET_RESULT", receipt.to_string())
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "10",
            "--stream-json",
            "--stream-json-input",
            "--",
            "hello",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    let all = frames(&o);
    let last = all
        .last()
        .unwrap_or_else(|| panic!("missing terminal frame: {o:?}"));
    assert_eq!(last["outcome"], "turn_budget_exhausted", "{o:?}");
    assert_eq!(last["is_error"], true);
    assert_eq!(last["turn_budget"]["consumed"], 10);
    assert_eq!(last["continuation"]["session_id"], "engine-session");
    assert_ne!(last["session_id"], "engine-session");
}

#[test]
fn process_failure_cannot_be_erased_by_a_successful_budget_receipt() {
    let f = Fixture::new();
    let receipt = json!({"type":"result","subtype":"success","is_error":false,"run_budget_version":1,"session_id":"engine-session","num_turns":2,"turn_budget":{"max_turns":12,"source":"explicit","consumed":2},"continuation":{"session_id":"engine-session","resumable":true}});
    let o = f
        .command()
        .env("TEST_BUDGET_RESULT", receipt.to_string())
        .env("TEST_ENGINE_EXIT", "7")
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--",
            "hello",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(7));
    assert_eq!(frames(&o).last().unwrap()["is_error"], true, "{o:?}");
}

#[test]
fn streamed_provider_failure_stays_distinct_from_unconfirmed_query_history() {
    use std::io::Write;
    let f = Fixture::new();
    let engine = f.0.path().join("engine");
    let source = fs::read_to_string(&engine).unwrap().replace(
        "print(os.environ.get('TEST_BUDGET_RESULT',json.dumps(normal)))",
        "sys.stdin.readline()\nprint(os.environ.get('TEST_BUDGET_RESULT',json.dumps(normal)),flush=True)",
    );
    fs::write(engine, source).unwrap();
    let receipt = json!({"type":"result","is_error":true,"error":"Authentication failed","run_budget_version":1,"session_id":"engine-session","num_turns":1,"turn_budget":{"max_turns":12,"source":"explicit","consumed":1},"continuation":{"session_id":"engine-session","resumable":true}});
    let mut child = f
        .command()
        .env("TEST_BUDGET_RESULT", receipt.to_string())
        .env("TEST_ENGINE_EXIT", "7")
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--stream-json-input",
            "--",
            "hello",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let all = frames(&output);
    let last = all.last().unwrap();
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert_eq!(last["error"], "Authentication failed");
    assert_eq!(last["continuation"]["resumable"], false);
    assert!(last.get("outcome").is_none());
}

#[test]
fn continuation_uses_persisted_engine_identity_instead_of_ledger_identity() {
    let f = Fixture::new();
    let receipt = json!({"type":"result","subtype":"success","is_error":false,"run_budget_version":1,"session_id":"engine-session","num_turns":2,"turn_budget":{"max_turns":12,"source":"explicit","consumed":2},"continuation":{"session_id":"engine-session","resumable":true}});
    let o = f
        .command()
        .env("TEST_BUDGET_RESULT", receipt.to_string())
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--",
            "hello",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    let all = frames(&o);
    let ledger = all.last().unwrap()["session_id"].as_str().unwrap();
    assert_ne!(ledger, "engine-session");
    let resumed = f
        .command()
        .env("TEST_BUDGET_RESULT", receipt.to_string())
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--continue",
            ledger,
            "--",
            "continue",
        ])
        .output()
        .unwrap();
    assert!(resumed.status.success(), "{resumed:?}");
    let args = f.args();
    let at = args.iter().position(|x| x == "--resume").unwrap();
    assert_eq!(args[at + 1], "engine-session");
}

#[test]
fn one_shot_budget_success_accepts_the_existing_engine_result_envelope() {
    let f = Fixture::new();
    // Observed from the real PR498 one-shot process after eleven rounds.
    let receipt = json!({"type":"result","session_id":"engine-session","num_turns":11,"run_budget_version":1,"turn_budget":{"max_turns":12,"source":"explicit","consumed":11},"continuation":{"session_id":"engine-session","resumable":true},"cost_usd":0.00099,"usage":{"input_tokens":1,"output_tokens":1}});
    let o = f
        .command()
        .env("TEST_BUDGET_RESULT", receipt.to_string())
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--",
            "hello",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    let all = frames(&o);
    let last = all.last().unwrap();
    assert_eq!(last["is_error"], false);
    assert_eq!(last["turn_budget"]["consumed"], 11);
}

#[test]
fn a_negotiated_budget_requires_a_matching_applied_receipt() {
    for receipt in [
        json!({"type":"result","subtype":"success","is_error":false,"session_id":"engine-session"}),
        json!({"type":"result","subtype":"success","is_error":false,"session_id":"engine-session","num_turns":1,"run_budget_version":1,"turn_budget":{"max_turns":10,"source":"default","consumed":1},"continuation":{"session_id":"engine-session","resumable":true}}),
    ] {
        let f = Fixture::new();
        let o = f
            .command()
            .env("TEST_BUDGET_RESULT", receipt.to_string())
            .args([
                "run",
                "coven-code",
                "--max-turns",
                "12",
                "--stream-json",
                "--",
                "hello",
            ])
            .output()
            .unwrap();
        assert_eq!(frames(&o).last().unwrap()["is_error"], true, "{o:?}");
    }
}

#[test]
fn ordinary_budget_run_renders_text_and_retains_engine_continuity() {
    let f = Fixture::new();
    let first = f
        .command()
        .args(["run", "coven-code", "--max-turns", "12", "--", "hello"])
        .output()
        .unwrap();
    assert!(first.status.success(), "{first:?}");
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(stdout.contains("fixture output"));
    assert!(
        !stdout.contains("\"type\""),
        "Human output must not contain machine protocol: {first:?}"
    );
    let o = f
        .command()
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--continue",
            "--",
            "again",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    let args = f.args();
    let at = args.iter().position(|x| x == "--resume").unwrap();
    assert_eq!(args[at + 1], "engine-session");
}

#[test]
fn a_newer_unconfirmed_save_blocks_every_older_engine_alias() {
    for ordinary in [false, true] {
        let f = Fixture::new();
        let first = f
            .command()
            .args([
                "run",
                "coven-code",
                "--max-turns",
                "12",
                "--stream-json",
                "--",
                "first",
            ])
            .output()
            .unwrap();
        assert!(first.status.success(), "{first:?}");
        let values = frames(&first);
        let older = values.last().unwrap()["session_id"].as_str().unwrap();
        let receipt = if ordinary {
            json!({"type":"result","subtype":"success","is_error":false,"run_budget_version":1,"session_id":"engine-session","num_turns":1,"turn_budget":{"max_turns":10,"source":"default","consumed":1},"continuation":{"session_id":"engine-session","resumable":false}})
        } else {
            json!({"type":"result","subtype":"success","is_error":false,"run_budget_version":1,"session_id":"engine-session","num_turns":1,"turn_budget":{"max_turns":12,"source":"explicit","consumed":1},"continuation":{"session_id":"engine-session","resumable":false}})
        };
        let mut second = f.command();
        second.env("TEST_BUDGET_RESULT", receipt.to_string()).args([
            "run",
            "coven-code",
            "--continue",
            older,
        ]);
        if !ordinary {
            second.args(["--max-turns", "12", "--stream-json"]);
        }
        let output = second.args(["--", "second"]).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        fs::remove_file(f.0.path().join("argv.json")).unwrap();
        let third = f
            .command()
            .args([
                "run",
                "coven-code",
                "--continue",
                older,
                "--max-turns",
                "12",
                "--stream-json",
                "--",
                "third",
            ])
            .output()
            .unwrap();
        assert!(
            !third.status.success(),
            "A failed newer save must invalidate the whole engine conversation: {third:?}"
        );
        assert!(
            !f.0.path().join("argv.json").exists(),
            "Unconfirmed history must fail before work starts"
        );
    }
}

#[test]
fn a_parsed_one_shot_receipt_survives_stdout_disconnect() {
    use std::io::{BufRead, BufReader};
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    let mut child = f
        .command()
        .env("TEST_RESULT_GATE", "1")
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--",
            "hello",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pipe = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(pipe).lines();
        let forwarded = lines.any(|line| {
            line.ok()
                .and_then(|line| serde_json::from_str::<Value>(&line).ok())
                .is_some_and(|value| value["type"] == "text_delta")
        });
        drop(lines); // Close the consumer only after initial output was forwarded.
        let _ = tx.send(forwarded);
    });
    let ready = rx.recv_timeout(Duration::from_secs(30));
    fs::write(f.0.path().join("release"), "").unwrap();
    // Hang guard, not a promptness assertion. The fixture gate also has its
    // own deadline, so an error cannot leave a permanently waiting engine.
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!(
                "consumer did not settle after disconnect: {:?}",
                child.wait_with_output()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = child.wait_with_output().unwrap();
    assert!(ready.unwrap(), "{out:?}");
    assert!(!out.status.success());
    let listing = f.command().args(["sessions", "--json"]).output().unwrap();
    let sessions: Value = serde_json::from_slice(&listing.stdout).unwrap();
    assert_eq!(
        sessions["sessions"][0]["conversation_id"], "engine-session",
        "{out:?}"
    );
}

#[test]
fn budget_probe_is_bound_to_the_executable_that_launches_work() {
    let f = Fixture::new();
    let root = f.0.path().join("home/.coven/engine");
    let source = fs::read_to_string(f.0.path().join("engine")).unwrap();
    for (version, label) in [("verified", "verified"), ("unverified", "unverified")] {
        let dir = root.join(version);
        fs::create_dir_all(&dir).unwrap();
        let mut script = source.replace(
            "pathlib.Path('argv.json')",
            &format!(
                "pathlib.Path('selected-engine').write_text('{label}')\npathlib.Path('argv.json')"
            ),
        );
        if version == "verified" {
            script = script.replace("pathlib.Path('capability-checked').touch()", "pathlib.Path('capability-checked').touch()\n (pathlib.Path(os.environ['HOME'])/'.coven/engine/current').write_text('unverified')");
        } else {
            script = script.replace(
                "print(os.environ.get('TEST_BUDGET_CAPS','{}'))",
                "print('{}')",
            );
        }
        let file = dir.join("coven-code");
        fs::write(&file, script).unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(root.join("current"), "verified").unwrap();
    let out = f
        .command()
        .env_remove("COVEN_ENGINE_BIN")
        .args([
            "run",
            "coven-code",
            "--max-turns",
            "12",
            "--stream-json",
            "--",
            "hello",
        ])
        .output()
        .unwrap();
    let selected = fs::read_to_string(f.0.path().join("selected-engine")).ok();
    if out.status.success() {
        assert_eq!(
            selected.as_deref(),
            Some("verified"),
            "A changed managed selector must not launch an unprobed engine: {out:?}"
        );
    } else {
        assert!(
            selected.is_none(),
            "An unsupported replacement must fail before work: {out:?}"
        );
    }
}
