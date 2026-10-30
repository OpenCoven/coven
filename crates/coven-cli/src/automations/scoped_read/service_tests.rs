//! Tests for the scoped read service's protocol, gate and time limits, and
//! for its channel and relay.

use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::open::{Body, Response};
use super::scope::{RelativePath, RootKind};
use super::service::{
    tool_definitions, Gate, Limits, Open, Operation, Outcome, Service, R0_OPERATION_TIME,
    TOOL_NAMES,
};
use super::{Refusal, R0_READ_POLICY};
use crate::automations::contract::canonical_json::{canonicalize, sha256_hex};

const LIMITS: Limits = Limits {
    operation: R0_OPERATION_TIME,
    deadline: None,
};

fn file(content: &str) -> Response {
    Response {
        root: RootKind::Project,
        path: RelativePath::parse("notes/today.md", &R0_READ_POLICY).expect("a path"),
        body: Body::File {
            content: content.to_owned(),
        },
    }
}

/// A service whose reader answers every request with `answer`, recording the
/// requests it saw.
fn service_with(
    answer: Result<Response, Refusal>,
    gate: Box<dyn Gate>,
    limits: Limits,
) -> (Service, Arc<Mutex<Vec<Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    let service = Service::with_serve(
        Box::new(move |request| {
            record.lock().unwrap().push(request.clone());
            answer.clone()
        }),
        gate,
        limits,
    );
    (service, seen)
}

/// Runs `service` over `messages`, one per line, and returns its replies and
/// operations.
fn run(service: Service, messages: &[Value]) -> (Vec<Value>, Vec<Operation>) {
    let mut input = Vec::new();
    for message in messages {
        input.extend(serde_json::to_vec(message).unwrap());
        input.push(b'\n');
    }
    let mut output = Vec::new();
    let operations = service.run(Cursor::new(input), &mut output);
    let replies = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("each reply is one JSON line"))
        .collect();
    (replies, operations)
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn initialize() -> Value {
    request(0, "initialize", json!({ "protocolVersion": "2025-06-18" }))
}

fn call(id: u64, tool: &str, arguments: Value) -> Value {
    request(
        id,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
}

fn read(id: u64) -> Value {
    call(
        id,
        "read_file",
        json!({ "root": "project", "path": "notes/today.md" }),
    )
}

/// The text and error flag of a tool result.
fn tool_text(reply: &Value) -> (String, bool) {
    let result = &reply["result"];
    assert_eq!(
        result["content"].as_array().map(Vec::len),
        Some(1),
        "{reply}"
    );
    (
        result["content"][0]["text"]
            .as_str()
            .expect("text content")
            .to_owned(),
        result["isError"].as_bool().expect("isError"),
    )
}

#[test]
fn initialize_negotiates_a_revision_it_speaks() {
    let (service, _) = service_with(Ok(file("x")), Box::new(Open), LIMITS);
    let (replies, _) = run(
        service,
        &[
            initialize(),
            request(1, "initialize", json!({ "protocolVersion": "1999-01-01" })),
            request(2, "initialize", json!({})),
        ],
    );
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[1]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(replies[2]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        replies[0]["result"]["capabilities"],
        json!({ "tools": { "listChanged": false } })
    );
}

#[test]
fn tools_are_listed_only_after_initialize() {
    let (service, _) = service_with(Ok(file("x")), Box::new(Open), LIMITS);
    let (replies, operations) = run(
        service,
        &[
            request(1, "ping", json!({})),
            request(2, "tools/list", json!({})),
            read(3),
            initialize(),
            request(4, "tools/list", json!({})),
        ],
    );
    assert_eq!(replies[0]["result"], json!({}));
    assert_eq!(replies[1]["error"]["code"], -32002);
    assert_eq!(replies[2]["error"]["code"], -32002);
    let tools = replies[4]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOL_NAMES);
    assert_eq!(replies[4]["result"]["tools"], tool_definitions());
    for tool in tools {
        assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{tool}");
    }
    assert!(operations.is_empty(), "a call before initialize never runs");
}

