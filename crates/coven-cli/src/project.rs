use std::path::{Path, PathBuf};

pub fn canonical_project_root(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(path.canonicalize()?)
}

pub fn resolve_inside_root(root: &Path, cwd: Option<&Path>) -> anyhow::Result<PathBuf> {
    let root = canonical_project_root(root)?;
    let candidate = match cwd {
        Some(cwd) if cwd.is_absolute() => cwd.to_path_buf(),
        Some(cwd) => root.join(cwd),
        None => root.clone(),
    };
    let candidate = candidate.canonicalize()?;

    if candidate == root || candidate.starts_with(&root) {
        Ok(candidate)
    } else {
        anyhow::bail!("cwd is outside the Coven project root");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn canonical_project_root_canonicalizes_project_root() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path().join("project");
        fs::create_dir(&root)?;

        let actual = canonical_project_root(&root)?;

        assert_eq!(actual, root.canonicalize()?);
        Ok(())
    }

    #[test]
    fn resolve_inside_root_accepts_root_itself() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path().join("project");
        fs::create_dir(&root)?;

        let actual = resolve_inside_root(&root, None)?;

        assert_eq!(actual, root.canonicalize()?);
        Ok(())
    }

    #[test]
    fn resolve_inside_root_accepts_child_path() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path().join("project");
        let child = root.join("child");
        fs::create_dir(&root)?;
        fs::create_dir(&child)?;

        let actual = resolve_inside_root(&root, Some(Path::new("child")))?;

        assert_eq!(actual, child.canonicalize()?);
        Ok(())
    }

    #[test]
    fn resolve_inside_root_rejects_outside_root() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path().join("project");
        let outside = temp_dir.path().join("outside");
        fs::create_dir(&root)?;
        fs::create_dir(&outside)?;

        let error = resolve_inside_root(&root, Some(&outside)).unwrap_err();

        assert!(
            error.to_string().contains("outside the Coven project root"),
            "unexpected error: {error:?}"
        );
        Ok(())
    }

    #[test]
    fn resolve_inside_root_rejects_symlink_escape() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path().join("project");
        let outside = temp_dir.path().join("outside");
        let escape = root.join("escape");
        fs::create_dir(&root)?;
        fs::create_dir(&outside)?;

        if create_dir_symlink(&outside, &escape).is_err() {
            return Ok(());
        }

        let error = resolve_inside_root(&root, Some(Path::new("escape"))).unwrap_err();

        assert!(
            error.to_string().contains("outside the Coven project root"),
            "unexpected error: {error:?}"
        );
        Ok(())
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(not(any(unix, windows)))]
    fn create_dir_symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "directory symlinks are unsupported on this platform",
        ))
    }
}
