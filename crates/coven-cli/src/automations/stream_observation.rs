//! Classifies a structured runtime stream for the terminal observer
//! (coven#857, slice 5b).
//!
//! The observer calls this only for a session whose signed binding pins a
//! structured runtime envelope, so every line is the harness's own event and
//! never model text. Coverage is complete only when the stream accounts for
//! everything:
//! - an init event whose tools, MCP servers, permission mode and harness
//!   version are within the envelope;
//! - every later line a known event;
//! - every tool call one the envelope classifies;
//! - exactly one final result;
//! - no dropped output.
//!
//! Anything else makes coverage partial. The evidence then says only what was
//! seen, and the consumer holds the run rather than settling it.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::contract::types::SideEffectClass;
use super::runtime_envelope::RuntimeEnvelope;

/// What the stream says the session produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamResult {
    /// The final result text, from a successful result event.
    Produced(String),
    /// The result event reports an error, or no result text.
    NotProduced,
    /// No result event was seen.
    Unobserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamObservation {
    /// The capabilities the classified tool calls exercised.
    pub capabilities: BTreeSet<&'static str>,
    /// The highest side-effect class among them.
    pub side_effects: SideEffectClass,
    /// How many tool calls the envelope classifies the stream shows.
    pub tool_calls: usize,
    pub complete: bool,
    pub result: StreamResult,
}

/// Classifies claude `--output-format stream-json` output. `output` is the
/// session's recorded output in order; `truncated` says some of it was
/// dropped.
pub(crate) fn classify_claude_stream(
    envelope: &RuntimeEnvelope,
    output: &str,
    truncated: bool,
) -> StreamObservation {
    let mut observation = StreamObservation {
        capabilities: BTreeSet::new(),
        side_effects: SideEffectClass::None,
        tool_calls: 0,
        complete: !truncated,
        result: StreamResult::Unobserved,
    };
    let mut initialized = false;
    let mut finished = false;
    // A final line without its newline was cut off.
    let (lines, rest) = output.rsplit_once('\n').unwrap_or(("", output));
    if !rest.is_empty() {
        observation.complete = false;
    }
    for line in lines.split('\n').filter(|line| !line.is_empty()) {
        let Ok(Value::Object(event)) = serde_json::from_str::<Value>(line) else {
            observation.complete = false;
            continue;
        };
        let kind = event.get("type").and_then(Value::as_str);
        let subtype = event.get("subtype").and_then(Value::as_str);
        // Nothing but wrapped stderr may follow the result.
        if finished && !(kind == Some("system") && subtype == Some("stderr")) {
            observation.complete = false;
        }
        match (kind, subtype) {
            (Some("system"), Some("init")) => {
                if initialized || !init_within(envelope, &event) {
                    observation.complete = false;
                }
                initialized = true;
            }
            // Diagnostics: wrapped stderr, token counts and the UI render
            // hint claude 2.1.304 and later emit before init, plus the
            // separate top-level rate-limit event. None carries model or
            // tool content. Unknown system events remain partial so a new
            // provider event cannot silently widen the evidence we treat as
            // complete.
            (Some("system"), Some("stderr" | "thinking_tokens" | "ui_invalidate"))
            | (Some("rate_limit_event"), _) => {}
            (Some("assistant"), _) => {
                observation.complete &= initialized;
                let Some(blocks) = content(&event) else {
                    observation.complete = false;
                    continue;
                };
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text" | "thinking" | "redacted_thinking") => {}
                        Some("tool_use") => {
                            let tool = block
                                .get("name")
                                .and_then(Value::as_str)
                                .and_then(|name| envelope.tool(name));
                            match tool {
                                Some(tool) => {
                                    observation.tool_calls += 1;
                                    observation.capabilities.insert(tool.capability);
                                    if rank(tool.side_effect) > rank(observation.side_effects) {
                                        observation.side_effects = tool.side_effect;
                                    }
                                }
                                None => observation.complete = false,
                            }
                        }
                        _ => observation.complete = false,
                    }
                }
            }
            (Some("user"), _) => {
                observation.complete &= initialized;
                let Some(blocks) = content(&event) else {
                    observation.complete = false;
                    continue;
                };
                for block in blocks {
                    if !matches!(
                        block.get("type").and_then(Value::as_str),
                        Some("tool_result" | "text")
                    ) {
                        observation.complete = false;
                    }
                }
            }
            (Some("result"), _) => {
                observation.complete &= initialized && !finished;
                finished = true;
                let succeeded = subtype == Some("success")
                    && event.get("is_error").and_then(Value::as_bool) == Some(false);
                observation.result = match event.get("result").and_then(Value::as_str) {
                    Some(text) if succeeded => StreamResult::Produced(text.to_owned()),
                    _ => StreamResult::NotProduced,
                };
            }
            _ => observation.complete = false,
        }
    }
    observation.complete &= initialized && finished;
    observation
}