#[test]
fn malformed_messages_get_protocol_errors() {
    let (service, seen) = service_with(Ok(file("x")), Box::new(Open), LIMITS);
    let mut input = b"not json\n[1,2]\n".to_vec();
    for message in [
        initialize(),
        json!({ "jsonrpc": "2.0", "id": null, "method": "ping" }),
        json!({ "id": 7, "method": "ping" }),
        json!({ "jsonrpc": "2.0", "id": 8, "method": "resources/list" }),
        json!({ "jsonrpc": "2.0", "id": 9 }),
        // A notification is never answered.
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 10, "method": "ping" }),
    ] {
        input.extend(serde_json::to_vec(&message).unwrap());
        input.push(b'\n');
    }
    let mut output = Vec::new();
    service.run(Cursor::new(input), &mut output);
    let replies: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let codes: Vec<(Value, Value)> = replies
        .iter()
        .map(|reply| (reply["id"].clone(), reply["error"]["code"].clone()))
        .collect();
    assert_eq!(
        codes,
        [
            (Value::Null, json!(-32700)),
            (Value::Null, json!(-32600)),
            (json!(0), Value::Null),
            (Value::Null, json!(-32600)),
            (json!(7), json!(-32600)),
            (json!(8), json!(-32601)),
            (json!(9), json!(-32600)),
            (json!(10), Value::Null),
        ]
    );
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn a_served_result_is_exactly_the_committed_text() {
    let commits = Arc::new(Mutex::new(Vec::new()));
    let gate = Scripted {
        admit: Ok(()),
        commit: Ok(()),
        commits: Arc::clone(&commits),
    };
    let response = file("granted notes");
    let (service, seen) = service_with(Ok(response.clone()), Box::new(gate), LIMITS);
    let (replies, operations) = run(service, &[initialize(), read(1)]);

    let (text, is_error) = tool_text(&replies[1]);
    assert!(!is_error);
    let expected = String::from_utf8(canonicalize(&response.to_json()).unwrap()).unwrap();
    assert_eq!(text, expected);
    let digest = sha256_hex(text.as_bytes());
    assert_eq!(digest, response.digest());
    assert_eq!(
        operations,
        [Operation {
            sequence: 1,
            tool: Some("read_file"),
            outcome: Outcome::Served {
                digest: digest.clone()
            },
        }]
    );
    assert_eq!(*commits.lock().unwrap(), [(Some("read_file"), digest)]);
    assert_eq!(
        *seen.lock().unwrap(),
        [json!({ "operation": "read_file", "root": "project", "path": "notes/today.md" })]
    );
}

#[test]
fn a_refusal_is_committed_and_answered_as_a_tool_error() {
    let (service, _) = service_with(Err(Refusal::ScopeMissing), Box::new(Open), LIMITS);
    let (replies, operations) = run(service, &[initialize(), read(1), read(2)]);
    let (text, is_error) = tool_text(&replies[1]);
    assert!(is_error);
    assert_eq!(text, r#"{"refusal":"scope_missing"}"#);
    assert_eq!(operations.len(), 2, "a refusal does not stop the service");
    assert_eq!(
        operations[0].outcome,
        Outcome::Refused {
            refusal: Refusal::ScopeMissing,
            digest: sha256_hex(text.as_bytes()),
        }
    );
    assert_eq!(operations[1].sequence, 2);
}

#[test]
fn every_call_reaches_the_reader_however_it_is_spelled() {
    let (service, seen) = service_with(Err(Refusal::RequestInvalid), Box::new(Open), LIMITS);
    let (_, operations) = run(
        service,
        &[
            initialize(),
            call(1, "remove_file", json!({ "root": "project", "path": "a" })),
            call(
                2,
                "read_file",
                json!({ "operation": "search", "root": "project", "path": "a" }),
            ),
            request(
                3,
                "tools/call",
                json!({ "name": "read_file", "arguments": [1] }),
            ),
            request(4, "tools/call", json!("read_file")),
            request(5, "tools/call", json!({ "name": "list_directory" })),
        ],
    );
    assert_eq!(
        *seen.lock().unwrap(),
        [
            json!({ "operation": "remove_file", "root": "project", "path": "a" }),
            json!({ "operation": null }),
            json!({ "operation": null }),
            json!({ "operation": null }),
            json!({ "operation": "list_directory" }),
        ]
    );
    let tools: Vec<Option<&str>> = operations.iter().map(|operation| operation.tool).collect();
    assert_eq!(
        tools,
        [
            None,
            Some("read_file"),
            Some("read_file"),
            None,
            Some("list_directory")
        ]
    );
}

#[test]
fn a_closed_gate_withholds_the_result_and_stops_the_service() {
    for (admit, commit) in [
        (Err(Refusal::AttemptClosed), Ok(())),
        (Ok(()), Err(Refusal::AttemptClosed)),
    ] {
        let gate = Scripted {
            admit,
            commit,
            commits: Arc::default(),
        };
        let (service, seen) = service_with(Ok(file("SENTINEL")), Box::new(gate), LIMITS);
        let mut output = Vec::new();
        let mut input = Vec::new();
        for message in [initialize(), read(1), read(2)] {
            input.extend(serde_json::to_vec(&message).unwrap());
            input.push(b'\n');
        }
        let operations = service.run(Cursor::new(input), &mut output);
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("SENTINEL"), "{output}");
        let replies: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            replies.len(),
            2,
            "nothing is answered after the gate closes"
        );
        assert_eq!(
            tool_text(&replies[1]),
            (r#"{"refusal":"attempt_closed"}"#.to_owned(), true)
        );
        assert_eq!(
            operations,
            [Operation {
                sequence: 1,
                tool: Some("read_file"),
                outcome: Outcome::Withheld {
                    refusal: Refusal::AttemptClosed
                },
            }]
        );
        let ran = seen.lock().unwrap().len();
        assert_eq!(ran, usize::from(admit.is_ok()), "admission comes first");
    }
}

