//! The scoped read service as the harness sees it: an MCP server speaking
//! newline-delimited JSON-RPC 2.0 over one stream.
//!
//! The harness's only MCP server is `coven automations scoped-read-relay`,
//! which connects to the attempt's private channel and copies bytes both
//! ways. Everything else runs in the daemon: the protocol, the tool schemas,
//! the reader, and the [`Gate`] that admits each call and commits each result
//! before a byte of it is written.
//!
//! Every `tools/call` reaches the reader, an unknown tool or malformed
//! arguments included, so every call counts against the attempt's limit and
//! is answered as a refused tool result. Each operation runs on a worker
//! thread under a time limit: the policy's, or less when the attempt's
//! deadline is nearer. One that overruns is refused (`limit_time`) and the
//! service stops, because the worker cannot be interrupted and no later call
//! may share the reader with it.

use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::open::{Response, ScopedReader};
use super::Refusal;
use crate::automations::contract::canonical_json::{canonicalize, sha256_hex};

/// The MCP server name. The harness names the tools `mcp__coven__<tool>`.
pub(crate) const SERVER_NAME: &str = "coven";
/// The version of the tool names and input schemas below. The runtime
/// envelope's descriptor binds it.
pub(crate) const TOOLS_VERSION: &str = "coven.scoped-read.tools.v1";
/// The tools, in the order `tools/list` gives them.
pub(crate) const TOOL_NAMES: &[&str] = &["read_file", "list_directory", "search"];
/// The design's time limit for one operation.
pub(crate) const R0_OPERATION_TIME: Duration = Duration::from_secs(5);
/// MCP revisions the service speaks, newest first. A client asking for any
/// other gets the newest, as the protocol's version negotiation says.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// The longest message the service reads. A request carries a path of at
/// most 4,096 bytes and a pattern of at most 256, so nothing near this is a
/// request the service could serve.
const MESSAGE_BYTES: usize = 64 * 1024;

/// The attempt's authority, consulted around every operation.
pub(crate) trait Gate: Send {
    /// Before an operation runs: the attempt is live and its grant current.
    fn admit(&mut self) -> Result<(), Refusal>;
    /// After a result is built and before any of it is written: authorizes
    /// exactly the result `committing` names by its digest. If cancellation
    /// or revocation committed first, this refuses and the result is
    /// discarded. Once it returns `Ok`, those bytes are authorized and cannot
    /// be recalled.
    fn commit(&mut self, committing: &Committing<'_>) -> Result<(), Refusal>;
}

/// The time an operation may take.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The most one operation may take.
    pub operation: Duration,
    /// When the attempt's run ends, if it has a deadline.
    pub deadline: Option<Instant>,
}

/// What happened to one tool call. It holds no file content and no path, so
/// the observer can record it as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Operation {
    /// The call's place in the attempt, from 1.
    pub sequence: u64,
    /// The tool, when the call named one of the service's.
    pub tool: Option<&'static str>,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The result was committed and written. `digest` is the SHA-256 of its
    /// JCS text, which is exactly the text the harness received.
    Served { digest: String },
    /// The request was refused, and the refusal was committed and written.
    Refused { refusal: Refusal, digest: String },
    /// The gate refused: nothing the reader produced was written.
    Withheld { refusal: Refusal },
}

/// What a commit authorizes: one operation's result, named by its digest. It
/// holds no file content, no path, no pattern and no entry name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Committing<'a> {
    /// The call's place in the attempt, from 1.
    pub sequence: u64,
    /// The tool, when the call named one of the service's.
    pub tool: Option<&'static str>,
    /// `None` when the result serves the request, or the reader's refusal.
    pub refusal: Option<Refusal>,
    /// The SHA-256 of the result's JCS text.
    pub digest: &'a str,
    /// The length of that text in bytes.
    pub bytes: usize,
}

type Serve = Box<dyn FnMut(&Value) -> Result<Response, Refusal> + Send>;
type Job = (Value, mpsc::Sender<Result<Response, Refusal>>);

pub(crate) struct Service {
    serve: Serve,
    gate: Box<dyn Gate>,
    limits: Limits,
}

impl Service {
    pub(crate) fn new(mut reader: ScopedReader, gate: Box<dyn Gate>, limits: Limits) -> Self {
        Self::with_serve(Box::new(move |request| reader.serve(request)), gate, limits)
    }

    /// A service whose operations `serve` answers, so tests can stand in for
    /// the reader.
    pub(crate) fn with_serve(serve: Serve, gate: Box<dyn Gate>, limits: Limits) -> Self {
        Self {
            serve,
            gate,
            limits,
        }
    }

