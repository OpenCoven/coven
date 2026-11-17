//! Consumer behavior at the real CLI/process boundary. Real-engine history
//! and side effects are qualified separately with the loopback model fixture.
#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

const CAPS: &str = r#"{"engine_contract_version":1,"run_budget_version":1,"flag":"--max-turns","minimum":1,"maximum":4294967295,"default":10,"exhausted_outcome":"turn_budget_exhausted","one_shot_exhausted_exit_code":1,"stream_eof_exit_code":0}"#;

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let engine = root.path().join("engine");
        fs::write(&engine, r#"#!/usr/bin/python3
import sys,os,json,pathlib
if sys.argv[1:] == ['--version']:
 print('0.9.1'); sys.exit(0)
if sys.argv[1:] == ['--run-budget-capabilities']:
 pathlib.Path('capability-checked').touch()
 print(os.environ.get('TEST_BUDGET_CAPS','{}')); sys.exit(0)
pathlib.Path('argv.json').write_text(json.dumps(sys.argv[1:]))
limit = int(sys.argv[sys.argv.index('--max-turns')+1]) if '--max-turns' in sys.argv else 10
print(json.dumps({'type':'system','subtype':'init','session_id':'engine-session','run_budget_version':1,'turn_budget':{'max_turns':limit,'source':'explicit'}}))
print(json.dumps({'type':'text_delta','text':'fixture output'}))
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
    let f = Fixture::new();
    let receipt = json!({"type":"result","subtype":"error_during_execution","is_error":true,"outcome":"turn_budget_exhausted","run_budget_version":1,"session_id":"engine-session","num_turns":10,"turn_budget":{"max_turns":10,"source":"explicit","consumed":10},"continuation":{"session_id":"engine-session","resumable":true},"error":"unfinished"});
    let o = f
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
        .output()
        .unwrap();
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
