//! Harness-owned startup records for main-session continuity.
use crate::pty_runner::PipedOutputSource;
use serde_json::Value;
use uuid::Uuid;

pub(crate) fn supports_harness(harness: &str) -> bool {
    matches!(harness, "claude" | "codex")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupOutcome {
    Ready(String),
    Stale,
}

#[derive(Default)]
struct Lines {
    pending: Vec<u8>,
    oversized: bool,
}

impl Lines {
    fn feed(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        let mut records = Vec::new();
        for byte in chunk {
            if *byte == b'\n' {
                if !self.oversized {
                    records.push(std::mem::take(&mut self.pending));
                }
                self.oversized = false;
            } else if !self.oversized {
                if self.pending.len() == 128 * 1024 {
                    self.pending.clear();
                    self.oversized = true;
                } else {
                    self.pending.push(*byte);
                }
            }
        }
        records
    }
}

pub(crate) struct StartupParser {
    harness: String,
    stdout: Lines,
    stderr: Lines,
}

impl StartupParser {
    pub(crate) fn new(harness: &str) -> Self {
        Self {
            harness: harness.to_owned(),
            stdout: Lines::default(),
            stderr: Lines::default(),
        }
    }

    pub(crate) fn feed(
        &mut self,
        source: PipedOutputSource,
        chunk: &[u8],
    ) -> Option<StartupOutcome> {
        let lines = match source {
            PipedOutputSource::Stdout => self.stdout.feed(chunk),
            PipedOutputSource::Stderr => self.stderr.feed(chunk),
        };
        // The first terminal startup record is authoritative. In particular,
        // Codex echoes the user prompt after its successful session banner;
        // later text must not turn that accepted session into a stale resume.
        lines.iter().find_map(|line| self.record(source, line))
    }

    fn finish_input(&mut self) -> Option<StartupOutcome> {
        let stderr = std::mem::take(&mut self.stderr.pending);
        let stdout = std::mem::take(&mut self.stdout.pending);
        let mut ready = None;
        for (source, bytes, oversized) in [
            (PipedOutputSource::Stderr, stderr, self.stderr.oversized),
            (PipedOutputSource::Stdout, stdout, self.stdout.oversized),
        ] {
            if oversized {
                continue;
            }
            match self.record(source, &bytes) {
                Some(StartupOutcome::Stale) => return Some(StartupOutcome::Stale),
                Some(outcome) => ready = Some(outcome),
                None => {}
            }
        }
        ready
    }

    fn record(&self, source: PipedOutputSource, line: &[u8]) -> Option<StartupOutcome> {
        let native = match source {
            PipedOutputSource::Stderr => {
                let text = std::str::from_utf8(line).ok()?.trim();
                let stale = match self.harness.as_str() {
                    "claude" => text.contains("No conversation found with session ID"),
                    "codex" => {
                        text.contains("no rollout found for thread id")
                            || text.contains("thread/resume failed")
                    }
                    _ => false,
                };
                if stale {
                    return Some(StartupOutcome::Stale);
                }
                if self.harness != "codex" {
                    return None;
                }
                text.strip_prefix("session id:")?.trim().to_string()
            }
            PipedOutputSource::Stdout => {
                if self.harness != "claude" {
                    return None;
                }
                let event: Value = serde_json::from_slice(line).ok()?;
                if event["type"] != "system" || event["subtype"] != "init" {
                    return None;
                }
                event["session_id"].as_str()?.to_string()
            }
        };
        Uuid::parse_str(&native).ok()?;
        Some(StartupOutcome::Ready(native))
    }
}

pub(crate) struct StartupMonitor {
    deadline: std::time::Instant,
    parser: std::sync::Mutex<StartupParser>,
    sender: std::sync::Mutex<Option<std::sync::mpsc::Sender<Result<StartupOutcome, &'static str>>>>,
}

impl StartupMonitor {
    #[cfg(test)]
    pub(crate) fn new(
        harness: &str,
    ) -> (
        std::sync::Arc<Self>,
        std::sync::mpsc::Receiver<Result<StartupOutcome, &'static str>>,
    ) {
        Self::with_budget(harness, std::time::Duration::from_secs(60))
    }

    pub(crate) fn with_budget(
        harness: &str,
        budget: std::time::Duration,
    ) -> (
        std::sync::Arc<Self>,
        std::sync::mpsc::Receiver<Result<StartupOutcome, &'static str>>,
    ) {
        let (sender, receiver) = std::sync::mpsc::channel();
        (
            std::sync::Arc::new(Self {
                deadline: std::time::Instant::now() + budget,
                parser: std::sync::Mutex::new(StartupParser::new(harness)),
                sender: std::sync::Mutex::new(Some(sender)),
            }),
            receiver,
        )
    }

    pub(crate) fn remaining(&self) -> std::time::Duration {
        self.deadline
            .saturating_duration_since(std::time::Instant::now())
    }

    pub(crate) fn timed_out(&self) -> bool {
        self.remaining().is_zero()
    }

    fn finish(&self, outcome: Result<StartupOutcome, &'static str>) {
        if let Some(sender) = self
            .sender
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = sender.send(outcome);
        }
    }

    pub(crate) fn status_observer(
        self: &std::sync::Arc<Self>,
    ) -> crate::pty_runner::PipedStatusObserver {
        let monitor = std::sync::Arc::clone(self);
        Box::new(move |source, bytes| {
            if monitor
                .sender
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_none()
            {
                return;
            }
            let outcome = monitor
                .parser
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .feed(source, bytes);
            if let Some(outcome) = outcome {
                monitor.finish(Ok(outcome));
            }
        })
    }

    pub(crate) fn observe_exit(
        self: &std::sync::Arc<Self>,
        observer: crate::pty_runner::DetachedPtyObserver,
    ) -> crate::pty_runner::DetachedPtyObserver {
        let monitor = std::sync::Arc::clone(self);
        crate::pty_runner::DetachedPtyObserver {
            on_output: observer.on_output,
            on_exit: Box::new(move |result| {
                let final_record = monitor
                    .parser
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .finish_input();
                (observer.on_exit)(result);
                if let Some(outcome) = final_record {
                    monitor.finish(Ok(outcome));
                }
                monitor.finish(Err(
                    "harness exited before reporting its conversation identity",
                ));
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "b5c84cac-58cd-44c5-991b-516ff2635ed9";

    #[test]
    fn codex_prompt_echo_cannot_override_ready_across_chunk_boundaries() {
        // Codex prints its authoritative session banner before echoing the prompt.
        let output = format!("session id: {ID}\nuser\nExplain thread/resume failed\n");
        for split in 0..=output.len() {
            let (monitor, receiver) = StartupMonitor::new("codex");
            let mut observe = monitor.status_observer();
            observe(PipedOutputSource::Stderr, &output.as_bytes()[..split]);
            observe(PipedOutputSource::Stderr, &output.as_bytes()[split..]);
            assert_eq!(
                receiver.try_recv().unwrap(),
                Ok(StartupOutcome::Ready(ID.to_string())),
                "split at {split}"
            );
        }
    }

    #[test]
    fn captures_native_id_across_every_record_split() {
        for (harness, source, record) in [
            (
                "claude",
                PipedOutputSource::Stdout,
                serde_json::json!({"type":"system","subtype":"init","session_id":ID}).to_string(),
            ),
            (
                "codex",
                PipedOutputSource::Stderr,
                format!("session id: {ID}"),
            ),
        ] {
            let line = format!("{record}\n");
            for split in 0..line.len() {
                let mut parser = StartupParser::new(harness);
                assert_eq!(parser.feed(source, &line.as_bytes()[..split]), None);
                assert_eq!(
                    parser.feed(source, &line.as_bytes()[split..]),
                    Some(StartupOutcome::Ready(ID.to_string()))
                );
            }
        }
    }

    #[test]
    fn assistant_text_and_malformed_ids_cannot_set_identity_or_request_rollover() {
        for harness in ["claude", "codex"] {
            let mut parser = StartupParser::new(harness);
            for line in [
                format!("session id: {ID}\n"),
                format!("{{\"type\":\"assistant\",\"text\":\"session id: {ID}; thread/resume failed; No conversation found with session ID\"}}\n"),
                "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"not-a-uuid\"}\n".to_string(),
                "{\"type\":\"system\",\"subtype\":\"stderr\",\"text\":\"session id: not-a-uuid\"}\n".to_string(),
            ] {
                assert_eq!(parser.feed(PipedOutputSource::Stdout, line.as_bytes()), None, "{harness}: {line}");
            }
        }
    }

    #[test]
    fn stale_detection_only_reads_harness_status_records() {
        for (harness, text) in [
            ("claude", "No conversation found with session ID: old"),
            ("codex", "no rollout found for thread id old"),
            ("codex", "thread/resume failed: missing"),
        ] {
            let mut parser = StartupParser::new(harness);
            let line = format!("{text}\n");
            assert_eq!(
                parser.feed(PipedOutputSource::Stderr, line.as_bytes()),
                Some(StartupOutcome::Stale)
            );
        }
    }

    #[test]
    fn startup_monitor_recognizes_final_stale_record_without_newline() {
        let (monitor, receiver) = StartupMonitor::new("codex");
        let mut status = monitor.status_observer();
        status(
            PipedOutputSource::Stderr,
            b"no rollout found for thread id old",
        );
        let observer = monitor.observe_exit(crate::pty_runner::DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(|_| {}),
        });
        (observer.on_exit)(crate::pty_runner::PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        });
        assert_eq!(receiver.try_recv().unwrap(), Ok(StartupOutcome::Stale));
    }

    #[test]
    fn interleaved_sources_do_not_corrupt_partial_startup_records() {
        let mut parser = StartupParser::new("claude");
        let line = format!(
            "{}\n",
            serde_json::json!({"type":"system","subtype":"init","session_id":ID})
        );
        let split = line.len() / 2;
        assert_eq!(
            parser.feed(PipedOutputSource::Stdout, &line.as_bytes()[..split]),
            None
        );
        assert_eq!(
            parser.feed(PipedOutputSource::Stderr, b"loading settings\n"),
            None
        );
        assert_eq!(
            parser.feed(PipedOutputSource::Stdout, &line.as_bytes()[split..]),
            Some(StartupOutcome::Ready(ID.to_string()))
        );
        let mut codex = StartupParser::new("codex");
        assert_eq!(
            codex.feed(PipedOutputSource::Stderr, b"session id: malformed\n"),
            None
        );
        let spoof = format!(
            "{}\n",
            serde_json::json!({"type":"system","subtype":"stderr","text":format!("session id: {ID}")})
        );
        assert_eq!(
            codex.feed(PipedOutputSource::Stdout, spoof.as_bytes()),
            None
        );
    }

    #[test]
    fn oversized_record_is_discarded_and_next_record_is_parsed() {
        let mut parser = StartupParser::new("claude");
        assert_eq!(
            parser.feed(PipedOutputSource::Stdout, &vec![b'x'; 256 * 1024]),
            None
        );
        assert!(parser.stdout.pending.len() <= 128 * 1024);
        let line = format!(
            "\n{}\n",
            serde_json::json!({"type":"system","subtype":"init","session_id":ID})
        );
        assert_eq!(
            parser.feed(PipedOutputSource::Stdout, line.as_bytes()),
            Some(StartupOutcome::Ready(ID.to_string()))
        );
    }
}
