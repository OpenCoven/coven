use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let executable = env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let kind = if executable.eq_ignore_ascii_case("coven-code") {
        "coven-code"
    } else {
        "codex"
    };

    if kind == "coven-code" {
        run_coven_code(&args);
        log_invocation(kind, &args, None);
        return;
    }

    let prompt = codex_prompt(&args);
    log_invocation(kind, &args, Some(&prompt));
    if args.first().is_some_and(|argument| argument == "--version") {
        println!("codex 0.0.0-fake");
    } else if args.first().is_some_and(|argument| argument == "login") {
        println!("fake codex login ok");
    } else {
        println!("fake codex harness=codex");
        println!("fake codex complete: {prompt}");
    }
}

fn run_coven_code(args: &[String]) {
    if args == ["--version"] {
        println!("coven-code 0.6.1");
    } else if args == ["auth", "status", "--json"] {
        println!("{{\"loggedIn\":true}}");
    } else {
        println!("fake coven-code ready");
    }
}

fn codex_prompt(args: &[String]) -> String {
    if args.last().is_some_and(|argument| argument == "-") {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .expect("failed reading Codex fixture stdin");
        let prompt = input.trim();
        return if prompt.is_empty() {
            "<empty prompt>".to_string()
        } else {
            prompt.to_string()
        };
    }

    let prompt_args = args
        .iter()
        .position(|argument| argument == "--")
        .map(|index| &args[index + 1..])
        .unwrap_or(args);
    let prompt = prompt_args
        .iter()
        .filter(|argument| !argument.starts_with('-'))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if prompt.trim().is_empty() {
        "<empty prompt>".to_string()
    } else {
        prompt
    }
}

fn log_invocation(kind: &str, args: &[String], prompt: Option<&str>) {
    let Ok(log_path) = env::var("COVEN_FAKE_FIXTURE_LOG") else {
        return;
    };
    let cwd = env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let args_json = args
        .iter()
        .map(|argument| format!("\"{}\"", json_escape(argument)))
        .collect::<Vec<_>>()
        .join(",");
    let prompt_json = prompt
        .map(|value| format!("\"{}\"", json_escape(value)))
        .unwrap_or_else(|| "null".to_string());
    let record = format!(
        "{{\"kind\":\"{}\",\"argv\":[{}],\"cwd\":\"{}\",\"prompt\":{}}}\n",
        json_escape(kind),
        args_json,
        json_escape(&cwd),
        prompt_json
    );
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .expect("failed opening fixture log");
    file.write_all(record.as_bytes())
        .expect("failed writing fixture log");
}

fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect(),
            '\n' => "\\n".chars().collect(),
            '\r' => "\\r".chars().collect(),
            '\t' => "\\t".chars().collect(),
            character if character.is_control() => {
                format!("\\u{:04x}", character as u32).chars().collect()
            }
            character => vec![character],
        })
        .collect()
}