/// The init event lists no tool outside the envelope and exactly the
/// envelope's MCP servers, each connected: none, or its scoped read service.
/// It reports the envelope's permission mode and, when the envelope was
/// proven against exact harness versions, one of them.
fn init_within(envelope: &RuntimeEnvelope, init: &Map<String, Value>) -> bool {
    let tools_within = init
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().all(|tool| {
                tool.as_str()
                    .is_some_and(|name| envelope.tool(name).is_some())
            })
        });
    let expected: &[&str] = envelope
        .service
        .as_ref()
        .map_or(&[], |service| std::slice::from_ref(&service.server));
    let servers_exact = init
        .get("mcp_servers")
        .and_then(Value::as_array)
        .is_some_and(|servers| {
            servers.len() == expected.len()
                && servers.iter().zip(expected).all(|(server, name)| {
                    server.get("name").and_then(Value::as_str) == Some(name)
                        && server.get("status").and_then(Value::as_str) == Some("connected")
                })
        });
    let version_admitted = envelope.harness_versions.is_empty()
        || init
            .get("claude_code_version")
            .and_then(Value::as_str)
            .is_some_and(|version| envelope.harness_versions.contains(&version));
    let permission = init.get("permissionMode").and_then(Value::as_str);
    tools_within
        && servers_exact
        && version_admitted
        && permission == Some(envelope.permission_mode)
}

/// A message's content blocks, or `None` when they are not a list of
/// objects, which would hide what the message did.
fn content(event: &Map<String, Value>) -> Option<Vec<&Map<String, Value>>> {
    event
        .get("message")?
        .get("content")?
        .as_array()?
        .iter()
        .map(Value::as_object)
        .collect()
}