    /// Serves `input` until it ends, the attempt closes or an operation
    /// overruns, writing every answer to `output`. Returns what each tool
    /// call did.
    pub(crate) fn run(self, mut input: impl BufRead, mut output: impl Write) -> Vec<Operation> {
        let Self {
            mut serve,
            mut gate,
            limits,
        } = self;
        let (jobs, queue) = mpsc::channel::<Job>();
        let worker = thread::Builder::new()
            .name("scoped-read".to_owned())
            .spawn(move || {
                for (request, answer) in queue {
                    // The service has stopped waiting when the send fails.
                    let _ = answer.send(serve(&request));
                }
            });
        let mut session = Session {
            jobs: worker.is_ok().then_some(jobs),
            gate: gate.as_mut(),
            limits,
            initialized: false,
            operations: Vec::new(),
        };
        let mut message = Vec::new();
        loop {
            message.clear();
            let reply = match read_message(&mut input, &mut message) {
                Ok(Incoming::End) | Err(_) => break,
                Ok(Incoming::TooLong) => {
                    let _ = write(
                        &mut output,
                        &error(&Value::Null, -32600, "message too long"),
                    );
                    break;
                }
                Ok(Incoming::Message) => session.answer(&message),
            };
            let Some(reply) = reply else { continue };
            if write(&mut output, &reply.message).is_err() || reply.stop {
                break;
            }
        }
        // Dropping the queue ends the worker once its operation returns.
        session.operations
    }
}

struct Session<'a> {
    /// `None` once an operation overran, or when the worker never started.
    jobs: Option<mpsc::Sender<Job>>,
    gate: &'a mut dyn Gate,
    limits: Limits,
    initialized: bool,
    operations: Vec<Operation>,
}

struct Reply {
    message: Value,
    /// The service serves nothing after this reply.
    stop: bool,
}

impl Session<'_> {
    /// The reply to one message, or `None` for a notification or a response.
    fn answer(&mut self, bytes: &[u8]) -> Option<Reply> {
        let reply = |message| {
            Some(Reply {
                message,
                stop: false,
            })
        };
        let Ok(message) = serde_json::from_slice::<Value>(bytes) else {
            return reply(error(&Value::Null, -32700, "parse error"));
        };
        let Some(object) = message.as_object() else {
            return reply(error(&Value::Null, -32600, "invalid request"));
        };
        let method = object.get("method").and_then(Value::as_str);
        let id = match object.get("id") {
            // A notification, such as `notifications/initialized`, or a
            // response to a request the service never makes.
            None => return None,
            Some(id @ (Value::String(_) | Value::Number(_))) => id.clone(),
            Some(_) => return reply(error(&Value::Null, -32600, "invalid request")),
        };
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return reply(error(&id, -32600, "invalid request"));
        }
        let params = object.get("params");
        match method {
            Some("initialize") => {
                self.initialized = true;
                reply(result(&id, initialize(params)))
            }
            Some("ping") => reply(result(&id, json!({}))),
            Some(_) if !self.initialized => reply(error(&id, -32002, "not initialized")),
            Some("tools/list") => reply(result(&id, json!({ "tools": tool_definitions() }))),
            Some("tools/call") => {
                let (outcome, stop) = self.call(params);
                Some(Reply {
                    message: result(&id, outcome),
                    stop,
                })
            }
            Some(_) => reply(error(&id, -32601, "method not found")),
            None => reply(error(&id, -32600, "invalid request")),
        }
    }

    /// Runs one tool call, returning its MCP result and whether the service
    /// stops after it.
    fn call(&mut self, params: Option<&Value>) -> (Value, bool) {
        let sequence = self.operations.len() as u64 + 1;
        let (tool, request) = request_for(params);
        let (outcome, delivered, stop) = match self.gate.admit() {
            Err(refusal) => (Outcome::Withheld { refusal }, refused(refusal), true),
            Ok(()) => {
                let answer = self.run_job(request);
                // An overrun or a lost worker leaves no reader to serve with.
                let stop = self.jobs.is_none();
                let payload = match &answer {
                    Ok(response) => response.to_json(),
                    Err(refusal) => json!({ "refusal": refusal.code() }),
                };
                let text =
                    String::from_utf8(canonicalize(&payload).expect("a tool result is I-JSON"))
                        .expect("JCS text is UTF-8");
                let digest = sha256_hex(text.as_bytes());
                let committing = Committing {
                    sequence,
                    tool,
                    refusal: answer.as_ref().err().copied(),
                    digest: &digest,
                    bytes: text.len(),
                };
                match self.gate.commit(&committing) {
                    Err(refusal) => (Outcome::Withheld { refusal }, refused(refusal), true),
                    Ok(()) => {
                        let is_error = answer.is_err();
                        let outcome = match answer {
                            Ok(_) => Outcome::Served { digest },
                            Err(refusal) => Outcome::Refused { refusal, digest },
                        };
                        (outcome, tool_result(text, is_error), stop)
                    }
                }
            }
        };
        self.operations.push(Operation {
            sequence,
            tool,
            outcome,
        });
        (delivered, stop)
    }

    fn run_job(&mut self, request: Value) -> Result<Response, Refusal> {
        let now = Instant::now();
        let limit = match self.limits.deadline {
            Some(deadline) if deadline <= now => {
                // The attempt's time is up, so nothing more is served.
                self.jobs = None;
                return Err(Refusal::LimitTime);
            }
            Some(deadline) => self.limits.operation.min(deadline - now),
            None => self.limits.operation,
        };
        let jobs = self.jobs.as_ref().ok_or(Refusal::ResourceUnavailable)?;
        let (answer, answered) = mpsc::channel();
        jobs.send((request, answer))
            .map_err(|_| Refusal::ResourceUnavailable)?;
        match answered.recv_timeout(limit) {
            Ok(answer) => answer,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.jobs = None;
                Err(Refusal::LimitTime)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.jobs = None;
                Err(Refusal::ResourceUnavailable)
            }
        }
    }
}

