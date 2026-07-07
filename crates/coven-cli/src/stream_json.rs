// Types and helpers land ahead of their call sites (Task 4.1 of the P0 parity
// plan); Tasks 4.2–4.4 wire them into the harness adapters. The allow can be
// removed once those land.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

/// One JSONL frame on the Coven stream-JSON I/O protocol.
///
/// The discriminator key is `type` and the variant names round-trip as
/// `system`, `user`, `assistant`, `tool_result`, `result`. The shape is
/// intentionally aligned with `@opencoven/coven-code` (which itself mirrors
/// Anthropic's tool-use schema) so external SDKs can drop in without
/// changing their parser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    System(System),
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResult),
    Output(HarnessOutput),
    Result(RunResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct System {
    pub subtype: String,
    pub cwd: String,
    pub session_id: String,
    pub tools: Vec<String>,
    pub agent_mode: Option<String>,
    /// The model the session was launched on, echoed back so clients (Cave) can
    /// confirm acceptance and render `applied` vs `pending`. Carries the id
    /// exactly as requested on `coven run --model` (namespaced form preserved,
    /// e.g. `anthropic/claude-…`); `None` when no `--model` was passed.
    pub model: Option<String>,
    /// The sandbox/permission policy the session was launched with, echoed back
    /// so clients (Cave) can confirm the Access chip was enforced. Canonical
    /// form (`full` / `read-only`); `None` when no `--permission` was passed.
    pub permission: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserMessage {
    pub message: MessageBody,
    pub session_id: String,
    pub parent_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistantMessage {
    pub message: MessageBody,
    pub session_id: String,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageBody {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Path { path: String, media_type: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    pub session_id: String,
}

/// Raw output captured from a harness that has no native stream-json
/// protocol (codex, external adapters). In `--stream-json` mode those
/// harnesses run on a PTY; every captured chunk is wrapped in one `output`
/// frame so stdout stays JSONL-only instead of interleaving raw PTY bytes
/// with the stream (#307). `text` is the raw PTY text: it may contain ANSI
/// escape sequences, carriage returns, and partial lines, and chunk
/// boundaries follow PTY reads rather than line breaks. Each chunk is
/// guaranteed valid UTF-8 (codepoints split across reads are reassembled
/// before wrapping).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HarnessOutput {
    pub text: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunResult {
    pub subtype: String,
    pub duration_ms: u64,
    pub is_error: bool,
    pub num_turns: u32,
    pub session_id: String,
    pub error: Option<String>,
}

/// Emit one event as a JSONL frame followed by a single `\n`. Flushes the
/// writer so consumers see the frame immediately (the protocol is intended
/// for streaming pipelines).
pub fn emit_event<W: Write>(writer: &mut W, event: &Event) -> Result<()> {
    serde_json::to_writer(&mut *writer, event)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// Read the next JSONL frame from `reader`. Returns `Ok(None)` on EOF or on
/// a blank line (treated as a frame separator). Any non-empty line that
/// fails to parse is surfaced as an error.
pub fn read_event<R: BufRead>(reader: &mut R) -> Result<Option<Event>> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(trimmed)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_user_message() {
        let event = Event::User(UserMessage {
            message: MessageBody {
                role: "user".into(),
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            },
            session_id: "s1".into(),
            parent_tool_use_id: None,
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let mut reader = Cursor::new(buf);
        let decoded = read_event(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn json_shape_matches_coven_code() {
        let event = Event::Result(RunResult {
            subtype: "success".into(),
            duration_ms: 12,
            is_error: false,
            num_turns: 1,
            session_id: "s1".into(),
            error: None,
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let line = String::from_utf8(buf).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "result");
        assert_eq!(v["subtype"], "success");
        assert_eq!(v["is_error"], false);
        assert_eq!(v["num_turns"], 1);
        assert_eq!(v["session_id"], "s1");
        assert!(v.get("error").is_some(), "null error field is preserved");
    }

    #[test]
    fn system_event_wire_shape() {
        let event = Event::System(System {
            subtype: "init".into(),
            cwd: "/Users/example/project".into(),
            session_id: "s1".into(),
            tools: vec!["bash".into(), "read_file".into()],
            agent_mode: Some("plan".into()),
            model: Some("anthropic/claude-sonnet-4".into()),
            permission: Some("read-only".into()),
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let line = String::from_utf8(buf).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "system");
        assert_eq!(v["subtype"], "init");
        assert_eq!(v["cwd"], "/Users/example/project");
        assert_eq!(v["tools"][0], "bash");
        assert_eq!(v["agent_mode"], "plan");
        assert_eq!(v["model"], "anthropic/claude-sonnet-4");
        assert_eq!(v["permission"], "read-only");
    }

    #[test]
    fn assistant_with_tool_use_round_trips() {
        let event = Event::Assistant(AssistantMessage {
            message: MessageBody {
                role: "assistant".into(),
                content: vec![
                    ContentBlock::Text {
                        text: "I'll read the file.".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_01".into(),
                        name: "read_file".into(),
                        input: serde_json::json!({"path": "README.md"}),
                    },
                ],
            },
            session_id: "s1".into(),
            stop_reason: Some("tool_use".into()),
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let mut reader = Cursor::new(buf);
        let decoded = read_event(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn tool_result_round_trips() {
        let event = Event::ToolResult(ToolResult {
            tool_use_id: "toolu_01".into(),
            content: vec![ContentBlock::Text {
                text: "file contents here".into(),
            }],
            is_error: false,
            session_id: "s1".into(),
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let mut reader = Cursor::new(buf);
        let decoded = read_event(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn image_content_block_path_variant() {
        let event = Event::User(UserMessage {
            message: MessageBody {
                role: "user".into(),
                content: vec![ContentBlock::Image {
                    source: ImageSource::Path {
                        path: "/abs/path/pic.png".into(),
                        media_type: "image/png".into(),
                    },
                }],
            },
            session_id: "s1".into(),
            parent_tool_use_id: None,
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let mut reader = Cursor::new(buf);
        let decoded = read_event(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn output_event_wire_shape_and_round_trip() {
        // Raw PTY text keeps ANSI escapes / carriage returns / partial JSON
        // verbatim inside the JSON string; the frame itself stays one valid
        // JSONL line (#307).
        let event = Event::Output(HarnessOutput {
            text: "\u{1b}[1;32mbanner\u{1b}[0m\r\n{\"not\":\"terminated".into(),
            session_id: "s1".into(),
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let line = String::from_utf8(buf.clone()).unwrap();
        assert_eq!(
            line.matches('\n').count(),
            1,
            "one frame must serialize to exactly one line: {line:?}"
        );
        let v: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["type"], "output");
        assert_eq!(
            v["text"],
            "\u{1b}[1;32mbanner\u{1b}[0m\r\n{\"not\":\"terminated"
        );
        assert_eq!(v["session_id"], "s1");
        let mut reader = Cursor::new(buf);
        let decoded = read_event(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn read_event_returns_none_on_eof() {
        let buf: Vec<u8> = Vec::new();
        let mut reader = Cursor::new(buf);
        let got = read_event(&mut reader).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn read_event_returns_none_on_blank_line() {
        let mut reader = Cursor::new(b"\n".to_vec());
        let got = read_event(&mut reader).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn read_event_surfaces_parse_errors() {
        let mut reader = Cursor::new(b"{not-json\n".to_vec());
        let got = read_event(&mut reader);
        assert!(got.is_err(), "non-empty malformed line should error");
    }

    #[test]
    fn multiple_events_stream_in_order() {
        let mut buf = Vec::new();
        emit_event(
            &mut buf,
            &Event::System(System {
                subtype: "init".into(),
                cwd: "/x".into(),
                session_id: "s1".into(),
                tools: vec![],
                agent_mode: None,
                model: None,
                permission: None,
            }),
        )
        .unwrap();
        emit_event(
            &mut buf,
            &Event::Result(RunResult {
                subtype: "success".into(),
                duration_ms: 1,
                is_error: false,
                num_turns: 0,
                session_id: "s1".into(),
                error: None,
            }),
        )
        .unwrap();
        let mut reader = Cursor::new(buf);
        let first = read_event(&mut reader).unwrap().unwrap();
        let second = read_event(&mut reader).unwrap().unwrap();
        assert!(matches!(first, Event::System(_)));
        assert!(matches!(second, Event::Result(_)));
        assert!(read_event(&mut reader).unwrap().is_none());
    }
}