const fn rank(class: SideEffectClass) -> u8 {
    match class {
        SideEffectClass::None => 0,
        SideEffectClass::LocalRead => 1,
        SideEffectClass::LocalWrite => 2,
        SideEffectClass::ExternalRead => 3,
        SideEffectClass::ExternalMutation => 4,
        SideEffectClass::IrreversibleExternalMutation => 5,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use serde_json::json;

    pub(crate) fn init(tools: &[&str]) -> String {
        json!({
            "type": "system",
            "subtype": "init",
            "tools": tools,
            "mcp_servers": [],
            "permissionMode": "plan",
            "session_id": "native-session"
        })
        .to_string()
    }

    pub(crate) fn tool_use(name: &str) -> String {
        json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "tool_use", "id": "toolu_1", "name": name, "input": { "file_path": "note.txt" } }
            ] }
        })
        .to_string()
    }

    pub(crate) fn tool_result() -> String {
        json!({
            "type": "user",
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": "toolu_1", "content": "hello" }
            ] }
        })
        .to_string()
    }

    pub(crate) fn text(text: &str) -> String {
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": text }] } })
            .to_string()
    }

    pub(crate) fn result(text: &str) -> String {
        json!({ "type": "result", "subtype": "success", "is_error": false, "result": text })
            .to_string()
    }

    /// A clean R0 read run, as claude 2.1 prints it.
    pub(crate) fn read_run() -> Vec<String> {
        vec![
            init(&["Glob", "Grep", "Read"]),
            json!({ "type": "system", "subtype": "thinking_tokens", "tokens": 12 }).to_string(),
            tool_use("Read"),
            json!({ "type": "rate_limit_event", "rate_limit_info": {} }).to_string(),
            tool_result(),
            text("The first word is hello."),
            result("The first word is hello."),
        ]
    }

    /// A clean scoped read run, as claude 2.1.305 prints it under
    /// `r0-read.2`.
    #[cfg(unix)]
    pub(crate) fn scoped_read_run() -> Vec<String> {
        vec![
            json!({ "type": "system", "subtype": "ui_invalidate", "event": "ui.render" })
                .to_string(),
            json!({
                "type": "system",
                "subtype": "init",
                "tools": ["mcp__coven__read_file", "mcp__coven__list_directory", "mcp__coven__search"],
                "mcp_servers": [{ "name": "coven", "status": "connected", "source": "dynamic" }],
                "permissionMode": "default",
                "claude_code_version": "2.1.305",
                "apiKeySource": "ANTHROPIC_API_KEY",
                "session_id": "native-session"
            })
            .to_string(),
            tool_use("mcp__coven__read_file"),
            tool_result(),
            text("It says granted notes."),
            result("It says granted notes."),
        ]
    }

    /// [`scoped_read_run`], with `calls` read tool calls, each answered.
    #[cfg(unix)]
    pub(crate) fn scoped_read_run_calling(calls: usize) -> Vec<String> {
        let mut lines = scoped_read_run();
        let answered = [tool_use("mcp__coven__read_file"), tool_result()];
        lines.splice(2..4, answered.iter().cycle().take(calls * 2).cloned());
        lines
    }

    pub(crate) fn stream(lines: &[String]) -> String {
        let mut output = lines.join("\n");
        output.push('\n');
        output
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::automations::runtime_envelope::CLAUDE_R0_READ;
    use serde_json::json;

    fn classify(lines: &[String]) -> StreamObservation {
        classify_claude_stream(&CLAUDE_R0_READ, &stream(lines), false)
    }

    #[test]
    fn a_clean_read_run_is_complete_and_names_what_it_read() {
        let observation = classify(&read_run());
        assert!(observation.complete);
        assert_eq!(
            observation.capabilities.into_iter().collect::<Vec<_>>(),
            ["analysis.read"]
        );
        assert_eq!(observation.side_effects, SideEffectClass::LocalRead);
        assert_eq!(
            observation.result,
            StreamResult::Produced("The first word is hello.".to_owned())
        );
        // Wrapped stderr is diagnostics, wherever it falls.
        let mut lines = read_run();
        let stderr =
            json!({ "type": "system", "subtype": "stderr", "text": "warning" }).to_string();
        lines.insert(3, stderr.clone());
        lines.push(stderr);
        assert!(classify(&lines).complete);
    }

    #[test]
    fn a_run_without_tools_exercised_nothing() {
        let observation = classify(&[init(&["Glob", "Grep", "Read"]), text("hi"), result("hi")]);
        assert!(observation.complete);
        assert!(observation.capabilities.is_empty());
        assert_eq!(observation.side_effects, SideEffectClass::None);
    }

    #[test]
    fn a_failed_result_is_complete_but_produced_nothing() {
        let mut lines = read_run();
        lines.pop();
        lines.push(
            json!({ "type": "result", "subtype": "error_during_execution", "is_error": true })
                .to_string(),
        );
        let observation = classify(&lines);
        assert!(observation.complete);
        assert_eq!(observation.result, StreamResult::NotProduced);
    }

    #[test]
    fn anything_the_stream_cannot_account_for_leaves_coverage_partial() {
        let edit = |change: &dyn Fn(&mut Vec<String>)| {
            let mut lines = read_run();
            change(&mut lines);
            lines
        };
        let cases: Vec<(&str, Vec<String>)> = vec![
            (
                "a tool outside the envelope",
                edit(&|l| l.insert(2, tool_use("Bash"))),
            ),
            (
                "an init listing a broader tool set",
                edit(&|l| l[0] = init(&["Bash", "Read"])),
            ),
            (
                "an init with an MCP server",
                edit(&|l| {
                    l[0] = json!({ "type": "system", "subtype": "init", "tools": ["Read"],
                        "mcp_servers": [{ "name": "x" }], "permissionMode": "plan" })
                    .to_string()
                }),
            ),
            (
                "an init in another permission mode",
                edit(&|l| {
                    l[0] = json!({ "type": "system", "subtype": "init", "tools": ["Read"],
                        "mcp_servers": [], "permissionMode": "default" })
                    .to_string()
                }),
            ),
            (
                "no init",
                edit(&|l| {
                    l.remove(0);
                }),
            ),
            ("two inits", edit(&|l| l.insert(1, init(&["Read"])))),
            (
                "no result",
                edit(&|l| {
                    l.pop();
                }),
            ),
            ("two results", edit(&|l| l.push(result("again")))),
            (
                "activity after the result",
                edit(&|l| l.push(tool_use("Read"))),
            ),
            (
                "diagnostics other than stderr after the result",
                edit(&|l| {
                    l.push(
                        json!({ "type": "system", "subtype": "thinking_tokens", "tokens": 12 })
                            .to_string(),
                    )
                }),
            ),
            (
                "a line that is not JSON",
                edit(&|l| l.insert(2, "Read note.txt".to_owned())),
            ),
            (
                "an unknown event",
                edit(&|l| l.insert(2, json!({ "type": "tool_progress" }).to_string())),
            ),
            (
                "an unknown system subtype",
                edit(&|l| {
                    l.insert(
                        2,
                        json!({ "type": "system", "subtype": "unrecognized_activity" }).to_string(),
                    )
                }),
            ),
            (
                "a system event without a subtype",
                edit(&|l| l.insert(2, json!({ "type": "system" }).to_string())),
            ),
            (
                "content that is not a list",
                edit(&|l| {
                    l.insert(
                        2,
                        json!({ "type": "assistant", "message": { "content": "Read" } })
                            .to_string(),
                    )
                }),
            ),
            (
                "an unknown content block",
                edit(&|l| {
                    l.insert(2, json!({ "type": "assistant", "message": { "content": [{ "type": "server_tool_use", "name": "web_search" }] } }).to_string())
                }),
            ),
        ];
        for (case, lines) in cases {
            assert!(!classify(&lines).complete, "{case}");
        }
        // A missing result leaves the result unobserved.
        assert_eq!(
            classify(&edit(&|l| {
                l.pop();
            }))
            .result,
            StreamResult::Unobserved
        );
        // A cut-off final line, and dropped or redacted output.
        let cut = stream(&read_run());
        assert!(!classify_claude_stream(&CLAUDE_R0_READ, &cut[..cut.len() - 1], false).complete);
        assert!(!classify_claude_stream(&CLAUDE_R0_READ, &cut, true).complete);
    }

    /// A named change to an init event.
    type InitEdit = (&'static str, fn(&mut Value));

    /// A scoped read run as claude 2.1.305 prints it under `r0-read.2`, with
    /// `init` edited by `edit`.
    fn scoped_run(edit: impl FnOnce(&mut Value)) -> Vec<String> {
        let mut init = json!({
            "type": "system",
            "subtype": "init",
            "tools": ["mcp__coven__read_file", "mcp__coven__list_directory", "mcp__coven__search"],
            "mcp_servers": [{ "name": "coven", "status": "connected", "source": "dynamic" }],
            "permissionMode": "default",
            "claude_code_version": "2.1.305",
            "apiKeySource": "ANTHROPIC_API_KEY",
            "session_id": "native-session"
        });
        edit(&mut init);
        vec![
            json!({ "type": "system", "subtype": "ui_invalidate", "event": "ui.render" })
                .to_string(),
            init.to_string(),
            tool_use("mcp__coven__read_file"),
            tool_result(),
            text("It says granted notes."),
            result("It says granted notes."),
        ]
    }

    #[test]
    fn a_scoped_read_run_is_complete_only_with_exactly_the_service() {
        use crate::automations::runtime_envelope::CLAUDE_R0_SCOPED_READ;

        let classify_scoped = |lines: &[String]| {
            classify_claude_stream(&CLAUDE_R0_SCOPED_READ, &stream(lines), false)
        };
        let observation = classify_scoped(&scoped_run(|_| {}));
        assert!(observation.complete);
        assert_eq!(
            observation.capabilities.into_iter().collect::<Vec<_>>(),
            ["analysis.read"]
        );
        assert_eq!(observation.side_effects, SideEffectClass::LocalRead);
        assert_eq!(
            observation.result,
            StreamResult::Produced("It says granted notes.".to_owned())
        );

        let partial: [InitEdit; 8] = [
            ("another harness version", |init| {
                init["claude_code_version"] = json!("2.1.306")
            }),
            ("no harness version", |init| {
                init.as_object_mut().unwrap().remove("claude_code_version");
            }),
            ("no MCP server", |init| init["mcp_servers"] = json!([])),
            ("the service not connected", |init| {
                init["mcp_servers"][0]["status"] = json!("failed")
            }),
            ("another MCP server", |init| {
                init["mcp_servers"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({ "name": "github", "status": "connected" }))
            }),
            ("a server under another name", |init| {
                init["mcp_servers"][0]["name"] = json!("coven-shadow")
            }),
            ("a native tool", |init| {
                init["tools"].as_array_mut().unwrap().push(json!("Read"))
            }),
            ("plan mode", |init| init["permissionMode"] = json!("plan")),
        ];
        for (case, edit) in partial {
            assert!(!classify_scoped(&scoped_run(edit)).complete, "{case}");
        }
        // A native read under the service's envelope is unclassified.
        let mut native = scoped_run(|_| {});
        native[2] = tool_use("Read");
        assert!(!classify_scoped(&native).complete);
        // The service under r0-read.1 is a widening, not a read.
        assert!(
            !classify(&scoped_run(|init| {
                init["permissionMode"] = json!("plan");
                init["tools"] = json!(["Read"]);
            }))
            .complete
        );
    }
}
