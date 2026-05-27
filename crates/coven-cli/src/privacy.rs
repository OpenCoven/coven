use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyConfig {
    pub persist_raw_artifacts: bool,
    pub raw_artifact_retention_days: u64,
    pub log_retention_days: u64,
    pub extra_patterns: Vec<String>,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            persist_raw_artifacts: false,
            raw_artifact_retention_days: 7,
            log_retention_days: 30,
            extra_patterns: Vec::new(),
        }
    }
}

pub fn load_config(coven_home: &Path) -> Result<PrivacyConfig> {
    let mut config = PrivacyConfig::default();
    let path = coven_home.join("privacy.toml");
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let file_config: PrivacyConfigFile = toml::from_str(&raw)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            if let Some(value) = file_config.persist_raw_artifacts {
                config.persist_raw_artifacts = value;
            }
            if let Some(value) = file_config.raw_artifact_retention_days {
                config.raw_artifact_retention_days = value;
            }
            if let Some(value) = file_config.log_retention_days {
                config.log_retention_days = value;
            }
            if let Some(value) = file_config.extra_patterns {
                config.extra_patterns = value;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        }
    }

    if let Some(value) = std::env::var_os("COVEN_PERSIST_RAW_ARTIFACTS") {
        config.persist_raw_artifacts = env_truthy(&value.to_string_lossy());
    }
    if let Some(value) = env_u64("COVEN_RAW_ARTIFACT_RETENTION_DAYS") {
        config.raw_artifact_retention_days = value;
    }
    if let Some(value) = env_u64("COVEN_LOG_RETENTION_DAYS") {
        config.log_retention_days = value;
    }

    Ok(config)
}

pub fn load_with_settings(
    coven_home: &Path,
    settings: Option<&crate::settings::Settings>,
) -> Result<PrivacyConfig> {
    let mut config = load_config(coven_home)?;
    if let Some(settings) = settings.and_then(|settings| settings.coven_cli.privacy.as_ref()) {
        if let Some(value) = settings.persist_raw_artifacts {
            config.persist_raw_artifacts = value;
        }
        if let Some(value) = settings.raw_artifact_retention_days {
            config.raw_artifact_retention_days = value;
        }
        if let Some(value) = settings.log_retention_days {
            config.log_retention_days = value;
        }
        if let Some(value) = &settings.extra_patterns {
            config.extra_patterns = value.clone();
        }
    }

    if let Some(value) = std::env::var_os("COVEN_PERSIST_RAW_ARTIFACTS") {
        config.persist_raw_artifacts = env_truthy(&value.to_string_lossy());
    }
    if let Some(value) = env_u64("COVEN_RAW_ARTIFACT_RETENTION_DAYS") {
        config.raw_artifact_retention_days = value;
    }
    if let Some(value) = env_u64("COVEN_LOG_RETENTION_DAYS") {
        config.log_retention_days = value;
    }

    Ok(config)
}

pub fn redact_text(text: &str) -> String {
    redact_text_with_config(text, &PrivacyConfig::default())
}