#[test]
fn an_operation_that_overruns_is_refused_and_stops_the_service() {
    let service = Service::with_serve(
        Box::new(|_| {
            thread::sleep(Duration::from_millis(400));
            Ok(file("SENTINEL"))
        }),
        Box::new(Open),
        Limits {
            operation: Duration::from_millis(20),
            deadline: None,
        },
    );
    let started = Instant::now();
    let (replies, operations) = run(service, &[initialize(), read(1), read(2)]);
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "the service did not wait"
    );
    assert_eq!(replies.len(), 2);
    assert_eq!(
        tool_text(&replies[1]),
        (r#"{"refusal":"limit_time"}"#.to_owned(), true)
    );
    assert!(matches!(
        operations[..],
        [Operation {
            outcome: Outcome::Refused {
                refusal: Refusal::LimitTime,
                ..
            },
            ..
        }]
    ));
}

#[test]
fn a_passed_deadline_refuses_before_the_reader_runs() {
    let (service, seen) = service_with(
        Ok(file("x")),
        Box::new(Open),
        Limits {
            operation: R0_OPERATION_TIME,
            deadline: Some(Instant::now()),
        },
    );
    let (replies, _) = run(service, &[initialize(), read(1), read(2)]);
    assert_eq!(replies.len(), 2);
    assert_eq!(
        tool_text(&replies[1]),
        (r#"{"refusal":"limit_time"}"#.to_owned(), true)
    );
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn an_overlong_message_ends_the_service() {
    let (service, seen) = service_with(Ok(file("x")), Box::new(Open), LIMITS);
    let mut input = serde_json::to_vec(&initialize()).unwrap();
    input.push(b'\n');
    input.extend(std::iter::repeat_n(b'a', 70 * 1024));
    input.push(b'\n');
    input.extend(serde_json::to_vec(&read(1)).unwrap());
    input.push(b'\n');
    let mut output = Vec::new();
    let operations = service.run(Cursor::new(input), &mut output);
    let replies: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["error"]["code"], -32600);
    assert!(operations.is_empty());
    assert!(seen.lock().unwrap().is_empty());
}

/// The tool and digest of each result a gate committed.
type Commits = Arc<Mutex<Vec<(Option<&'static str>, String)>>>;

/// A gate that answers as scripted and records what it committed.
struct Scripted {
    admit: Result<(), Refusal>,
    commit: Result<(), Refusal>,
    commits: Commits,
}

impl Gate for Scripted {
    fn admit(&mut self) -> Result<(), Refusal> {
        self.admit
    }

    fn commit(&mut self, tool: Option<&'static str>, digest: &str) -> Result<(), Refusal> {
        self.commits.lock().unwrap().push((tool, digest.to_owned()));
        self.commit
    }
}

#[cfg(unix)]
mod over_the_channel {
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::super::channel::{relay, Channel, CHANNEL_ENV, RELAY_ARGS, TOKEN_ENV};
    use super::super::open::ScopedReader;
    use super::super::service::{Gate, Open, Outcome, Service};
    use super::super::{Refusal, R0_READ_POLICY};
    use super::{file, initialize, read, tool_text, LIMITS};
    use crate::automations::scoped_read::scope::RootKind;

    /// A short scratch base: the channel's socket path has to fit in about
    /// 100 bytes.
    fn scratch() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("csr")
            .tempdir_in("/tmp")
            .expect("scratch directory")
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The socket and token the channel's MCP configuration gives the relay.
    fn relay_env(channel: &Channel) -> (PathBuf, String) {
        let config: Value =
            serde_json::from_slice(&fs::read(channel.config_path()).unwrap()).unwrap();
        let server = &config["mcpServers"]["coven"];
        (
            PathBuf::from(server["env"][CHANNEL_ENV].as_str().unwrap()),
            server["env"][TOKEN_ENV].as_str().unwrap().to_owned(),
        )
    }

    /// Writes `message` as one line and reads one reply line.
    fn exchange(stream: &mut UnixStream, replies: &mut impl BufRead, message: &Value) -> Value {
        let mut line = serde_json::to_vec(message).unwrap();
        line.push(b'\n');
        stream.write_all(&line).unwrap();
        let mut reply = String::new();
        replies.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap_or_else(|_| panic!("a reply line: {reply:?}"))
    }

    fn open(runtime: &Path, service: impl FnOnce(Box<dyn Gate>) -> Service) -> Channel {
        Channel::open(
            runtime,
            Path::new("/usr/local/bin/coven"),
            service,
            Box::new(Open),
        )
        .expect("the channel opens")
    }

    #[test]
    fn the_configuration_names_only_the_relay_and_stays_private() {
        let scratch = scratch();
        let runtime = scratch.path().join("runtime/scoped-read");
        // A runtime directory left too open is made private again.
        fs::create_dir_all(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
        let channel = open(&runtime, |gate| {
            Service::with_serve(Box::new(|_| Ok(file("x"))), gate, LIMITS)
        });
        let config: Value =
            serde_json::from_slice(&fs::read(channel.config_path()).unwrap()).unwrap();
        let servers = config["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        let server = &servers["coven"];
        assert_eq!(server["type"], "stdio");
        assert_eq!(server["command"], "/usr/local/bin/coven");
        assert_eq!(server["args"], json!(RELAY_ARGS));
        let (socket, token) = relay_env(&channel);
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let directory = channel.config_path().parent().unwrap().to_path_buf();
        assert_eq!(socket.parent(), Some(directory.as_path()));
        assert_eq!(mode(&runtime), 0o700);
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(channel.config_path()), 0o600);
        assert_eq!(mode(&socket), 0o600);

        assert!(channel.close().is_empty());
        assert!(!directory.exists(), "the channel removes what it created");
        assert!(runtime.is_dir(), "and nothing else");
    }

    #[test]
    fn a_failed_open_cleans_its_overlong_socket_directory() {
        let scratch = scratch();
        let runtime = scratch.path().join("long".repeat(30));
        fs::create_dir(&runtime).unwrap();
        fs::write(runtime.join("keep"), "unrelated entry").unwrap();
        let result = Channel::open(
            &runtime,
            Path::new("/usr/local/bin/coven"),
            |gate| Service::with_serve(Box::new(|_| Ok(file("x"))), gate, LIMITS),
            Box::new(Open),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(runtime.join("keep")).unwrap(), b"unrelated entry");
        assert_eq!(
            fs::read_dir(&runtime).unwrap().count(),
            1,
            "failed initialization must remove its attempt directory and preserve unrelated entries"
        );
    }

    #[test]
    fn a_failed_open_cleans_its_socket_before_the_thread_exists() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let scratch = scratch();
        let runtime = scratch.path().join("runtime");
        fs::create_dir(&runtime).unwrap();
        fs::write(runtime.join("keep"), "unrelated entry").unwrap();
        let relay = PathBuf::from(OsString::from_vec(b"/relay-\xff".to_vec()));
        let result = Channel::open(
            &runtime,
            &relay,
            |gate| Service::with_serve(Box::new(|_| Ok(file("x"))), gate, LIMITS),
            Box::new(Open),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(runtime.join("keep")).unwrap(), b"unrelated entry");
        assert_eq!(
            fs::read_dir(&runtime).unwrap().count(),
            1,
            "a configuration error must not leave the attempt directory or bound socket behind"
        );
    }

    #[test]
    fn the_call_limit_refusal_ends_the_service_and_its_records() {
        let scratch = scratch();
        let root = scratch.path().join("root");
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(root.join("notes/today.md"), "granted notes").unwrap();
        let mut policy = R0_READ_POLICY;
        policy.calls = 1;
        let reader = ScopedReader::new(
            &[json!({"kind": "filesystem", "root": "project", "path": "notes/today.md", "access": "read", "recursive": false})],
            &[(RootKind::Project, root.as_path())],
            &[],
            policy,
        )
        .unwrap();
        let (replies, operations) = super::run(
            Service::new(reader, Box::new(Open), LIMITS),
            &[initialize(), read(1), read(2), read(3)],
        );
        assert_eq!(
            operations.len(),
            2,
            "only the budgeted call and one terminal limit refusal may be retained"
        );
        assert_eq!(replies.len(), 3, "nothing is answered after exhaustion");
        assert!(matches!(operations[0].outcome, Outcome::Served { .. }));
        assert!(matches!(
            operations[1].outcome,
            Outcome::Refused {
                refusal: Refusal::LimitCalls,
                ..
            }
        ));
    }

    #[test]
    fn the_relay_carries_a_whole_session() {
        let scratch = scratch();
        let root = scratch.path().join("root");
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(root.join("notes/today.md"), "granted notes").unwrap();
        let reader = ScopedReader::new(
            &[json!({"kind": "filesystem", "root": "project", "path": "notes/today.md", "access": "read", "recursive": false})],
            &[(RootKind::Project, root.as_path())],
            &[],
            R0_READ_POLICY,
        )
        .unwrap();
        let channel = open(&scratch.path().join("runtime"), |gate| {
            Service::new(reader, gate, LIMITS)
        });
        let (socket, token) = relay_env(&channel);

        let mut input = Vec::new();
        for message in [initialize(), read(1), read(2)] {
            input.extend(serde_json::to_vec(&message).unwrap());
            input.push(b'\n');
        }
        let mut output = Vec::new();
        relay(&socket, &token, std::io::Cursor::new(input), &mut output).expect("the relay runs");
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 3);
        let (text, is_error) = tool_text(&replies[1]);
        assert!(!is_error, "{text}");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap()["content"],
            "granted notes"
        );
        let operations = channel.close();
        assert_eq!(operations.len(), 2);
        assert!(operations
            .iter()
            .all(|operation| matches!(operation.outcome, Outcome::Served { .. })));
    }

    #[test]
    fn only_the_first_connection_with_the_token_is_served() {
        let scratch = scratch();
        let channel = open(&scratch.path().join("runtime"), |gate| {
            Service::with_serve(Box::new(|_| Ok(file("x"))), gate, LIMITS)
        });
        let (socket, token) = relay_env(&channel);

        let mut stranger = UnixStream::connect(&socket).unwrap();
        stranger.write_all(b"0000\n").unwrap();
        stranger
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut nothing = String::new();
        BufReader::new(&stranger).read_line(&mut nothing).unwrap();
        assert_eq!(nothing, "", "a wrong token is disconnected");

        let mut relay = UnixStream::connect(&socket).unwrap();
        relay.write_all(format!("{token}\n").as_bytes()).unwrap();
        let mut replies = BufReader::new(relay.try_clone().unwrap());
        let reply = exchange(&mut relay, &mut replies, &initialize());
        assert_eq!(reply["result"]["serverInfo"]["name"], "coven-scoped-read");

        assert!(
            UnixStream::connect(&socket).is_err(),
            "the listener is gone once the relay is in"
        );
        drop(channel);
    }

    #[test]
    fn closing_withholds_a_running_operation_and_ends_the_relay() {
        let scratch = scratch();
        let started = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&started);
        let channel = open(&scratch.path().join("runtime"), move |gate| {
            Service::with_serve(
                Box::new(move |_| {
                    *flag.lock().unwrap() = true;
                    thread::sleep(Duration::from_millis(300));
                    Ok(file("SENTINEL"))
                }),
                gate,
                LIMITS,
            )
        });
        let (socket, token) = relay_env(&channel);
        let directory = channel.config_path().parent().unwrap().to_path_buf();

        let mut relay = UnixStream::connect(&socket).unwrap();
        relay.write_all(format!("{token}\n").as_bytes()).unwrap();
        let mut replies = BufReader::new(relay.try_clone().unwrap());
        exchange(&mut relay, &mut replies, &initialize());
        let mut line = serde_json::to_vec(&read(1)).unwrap();
        line.push(b'\n');
        relay.write_all(&line).unwrap();
        while !*started.lock().unwrap() {
            thread::sleep(Duration::from_millis(5));
        }

        let operations = channel.close();
        assert_eq!(operations.len(), 1);
        assert_eq!(
            operations[0].outcome,
            Outcome::Withheld {
                refusal: Refusal::AttemptClosed
            }
        );
        let mut rest = String::new();
        std::io::Read::read_to_string(&mut replies, &mut rest).unwrap();
        assert!(!rest.contains("SENTINEL"), "{rest}");
        assert!(!directory.exists());
    }

    #[test]
    fn closing_before_any_connection_removes_the_channel() {
        let scratch = scratch();
        let channel = open(&scratch.path().join("runtime"), |gate| {
            Service::with_serve(Box::new(|_| Ok(file("x"))), gate, LIMITS)
        });
        let (socket, token) = relay_env(&channel);
        let directory = channel.config_path().parent().unwrap().to_path_buf();
        assert!(channel.close().is_empty());
        assert!(!directory.exists());
        assert!(relay(&socket, &token, std::io::empty(), Vec::new()).is_err());
    }

    /// Keeps a partial token moving for about twelve seconds unless the
    /// channel rejects it first. A progress event starts the assertion after
    /// three bytes, and every path shuts down the peer and joins its writer.
    fn with_dripping_token<T>(socket: &Path, test: impl FnOnce(&mut UnixStream) -> T) -> T {
        let mut peer = UnixStream::connect(socket).unwrap();
        let mut writer = peer.try_clone().unwrap();
        thread::scope(|threads| {
            let (stop, stopped) = mpsc::channel();
            let (progress, progressed) = mpsc::channel();
            let sending = threads.spawn(move || {
                for byte in 0..64 {
                    if writer.write_all(b"x").is_err() {
                        break;
                    }
                    if byte == 2 {
                        let _ = progress.send(());
                    }
                    match stopped.recv_timeout(Duration::from_millis(200)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
            let ready = progressed.recv_timeout(Duration::from_secs(5));
            let outcome = ready.is_ok().then(|| test(&mut peer));
            let _ = stop.send(());
            let _ = peer.shutdown(std::net::Shutdown::Both);
            sending.join().expect("the token writer joins");
            ready.expect("the peer presented a partial token");
            outcome.expect("the channel assertion ran")
        })
    }

    #[test]
    fn a_slow_handshake_has_one_total_deadline() {
        let scratch = scratch();
        let channel = open(&scratch.path().join("runtime"), |gate| {
            Service::with_serve(Box::new(|_| Ok(file("SENTINEL"))), gate, LIMITS)
        });
        let (socket, _) = relay_env(&channel);
        let directory = channel.config_path().parent().unwrap().to_path_buf();
        let result = with_dripping_token(&socket, |peer| {
            // Five seconds distinguishes the existing two-second total
            // budget from the injected twelve-second byte stream, with
            // scheduler margin; it is not a new production deadline.
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            peer.read(&mut [0u8; 1])
        });
        let operations = channel.close();
        assert!(operations.is_empty());
        assert!(!directory.exists());
        assert!(
            matches!(result, Ok(0)),
            "the unauthenticated peer must be disconnected within the total handshake budget, not kept alive by each byte: {result:?}"
        );
    }

    #[test]
    fn closing_during_a_slow_handshake_waits_only_for_its_total_budget() {
        let scratch = scratch();
        let channel = open(&scratch.path().join("runtime"), |gate| {
            Service::with_serve(Box::new(|_| Ok(file("SENTINEL"))), gate, LIMITS)
        });
        let (socket, _) = relay_env(&channel);
        let directory = channel.config_path().parent().unwrap().to_path_buf();
        let (result, closing) = with_dripping_token(&socket, |_| {
            let (finished, completion) = mpsc::channel();
            let closing = thread::spawn(move || {
                let operations = channel.close();
                let _ = finished.send(());
                operations
            });
            (completion.recv_timeout(Duration::from_secs(5)), closing)
        });
        let operations = closing.join().expect("the closing channel joins");
        assert!(operations.is_empty());
        assert!(!directory.exists());
        assert!(
            result.is_ok(),
            "a partial token must not keep channel close waiting beyond the total handshake budget: {result:?}"
        );
    }

    /// What the scripted model answers to each model request, in order.
    enum Reply {
        /// A 400, as a capture-only server gives.
        Reject,
        /// One `tool_use` block calling `name` with `input`.
        ToolUse { name: &'static str, input: Value },
        /// A final text answer.
        Text(&'static str),
    }

    /// Each request's line and body, as the stand-in model received them.
    type Requests = Arc<Mutex<Vec<(String, String)>>>;

    /// A local stand-in for the model API. It records each request as its
    /// request line and body, answers model requests from `script` in order,
    /// and rejects everything else.
    fn model_server(script: Vec<Reply>) -> (u16, Requests) {
        use std::io::Read;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        thread::spawn(move || {
            let mut script = script.into_iter().enumerate();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = Vec::new();
                let mut buffer = [0u8; 64 * 1024];
                let body_start = loop {
                    let Ok(read) = stream.read(&mut buffer) else {
                        break None;
                    };
                    if read == 0 {
                        break None;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                        break Some(end + 4);
                    }
                };
                let Some(body_start) = body_start else {
                    continue;
                };
                let head = String::from_utf8_lossy(&request[..body_start]).to_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while request.len() < body_start + length {
                    let Ok(read) = stream.read(&mut buffer) else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let line = head.lines().next().unwrap_or_default().to_owned();
                let body = String::from_utf8_lossy(&request[body_start..]).into_owned();
                let model = line.starts_with("post /v1/messages ")
                    || line.starts_with("post /v1/messages?");
                captured.lock().unwrap().push((line, body));
                let (turn, reply) = if model {
                    script.next().unwrap_or((usize::MAX, Reply::Reject))
                } else {
                    (usize::MAX, Reply::Reject)
                };
                let (status, kind, payload) = match reply {
                    Reply::Reject => (
                        "400 Bad Request",
                        "application/json",
                        r#"{"type":"error","error":{"type":"invalid_request_error","message":"capture only"}}"#.to_owned(),
                    ),
                    Reply::ToolUse { name, input } => (
                        "200 OK",
                        "text/event-stream",
                        events(
                            json!({"type": "tool_use", "id": format!("toolu_scripted_{turn}"), "name": name, "input": {}}),
                            json!({"type": "input_json_delta", "partial_json": input.to_string()}),
                            "tool_use",
                        ),
                    ),
                    Reply::Text(text) => (
                        "200 OK",
                        "text/event-stream",
                        events(
                            json!({"type": "text", "text": ""}),
                            json!({"type": "text_delta", "text": text}),
                            "end_turn",
                        ),
                    ),
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                        payload.len()
                    )
                    .as_bytes(),
                );
            }
        });
        (port, requests)
    }

    /// A streamed Messages API answer holding one content block.
    fn events(block: Value, delta: Value, stop: &str) -> String {
        [
            ("message_start", json!({"type": "message_start", "message": {"id": "msg_scripted", "type": "message", "role": "assistant", "model": "claude-scripted", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 1}}})),
            ("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": block})),
            ("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": delta})),
            ("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": null}, "usage": {"output_tokens": 5}})),
            ("message_stop", json!({"type": "message_stop"})),
        ]
        .iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
        .collect()
    }

    /// One run of the installed Claude Code against a channel over a
    /// scratch project, launched the way the envelope will launch it.
    struct Harness {
        scratch: tempfile::TempDir,
        relay: PathBuf,
        claude: PathBuf,
    }

    const SERVICE_TOOLS: [&str; 3] = [
        "mcp__coven__list_directory",
        "mcp__coven__read_file",
        "mcp__coven__search",
    ];

    impl Harness {
        /// The scratch project holds `notes/today.md`, which the grant
        /// names, `notes/secret.md`, which it does not, and memory files
        /// where a harness looks for them on its own. Each of the last
        /// three holds the sentinel, which must never reach the model.
        fn new() -> Self {
            let relay = fs::canonicalize(
                std::env::var_os("COVEN_TEST_RELAY_BIN")
                    .expect("COVEN_TEST_RELAY_BIN names the coven binary"),
            )
            .unwrap();
            let claude = std::env::var_os("PATH")
                .and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|directory| directory.join("claude"))
                        .find(|candidate| candidate.is_file())
                })
                .expect("claude on PATH");
            let scratch = scratch();
            let base = scratch.path();
            fs::create_dir_all(base.join("root/notes")).unwrap();
            fs::write(base.join("root/notes/today.md"), "granted notes").unwrap();
            fs::write(base.join("root/notes/secret.md"), "SENTINEL secret").unwrap();
            fs::create_dir_all(base.join("home/.claude")).unwrap();
            fs::create_dir_all(base.join("launch")).unwrap();
            fs::write(base.join("home/.claude/CLAUDE.md"), "SENTINEL home memory").unwrap();
            fs::write(base.join("launch/CLAUDE.md"), "SENTINEL project memory").unwrap();
            fs::write(base.join("CLAUDE.md"), "SENTINEL parent memory").unwrap();
            Self {
                scratch,
                relay,
                claude,
            }
        }

        fn channel(&self) -> Channel {
            let root = self.scratch.path().join("root");
            let reader = ScopedReader::new(
                &[json!({"kind": "filesystem", "root": "project", "path": "notes/today.md", "access": "read", "recursive": false})],
                &[(RootKind::Project, root.as_path())],
                &[],
                R0_READ_POLICY,
            )
            .unwrap();
            Channel::open(
                &self.scratch.path().join("runtime"),
                &self.relay,
                |gate| Service::new(reader, gate, LIMITS),
                Box::new(Open),
            )
            .unwrap()
        }

        /// Runs claude to completion and returns its stream's events.
        fn run(&self, channel: &Channel, port: u16) -> Vec<Value> {
            use std::io::Read;
            use std::process::{Command, Stdio};
            use std::time::Instant;

            let base = self.scratch.path();
            let mut child = Command::new(&self.claude)
                .env_clear()
                .env("HOME", base.join("home"))
                .env("PATH", "/usr/bin:/bin")
                .env("ANTHROPIC_API_KEY", "sk-ant-test-capture-only")
                .env("ANTHROPIC_BASE_URL", format!("http://127.0.0.1:{port}"))
                .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
                .current_dir(base.join("launch"))
                .args([
                    "--bare",
                    "--print",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                ])
                .args([
                    "--tools",
                    "",
                    "--strict-mcp-config",
                    "--setting-sources",
                    "",
                ])
                .args([
                    "--allowedTools",
                    "mcp__coven__read_file,mcp__coven__list_directory,mcp__coven__search",
                ])
                .args(["--no-session-persistence", "--disable-slash-commands"])
                .arg("--mcp-config")
                .arg(channel.config_path())
                .args(["--", "Read notes/today.md and tell me what it says."])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("claude starts");
            let mut stdout = child.stdout.take().unwrap();
            let reading = thread::spawn(move || {
                let mut text = String::new();
                let _ = stdout.read_to_string(&mut text);
                text
            });
            let started = Instant::now();
            while child.try_wait().unwrap().is_none() {
                if started.elapsed() > Duration::from_secs(120) {
                    let _ = child.kill();
                    panic!("claude did not finish in two minutes");
                }
                thread::sleep(Duration::from_millis(100));
            }
            reading
                .join()
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }
    }

    /// The init event's tools, sorted, after checking it names only the
    /// service as an MCP server.
    fn init_tools(events: &[Value]) -> Vec<String> {
        let init = events
            .iter()
            .find(|event| event["type"] == "system" && event["subtype"] == "init")
            .unwrap_or_else(|| panic!("an init event in {events:?}"));
        eprintln!(
            "claude {} init: tools {}, mcp_servers {}, permissionMode {}, apiKeySource {}",
            init["claude_code_version"],
            init["tools"],
            init["mcp_servers"],
            init["permissionMode"],
            init["apiKeySource"]
        );
        let servers = init["mcp_servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1, "{servers:?}");
        assert_eq!(servers[0]["name"], "coven");
        assert_eq!(servers[0]["status"], "connected");
        let mut tools: Vec<String> = init["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool.as_str().unwrap().to_owned())
            .collect();
        tools.sort_unstable();
        tools
    }

    /// The model requests, checking each offers only the service's tools and
    /// carries no sentinel.
    fn model_requests(requests: &Requests) -> Vec<Value> {
        let requests = requests.lock().unwrap().clone();
        for (_, body) in &requests {
            assert!(
                !body.contains("SENTINEL"),
                "unauthorized content reached the model"
            );
        }
        let model: Vec<Value> = requests
            .iter()
            .filter(|(line, _)| line.starts_with("post /v1/messages"))
            .map(|(_, body)| serde_json::from_str(body).unwrap())
            .collect();
        assert!(
            !model.is_empty(),
            "claude sent no model request: {requests:?}"
        );
        for body in &model {
            let mut sent: Vec<&str> = body["tools"]
                .as_array()
                .map(|tools| {
                    tools
                        .iter()
                        .map(|tool| tool["name"].as_str().unwrap())
                        .collect()
                })
                .unwrap_or_default();
            sent.sort_unstable();
            assert_eq!(sent, SERVICE_TOOLS);
        }
        model
    }

    /// The text of the last tool result in a model request.
    fn last_tool_result(body: &Value) -> String {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
            .rfind(|block| block["type"] == "tool_result")
            .map(|block| block["content"].to_string())
            .unwrap_or_else(|| panic!("a tool result in {body}"))
    }

    /// Launches the installed Claude Code at the service through the real
    /// relay, with a dummy key and a local server that captures the model
    /// request and answers 400, so nothing is billed. Run with the `coven`
    /// binary to relay through and `claude` on PATH:
    /// `COVEN_TEST_RELAY_BIN=$PWD/target/debug/coven cargo test -p coven-cli --bin coven -- --ignored --nocapture claude_`
    #[test]
    #[ignore = "launches the installed claude; needs COVEN_TEST_RELAY_BIN"]
    fn claude_reaches_only_the_service() {
        let harness = Harness::new();
        let channel = harness.channel();
        let (port, requests) = model_server(vec![Reply::Reject]);
        let events = harness.run(&channel, port);
        assert_eq!(init_tools(&events), SERVICE_TOOLS);
        model_requests(&requests);
        assert!(channel.close().is_empty());
    }

    /// The same launch, with the local server scripting the model: it reads
    /// the granted file, then a sibling the grant does not name, then
    /// answers. Both calls go through the relay to the reader in one run;
    /// the first returns the file and the second is refused.
    #[test]
    #[ignore = "launches the installed claude; needs COVEN_TEST_RELAY_BIN"]
    fn claude_reads_through_the_service() {
        let harness = Harness::new();
        let channel = harness.channel();
        let (port, requests) = model_server(vec![
            Reply::ToolUse {
                name: "mcp__coven__read_file",
                input: json!({"root": "project", "path": "notes/today.md"}),
            },
            Reply::ToolUse {
                name: "mcp__coven__read_file",
                input: json!({"root": "project", "path": "notes/secret.md"}),
            },
            Reply::Text("done"),
        ]);
        let events = harness.run(&channel, port);
        assert_eq!(init_tools(&events), SERVICE_TOOLS);

        let model = model_requests(&requests);
        assert_eq!(model.len(), 3, "one request per scripted reply");
        let granted = last_tool_result(&model[1]);
        assert!(granted.contains("granted notes"), "{granted}");
        let refused = last_tool_result(&model[2]);
        assert!(refused.contains("scope_missing"), "{refused}");
        eprintln!("tool results reaching the model: {granted} then {refused}");

        let result = events
            .iter()
            .find(|event| event["type"] == "result")
            .expect("a result event");
        assert_eq!(result["subtype"], "success", "{result}");
        assert_eq!(result["result"], "done");

        let operations = channel.close();
        let outcomes: Vec<&Outcome> = operations
            .iter()
            .map(|operation| &operation.outcome)
            .collect();
        assert!(matches!(
            outcomes[..],
            [
                Outcome::Served { .. },
                Outcome::Refused {
                    refusal: Refusal::ScopeMissing,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn a_runtime_directory_that_is_a_link_is_refused() {
        let scratch = scratch();
        let elsewhere = scratch.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let runtime = scratch.path().join("runtime");
        std::os::unix::fs::symlink(&elsewhere, &runtime).unwrap();
        let opened = Channel::open(
            &runtime,
            Path::new("/usr/local/bin/coven"),
            |gate| Service::with_serve(Box::new(|_| Ok(file("x"))), gate, LIMITS),
            Box::new(Open),
        );
        assert!(opened.is_err());
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }
}
