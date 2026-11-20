//! Account for query-bearing JSONL input without confusing transcript prefill
//! or ignored control frames with model turns. Unknown input fails closed.

use serde_json::Value;

const MAX_FRAME_BYTES: usize = 24 * 1024 * 1024;

#[derive(Default)]
pub struct Progress {
    line: Vec<u8>,
    oversized: bool,
    uncertain: bool,
    queries: usize,
    unsaved_prefill: bool,
}

impl Progress {
    /// Observe before delivery so a fast result cannot outrun its input fence.
    pub fn observe(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                if self.line.len() == MAX_FRAME_BYTES {
                    self.oversized = true;
                }
                self.finish_line();
            } else if self.line.len() < MAX_FRAME_BYTES {
                self.line.push(byte);
            } else {
                self.oversized = true;
            }
        }
    }

    pub fn eof(&mut self) {
        self.finish_line();
    }

    pub fn saved_queries_cover_input(&self, completed: usize) -> bool {
        !self.uncertain
            && !self.oversized
            && !self.unsaved_prefill
            && self.line.iter().all(u8::is_ascii_whitespace)
            && self.queries == completed
    }

    fn finish_line(&mut self) {
        if self.oversized {
            self.uncertain = true;
        } else if self.line.iter().any(|byte| !byte.is_ascii_whitespace()) {
            let kind = serde_json::from_slice::<Value>(&self.line)
                .ok()
                .and_then(|frame| classify(&frame));
            match kind {
                Some(Frame::Query) => {
                    self.queries += 1;
                    // A subsequent saved query persists the preceding prefill.
                    self.unsaved_prefill = false;
                }
                Some(Frame::Prefill) => self.unsaved_prefill = true,
                Some(Frame::Ignored) => {}
                None => self.uncertain = true,
            }
        }
        self.line.clear();
        self.oversized = false;
    }
}

enum Frame {
    Query,
    Prefill,
    Ignored,
}

fn classify(frame: &Value) -> Option<Frame> {
    let (role, content) = if let Some(kind) = frame.get("type").and_then(Value::as_str) {
        if !matches!(kind, "user" | "assistant") {
            return Some(Frame::Ignored);
        }
        let message = frame.get("message")?;
        let role = match message.get("role") {
            Some(role) => role.as_str()?,
            None => kind,
        };
        if role != kind {
            return None;
        }
        (role, message.get("content"))
    } else {
        let role = match frame.get("role") {
            Some(role) => role.as_str()?,
            None => "user",
        };
        (role, frame.get("content"))
    };
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let nonempty = match content {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => {
            if role == "assistant" {
                !text.is_empty()
            } else {
                !text.trim().is_empty()
            }
        }
        Some(Value::Array(blocks)) => {
            // The producer joins text-only blocks with newlines. Two empty
            // assistant blocks therefore append a real (whitespace) prefill.
            let mut nonempty = role == "assistant" && blocks.len() > 1;
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block.get("text")?.as_str()?;
                        nonempty |= if role == "assistant" {
                            !text.is_empty()
                        } else {
                            !text.trim().is_empty()
                        };
                    }
                    // The engine validates image bytes. Invalid images cannot
                    // produce a saved query receipt, so counting one is safe.
                    Some("image") if role == "user" => nonempty = true,
                    _ => return None,
                }
            }
            nonempty
        }
        _ => return None,
    };
    Some(if !nonempty {
        Frame::Ignored
    } else if role == "assistant" {
        Frame::Prefill
    } else {
        Frame::Query
    })
}

#[cfg(test)]
mod tests {
    use super::{Progress, MAX_FRAME_BYTES};

    #[test]
    fn physical_line_limit_counts_newline_but_not_eof() {
        for (content_bytes, terminator, accepted) in [
            (MAX_FRAME_BYTES - 1, b"\n".as_slice(), true),
            (MAX_FRAME_BYTES, b"\n".as_slice(), false),
            (MAX_FRAME_BYTES, b"".as_slice(), true),
        ] {
            let mut progress = Progress::default();
            progress.observe(&vec![b' '; content_bytes]);
            progress.observe(terminator);
            progress.eof();

            assert_eq!(
                progress.saved_queries_cover_input(0),
                accepted,
                "content_bytes={content_bytes}, newline={}",
                !terminator.is_empty()
            );
        }
    }
}