pub fn redact_text_with_config(text: &str, config: &PrivacyConfig) -> String {
    let mut out = text.to_string();
    for regex in built_in_patterns() {
        out = regex.replace_all(&out, REDACTED).into_owned();
    }
    for pattern in &config.extra_patterns {
        if let Ok(regex) = Regex::new(pattern) {
            out = regex.replace_all(&out, REDACTED).into_owned();
        }
    }
    out
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn redact_value(value: &Value) -> Value {
    redact_value_with_config(value, &PrivacyConfig::default())
}

pub fn redact_payload_json(payload_json: &str) -> String {
    redact_payload_json_with_config(payload_json, &PrivacyConfig::default())
}

pub fn redact_payload_json_with_config(payload_json: &str, config: &PrivacyConfig) -> String {
    match serde_json::from_str::<Value>(payload_json) {
        Ok(value) => redact_value_with_config(&value, config).to_string(),
        Err(_) => redact_text_with_config(payload_json, config),
    }
}

pub fn payload_preview(payload: &Value, max_chars: usize) -> String {
    let text = payload
        .get("text")
        .or_else(|| payload.get("message"))
        .or_else(|| payload.get("data"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| payload.to_string());
    truncate_chars(&redact_text(&text), max_chars)
}

#[derive(Debug, Deserialize)]
struct PrivacyConfigFile {
    persist_raw_artifacts: Option<bool>,
    raw_artifact_retention_days: Option<u64>,
    log_retention_days: Option<u64>,
    extra_patterns: Option<Vec<String>>,
}

fn env_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
}

fn redact_value_with_config(value: &Value, config: &PrivacyConfig) -> Value {
    match value {
        Value::String(text) => Value::String(redact_text_with_config(text, config)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| redact_value_with_config(value, config))
                .collect(),
        ),
        Value::Object(map) => {
            let mut redacted = Map::new();
            for (key, value) in map {
                let value = if secretish_name(key) {
                    Value::String(REDACTED.to_string())
                } else {
                    redact_value_with_config(value, config)
                };
                redacted.insert(key.clone(), value);
            }
            Value::Object(redacted)
        }
        other => other.clone(),
    }
}

pub fn secretish_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase().replace('-', "_");
    normalized == "authorization"
        || normalized == "cookie"
        || normalized == "set_cookie"
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.contains("access_token")
        || normalized.contains("refresh_token")
        || normalized.contains("auth_token")
        || normalized.contains("github_token")
        || normalized.contains("openai_api_key")
        || normalized.contains("anthropic_api_key")
        || normalized.contains("private_key")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.ends_with("_token")
        || normalized == "token"
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push_str("...");
    truncated
}

fn built_in_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            [
                r"(?is)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
                r"(?im)\bAuthorization\s*:\s*(?:Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+",
                r"(?im)\b(?:Cookie|Set-Cookie)\s*:\s*[^\r\n]+",
                r#"(?i)\b(?:OPENAI_API_KEY|ANTHROPIC_API_KEY|GITHUB_TOKEN|GH_TOKEN|API[_-]?KEY|ACCESS[_-]?TOKEN|REFRESH[_-]?TOKEN|AUTH[_-]?TOKEN|SECRET|PASSWORD|PRIVATE[_-]?KEY)\s*=\s*["']?[^"'\s]+"#,
                r#"(?i)\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|secret|password|private[_-]?key)\s*[:=]\s*["']?[^"',\s}]+"#,
                r"\bsk-ant-[A-Za-z0-9_-]{20,}\b",
                r"\bsk-[A-Za-z0-9]{20,}\b",
                r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b",
                r"\bgithub_pat_[A-Za-z0-9_]{20,}\b",
                r#"(?i)(?:https?://)[^\s"']*(?:gateway|private|internal)[^\s"']*"#,
                r#"(?i)([?&](?:token|key|secret|api_key|access_token)=)[^&\s"']+"#,
            ]
            .into_iter()
            .map(|pattern| Regex::new(pattern).expect("valid privacy redaction regex"))
            .collect()
        })
        .as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    #[test]
    fn redact_text_removes_common_secret_shapes() {
        let fake_openai = fake_openai_key();
        let fake_github = fake_github_token();
        let fake_anthropic = fake_anthropic_key();
        let fake_gateway = fake_gateway_url();
        let fake_env = format!("OPENAI_API_KEY={fake_openai}");
        let fake_private_key_body = "fake".repeat(7);
        let fake_private_key = format!(
            "{}\n{}\n{}",
            private_key_marker("BEGIN"),
            fake_private_key_body,
            private_key_marker("END")
        );
        let input = format!(
            "Authorization: Bearer {fake_openai}\nAuthorization: Basic dXNlcjpwYXNz\nCookie: session={fake_github}\n{fake_env}\nANTHROPIC_API_KEY={fake_anthropic}\nkey={fake_github}\nurl={fake_gateway}\n{fake_private_key}"
        );

        let redacted = redact_text(&input);

        for secret in [
            &fake_openai,
            &fake_github,
            &fake_anthropic,
            "dXNlcjpwYXNz",
            "fakegatewaytoken123",
            &fake_private_key_body,
        ] {
            assert!(
                !redacted.contains(secret),
                "fake secret survived redaction: {secret}"
            );
        }
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn redact_value_removes_secretish_object_fields_and_string_contents() {
        let fake_openai = fake_openai_key();
        let fake_github = fake_github_token();
        let fake_anthropic = fake_anthropic_key();
        let value = serde_json::json!({
            "message": format!("call me with bearer {fake_openai}"),
            "headers": {
                "authorization": format!("Bearer {fake_github}"),
                "cookie": format!("session={fake_anthropic}")
            },
            "nested": [{ "OPENAI_API_KEY": fake_openai }]
        });

        let redacted = redact_value(&value);
        let body = redacted.to_string();

        for secret in [&fake_openai, &fake_github, &fake_anthropic] {
            assert!(
                !body.contains(secret),
                "fake secret survived JSON redaction"
            );
        }
        assert!(body.contains("[REDACTED]"));
    }

    #[test]
    fn payload_preview_redacts_and_bounds_text_fields() {
        let fake_openai = fake_openai_key();
        let payload = serde_json::json!({
            "data": format!("{} {}", "x".repeat(300), fake_openai),
        });

        let preview = payload_preview(&payload, 80);

        assert!(!preview.contains(&fake_openai));
        assert!(preview.chars().count() <= 83);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn load_config_reads_file_and_env_overrides() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("privacy.toml"),
            r#"
persist_raw_artifacts = false
raw_artifact_retention_days = 9
log_retention_days = 41
extra_patterns = ["custom-sensitive-[0-9]+"]
"#,
        )?;

        let _env_lock = ENV_LOCK.lock().expect("privacy env lock poisoned");
        let _env_guard = EnvVarGuard::set("COVEN_PERSIST_RAW_ARTIFACTS", "1");
        let config = load_config(temp.path())?;

        assert!(config.persist_raw_artifacts);
        assert_eq!(config.raw_artifact_retention_days, 9);
        assert_eq!(config.log_retention_days, 41);
        assert_eq!(config.extra_patterns, vec!["custom-sensitive-[0-9]+"]);
        assert_eq!(
            redact_text_with_config("custom-sensitive-1234", &config),
            "[REDACTED]"
        );
        Ok(())
    }

    #[test]
    fn settings_override_toml_but_env_still_wins() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("privacy.toml"),
            r#"
