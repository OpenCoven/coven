#![cfg(windows)]

use std::io::{Read, Write};
use std::time::Duration;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    std::fs::write("args.txt", args.join(" ")).expect("failed to record Codex argv");

    let mut prompt = String::new();
    std::io::stdin()
        .read_to_string(&mut prompt)
        .expect("failed to read Codex prompt");
    std::fs::write("stdin.txt", prompt).expect("failed to record Codex stdin");

    if std::env::var_os("COVEN_TEST_CODEX_PROBE_SILENT").is_some() {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }

    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        r#"{{"type":"thread.started","thread_id":"thread-789"}}"#
    )
    .unwrap();
    writeln!(stdout, r#"{{"type":"turn.started"}}"#).unwrap();
    writeln!(
        stdout,
        r#"{{"type":"item.completed","item":{{"id":"item-1","type":"agent_message","text":"reply for Cave"}}}}"#
    )
    .unwrap();
    writeln!(stdout, r#"{{"type":"turn.completed"}}"#).unwrap();
    stdout.flush().unwrap();
}