/// The reader request a `tools/call` asks for, and the tool it names. A call
/// the reader cannot parse still goes to it, so the call is counted and
/// refused there.
fn request_for(params: Option<&Value>) -> (Option<&'static str>, Value) {
    let invalid = || json!({ "operation": Value::Null });
    let Some(params) = params.and_then(Value::as_object) else {
        return (None, invalid());
    };
    let name = params.get("name").and_then(Value::as_str);
    let tool = name.and_then(|name| TOOL_NAMES.iter().copied().find(|tool| *tool == name));
    let arguments = match params.get("arguments") {
        None => Map::new(),
        Some(Value::Object(arguments)) => arguments.clone(),
        Some(_) => return (tool, invalid()),
    };
    // A tool's arguments never name the operation; the tool does.
    if arguments.contains_key("operation") {
        return (tool, invalid());
    }
    let mut request = arguments;
    request.insert(
        "operation".to_owned(),
        name.map_or(Value::Null, |name| Value::String(name.to_owned())),
    );
    (tool, Value::Object(request))
}

fn initialize(params: Option<&Value>) -> Value {
    let requested = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str);
    let version = PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|version| Some(*version) == requested)
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "coven-scoped-read", "version": TOOLS_VERSION },
    })
}

/// The tools as `tools/list` gives them. The runtime envelope's descriptor
/// binds these through [`TOOLS_VERSION`].
pub(crate) fn tool_definitions() -> Value {
    let root = json!({
        "type": "string",
        "enum": ["project", "workspace"],
        "description": "Which granted root `path` is relative to.",
    });
    let path = |what: &str| {
        json!({
            "type": "string",
            "description": format!("The {what}, relative to the root, with `/` between names."),
        })
    };
    json!([
        {
            "name": "read_file",
            "description": "Read a text file the owner granted.",
            "inputSchema": {
                "type": "object",
                "properties": { "root": root, "path": path("file") },
                "required": ["root", "path"],
                "additionalProperties": false,
            },
        },
        {
            "name": "list_directory",
            "description": "List a directory the owner granted: each entry's name and kind, up to 256 a page in byte order. To continue, pass the page's `next` as `after`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": root,
                    "path": path("directory"),
                    "after": { "type": "string", "description": "The name to continue after." },
                },
                "required": ["root", "path"],
                "additionalProperties": false,
            },
        },
        {
            "name": "search",
            "description": "Find the lines containing `pattern`, as literal text, in the files beneath a directory the owner granted: up to 256 a page. To continue, pass the page's `next` as `after`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": root,
                    "path": path("directory"),
                    "pattern": { "type": "string", "description": "The text to find, 1 to 256 bytes, on one line." },
                    "after": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "line": { "type": "integer", "minimum": 1 },
                        },
                        "required": ["path", "line"],
                        "additionalProperties": false,
                        "description": "The file and line to continue after.",
                    },
                },
                "required": ["root", "path", "pattern"],
                "additionalProperties": false,
            },
        },
    ])
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// The result of a call the gate refused. It names only the attempt's state.
fn refused(refusal: Refusal) -> Value {
    tool_result(json!({ "refusal": refusal.code() }).to_string(), true)
}

fn result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn write(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    output.write_all(&line)?;
    output.flush()
}

enum Incoming {
    Message,
    TooLong,
    End,
}

/// Reads one newline-terminated message into `message`, holding at most
/// [`MESSAGE_BYTES`] of it.
fn read_message(input: &mut impl BufRead, message: &mut Vec<u8>) -> io::Result<Incoming> {
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(if message.is_empty() {
                Incoming::End
            } else {
                Incoming::Message
            });
        }
        let (taken, ended) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        if message.len() + taken > MESSAGE_BYTES {
            return Ok(Incoming::TooLong);
        }
        message.extend_from_slice(&available[..taken]);
        input.consume(taken);
        if ended {
            return Ok(Incoming::Message);
        }
    }
}

/// A gate that admits everything, for tests of the protocol alone.
#[cfg(test)]
pub(crate) struct Open;

#[cfg(test)]
impl Gate for Open {
    fn admit(&mut self) -> Result<(), Refusal> {
        Ok(())
    }

    fn commit(&mut self, _committing: &Committing<'_>) -> Result<(), Refusal> {
        Ok(())
    }
}