persist_raw_artifacts = false
raw_artifact_retention_days = 9
log_retention_days = 41
extra_patterns = ["toml-secret"]
"#,
        )?;
        let settings = crate::settings::Settings {
            coven_cli: crate::settings::CovenCliSettings {
                privacy: Some(crate::settings::PrivacySettings {
                    persist_raw_artifacts: Some(false),
                    raw_artifact_retention_days: Some(3),
                    log_retention_days: Some(4),
                    extra_patterns: Some(vec!["jsonc-secret".to_string()]),
                }),
                ..Default::default()
            },
        };

        let _env_lock = ENV_LOCK.lock().expect("privacy env lock poisoned");
        let _env_guard = EnvVarGuard::set("COVEN_PERSIST_RAW_ARTIFACTS", "1");
        let loaded = load_with_settings(temp.path(), Some(&settings))?;

        assert!(loaded.persist_raw_artifacts);
        assert_eq!(loaded.raw_artifact_retention_days, 3);
        assert_eq!(loaded.log_retention_days, 4);
        assert_eq!(loaded.extra_patterns, vec!["jsonc-secret"]);
        Ok(())
    }

    fn fake_openai_key() -> String {
        format!("sk-{}", "a".repeat(40))
    }

    fn fake_github_token() -> String {
        format!("ghp_{}", "b".repeat(40))
    }

    fn fake_anthropic_key() -> String {
        format!("sk-ant-{}", "c".repeat(40))
    }

    fn fake_gateway_url() -> String {
        "https://private-gateway.example.test/session?token=fakegatewaytoken123".to_string()
    }

    fn private_key_marker(boundary: &str) -> String {
        format!("-----{boundary} {}-----", "PRIVATE KEY")
    }
}
