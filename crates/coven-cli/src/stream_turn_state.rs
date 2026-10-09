//! Tracks submitted stream turns until their terminal result records arrive.
//! A writable process is not necessarily idle. Unknown or oversized output
//! never establishes idleness.

#[derive(Default)]
pub(crate) struct StreamTurnState {
    pending: usize,
    line: Vec<u8>,
    discard_line: bool,
}

impl StreamTurnState {
    pub(crate) fn begin(&mut self) {
        self.pending = self.pending.saturating_add(1);
    }

    pub(crate) fn idle(&self) -> bool {
        self.pending == 0
    }

    pub(crate) fn observe(&mut self, bytes: &[u8]) {
        const MAX_LINE: usize = 1024 * 1024;
        for &byte in bytes {
            if byte == b'\n' {
                if !self.discard_line {
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&self.line) {
                        if value.get("type").and_then(|v| v.as_str()) == Some("result") {
                            self.pending = self.pending.saturating_sub(1);
                        }
                    }
                }
                self.line.clear();
                self.discard_line = false;
            } else if !self.discard_line {
                if self.line.len() == MAX_LINE {
                    self.line.clear();
                    self.discard_line = true;
                } else {
                    self.line.push(byte);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_result_records_finish_submitted_turns() {
        let mut state = StreamTurnState::default();
        state.begin();
        state.begin();
        state.observe(b"{\"type\":\"assistant\"}\n{\"type\":\"res");
        assert!(!state.idle());
        state.observe(b"ult\",\"is_error\":false}\n");
        assert!(!state.idle(), "one queued turn still has no result");
        state.observe(b"{\"type\":\"result\",\"is_error\":true}\n");
        assert!(state.idle(), "failed turns finish too");
    }

    #[test]
    fn stderr_and_malformed_output_do_not_establish_idle() {
        let mut state = StreamTurnState::default();
        state.begin();
        state.observe(b"not json\n{\"type\":\"stderr\",\"text\":\"result\"}\n");
        assert!(!state.idle());
    }

    #[test]
    fn oversized_lines_are_discarded_through_the_next_newline() {
        let mut state = StreamTurnState::default();
        state.begin();
        state.observe(&vec![b' '; 1024 * 1024 + 1]);
        state.observe(b"{\"type\":\"result\"}\n");
        assert!(!state.idle());
        state.observe(b"{\"type\":\"result\"}\n");
        assert!(state.idle());
    }
}
