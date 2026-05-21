//! Persistent chat-TUI settings. The on-disk form is a small JSON document at
//! `<coven_home>/chat-settings.json` so non-Coven tooling can inspect or edit
//! it without learning a custom format. Today the only setting is the
//! streaming mode toggle; future entries should land in the same file rather
//! than introducing a parallel store.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Whether agent responses should appear chunk-by-chunk as they arrive
/// (`Live`) or be held back until the session reports completion (`Batched`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StreamingMode {
    #[default]
    Live,
    Batched,
}

impl StreamingMode {
    pub(super) fn status_label(self) -> &'static str {
        match self {
            StreamingMode::Live => "live",
            StreamingMode::Batched => "off",
        }
    }

    pub(super) fn is_live(self) -> bool {
        matches!(self, StreamingMode::Live)
    }

    pub(super) fn toggled(self) -> Self {
        match self {
            StreamingMode::Live => StreamingMode::Batched,
            StreamingMode::Batched => StreamingMode::Live,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ChatSettings {
    #[serde(default)]
    pub(super) streaming: StreamingMode,
}

pub(super) fn settings_path(coven_home: &Path) -> PathBuf {
    coven_home.join("chat-settings.json")
}

/// Load settings from disk, falling back to defaults on any error. The TUI
/// uses this on startup, so an unreadable or partially-written file must not
/// stop the chat from coming up — it just resets to defaults silently.
pub(super) fn load_from(coven_home: &Path) -> ChatSettings {
    let path = settings_path(coven_home);
    let Ok(data) = std::fs::read(&path) else {
        return ChatSettings::default();
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

/// Persist settings to disk, creating `<coven_home>` if missing. Returns the
/// underlying io error so callers can surface a system message — but they
/// must not treat it as a fatal failure (the in-memory mode is still active).
pub(super) fn save_to(coven_home: &Path, settings: &ChatSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(coven_home)?;
    let body = serde_json::to_vec_pretty(settings)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    std::fs::write(settings_path(coven_home), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_live_streaming() {
        let settings = ChatSettings::default();
        assert_eq!(settings.streaming, StreamingMode::Live);
        assert!(settings.streaming.is_live());
        assert_eq!(settings.streaming.status_label(), "live");
    }

    #[test]
    fn toggled_streaming_mode_round_trips() {
        assert_eq!(StreamingMode::Live.toggled(), StreamingMode::Batched);
        assert_eq!(StreamingMode::Batched.toggled(), StreamingMode::Live);
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_from(temp.path());
        assert_eq!(loaded, ChatSettings::default());
    }

    #[test]
    fn save_then_load_preserves_streaming_choice() {
        let temp = tempfile::tempdir().unwrap();
        let settings = ChatSettings {
            streaming: StreamingMode::Batched,
        };
        save_to(temp.path(), &settings).expect("save settings");
        let reloaded = load_from(temp.path());
        assert_eq!(reloaded, settings);
    }

    #[test]
    fn corrupt_settings_file_falls_back_to_defaults() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(settings_path(temp.path()), b"not json").unwrap();
        assert_eq!(load_from(temp.path()), ChatSettings::default());
    }
}
