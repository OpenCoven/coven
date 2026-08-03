use anyhow::{bail, Result};
use std::path::{Component, Path};

pub fn validate_portable_reference(reference: &str) -> Result<()> {
    if reference.trim().is_empty() {
        bail!("portable reference must not be empty");
    }
    if reference.starts_with("agent:") {
        bail!("runtime-specific session keys are not portable");
    }
    if is_absolute_on_any_supported_platform(reference) {
        bail!("absolute paths are not portable");
    }
    if Path::new(reference)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("parent traversal is not allowed in portable references");
    }
    if let Some(session) = reference.strip_prefix("session://") {
        let mut parts = session.split('/');
        if parts.next().is_none_or(str::is_empty)
            || parts.next().is_none_or(str::is_empty)
            || parts.next().is_none_or(str::is_empty)
            || parts.next().is_some()
        {
            bail!("session reference must use session://<familiar>/<date>/<slug>");
        }
    }
    Ok(())
}

fn is_absolute_on_any_supported_platform(value: &str) -> bool {
    if value.starts_with('/') || value.starts_with('\\') {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

#[cfg(test)]
mod tests {
    use super::validate_portable_reference;

    #[test]
    fn portable_reference_accepts_relative_path() {
        validate_portable_reference("memory/example.md").expect("relative path should pass");
    }

    #[test]
    fn portable_reference_accepts_session_class() {
        validate_portable_reference("session://<familiar-id>/2026-07-24/example")
            .expect("portable session class should pass");
    }

    #[test]
    fn portable_reference_rejects_unix_absolute_path() {
        let error = validate_portable_reference("/absolute/example.md").expect_err("absolute path");

        assert!(error.to_string().contains("absolute"));
    }

    #[test]
    fn portable_reference_rejects_windows_absolute_path() {
        let error =
            validate_portable_reference(r"C:\absolute\example.md").expect_err("absolute path");

        assert!(error.to_string().contains("absolute"));
    }

    #[test]
    fn portable_reference_rejects_parent_traversal() {
        let error = validate_portable_reference("../example.md").expect_err("parent traversal");

        assert!(error.to_string().contains("traversal"));
    }

    #[test]
    fn portable_reference_rejects_runtime_session_key() {
        let reference = ["agent", "example", "webchat", "direct", "123456789"].join(":");
        let error = validate_portable_reference(&reference).expect_err("runtime session key");

        assert!(error.to_string().contains("runtime-specific"));
    }
}
