//! Path normalization helpers.
//!
//! All public APIs accept arbitrary slash-separated paths and normalize them
//! to absolute, `/`-rooted, canonical form (no empty components, `.` removed,
//! `..` resolved lexically, never escaping the root).

/// Normalize a path to canonical absolute form.
///
/// `""`, `"/"`, `"."` and `"/../.."` all normalize to `"/"`.
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Parent of a normalized path. The parent of `/` is `/` (SPEC: for the root
/// directory, `parent_path` is `/`).
pub fn parent(normalized: &str) -> String {
    match normalized.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(idx) => normalized[..idx].to_string(),
    }
}

/// Final component of a normalized path. The basename of `/` is `""`.
pub fn basename(normalized: &str) -> &str {
    match normalized.rfind('/') {
        Some(idx) => &normalized[idx + 1..],
        None => normalized,
    }
}

/// Non-empty components of a normalized path, in order.
pub fn components(normalized: &str) -> impl Iterator<Item = &str> {
    normalized.split('/').filter(|c| !c.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_to_root() {
        for p in ["", "/", ".", "//", "/./", "/..", "/a/.."] {
            assert_eq!(normalize(p), "/", "input {p:?}");
        }
    }

    #[test]
    fn normalizes_relative_and_messy_paths() {
        assert_eq!(normalize("a/b"), "/a/b");
        assert_eq!(normalize("/a//b/./c"), "/a/b/c");
        assert_eq!(normalize("/a/b/../c"), "/a/c");
        assert_eq!(normalize("/../a"), "/a");
    }

    #[test]
    fn parent_and_basename() {
        assert_eq!(parent("/"), "/");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(basename("/"), "");
        assert_eq!(basename("/a"), "a");
        assert_eq!(basename("/a/b"), "b");
    }

    #[test]
    fn components_iterates() {
        let c: Vec<&str> = components("/a/b/c").collect();
        assert_eq!(c, vec!["a", "b", "c"]);
        assert_eq!(components("/").count(), 0);
    }
}
