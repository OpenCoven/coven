use std::path::Path;

use anyhow::{anyhow, Result};

use crate::{cockpit_sources, harness};

pub(crate) fn resolve_optional(
    coven_home: &Path,
    familiar_id: Option<&str>,
) -> Result<Option<harness::FamiliarContext>> {
    let Some(familiar_id) = familiar_id
        .map(str::trim)
        .filter(|familiar_id| !familiar_id.is_empty())
    else {
        return Ok(None);
    };

    resolve(coven_home, familiar_id)?
        .map(Some)
        .ok_or_else(|| unknown_familiar_error(coven_home, familiar_id))
}

pub(crate) fn resolve(
    coven_home: &Path,
    familiar_id: &str,
) -> Result<Option<harness::FamiliarContext>> {
    Ok(cockpit_sources::read_familiars(coven_home)?
        .into_iter()
        .find(|familiar| familiar.id == familiar_id)
        .map(|familiar| harness::FamiliarContext {
            id: familiar.id,
            display_name: familiar.display_name,
            role: Some(familiar.role).filter(|role| !role.is_empty()),
        }))
}

pub(crate) fn known_ids(coven_home: &Path) -> Result<Vec<String>> {
    Ok(cockpit_sources::read_familiars(coven_home)?
        .into_iter()
        .map(|familiar| familiar.id)
        .collect())
}

pub(crate) fn unknown_familiar_error(coven_home: &Path, familiar_id: &str) -> anyhow::Error {
    let known = known_ids(coven_home).unwrap_or_default();
    if known.is_empty() {
        anyhow!(
            "unknown familiar `{familiar_id}`; no familiars are configured in {}",
            coven_home.join("familiars.toml").display()
        )
    } else {
        anyhow!(
            "unknown familiar `{familiar_id}`; expected one of: {}",
            known.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_optional_returns_context_for_known_familiar() -> Result<()> {
        let temp = tempfile::tempdir()?;
        seed_familiars(temp.path())?;

        let context = resolve_optional(temp.path(), Some("sage"))?.expect("known familiar");

        assert_eq!(context.id, "sage");
        assert_eq!(context.display_name, "Sage");
        assert_eq!(context.role.as_deref(), Some("Research"));
        Ok(())
    }

    #[test]
    fn resolve_optional_rejects_unknown_familiar() -> Result<()> {
        let temp = tempfile::tempdir()?;
        seed_familiars(temp.path())?;

        let error = resolve_optional(temp.path(), Some("missing")).unwrap_err();

        assert!(error.to_string().contains("unknown familiar `missing`"));
        assert!(error.to_string().contains("sage"));
        Ok(())
    }

    fn seed_familiars(coven_home: &Path) -> Result<()> {
        std::fs::write(
            coven_home.join("familiars.toml"),
            r#"
[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Reads and synthesizes."
"#,
        )?;
        Ok(())
    }
}
