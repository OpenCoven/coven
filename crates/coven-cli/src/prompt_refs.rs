use anyhow::Result;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub const MAX_TEXT_LINES: usize = 500;
#[allow(dead_code)]
pub const MAX_LINE_CHARS: usize = 2048;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    /// `@path/to/file` or `@glob/*.md`
    Path(String),
    /// `@T-<uuid>` — thread/session id reference
    Thread(String),
    /// `@@search words` — FTS5 query (runs to end-of-line)
    Search(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPrompt {
    pub raw: String,
    pub refs: Vec<Ref>,
}

#[allow(dead_code)]
pub fn parse(prompt: &str) -> ParsedPrompt {
    let mut refs = Vec::new();
    let bytes = prompt.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        // double-@ for search comes first so single-@ doesn't swallow it
        if i + 1 < bytes.len() && bytes[i + 1] == b'@' {
            let end = prompt[i..].find('\n').map(|n| i + n).unwrap_or(prompt.len());
            let body = &prompt[i + 2..end];
            if !body.trim().is_empty() {
                refs.push(Ref::Search(body.trim().to_string()));
            }
            i = end;
            continue;
        }
        // single @ — runs to next whitespace
        let end = prompt[i + 1..]
            .find(|c: char| c.is_whitespace())
            .map(|n| i + 1 + n)
            .unwrap_or(prompt.len());
        let body = &prompt[i + 1..end];
        if let Some(rest) = body.strip_prefix("T-") {
            refs.push(Ref::Thread(format!("T-{rest}")));
        } else if !body.is_empty() {
            refs.push(Ref::Path(body.to_string()));
        }
        i = end;
    }
    ParsedPrompt {
        raw: prompt.to_string(),
        refs,
    }
}

#[allow(dead_code)]
pub fn expand_path(cwd: &Path, raw: &str) -> Result<String> {
    let full = cwd.join(raw);
    if !full.exists() {
        return Ok(format!("[missing @{raw}]"));
    }
    let mime = guess_mime(&full);
    if mime.starts_with("image/") {
        let bytes = std::fs::metadata(&full)?.len();
        return Ok(format!(
            "[image @ {}: {mime}, {bytes} bytes]",
            full.display()
        ));
    }
    let raw_text = std::fs::read_to_string(&full)?;
    let mut out = String::new();
    out.push_str(&format!("--- @{raw} ---\n"));
    let mut written = 0usize;
    for line in raw_text.lines() {
        if written >= MAX_TEXT_LINES {
            out.push_str(&format!("[…truncated at {MAX_TEXT_LINES} lines]\n"));
            break;
        }
        let truncated: String = line.chars().take(MAX_LINE_CHARS).collect();
        out.push_str(&truncated);
        out.push('\n');
        written += 1;
    }
    out.push_str("--- end ---\n");
    Ok(out)
}

#[allow(dead_code)]
fn guess_mime(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png".into(),
        Some("jpg") | Some("jpeg") => "image/jpeg".into(),
        Some("gif") => "image/gif".into(),
        Some("webp") => "image/webp".into(),
        _ => "text/plain".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_path_refs() {
        let p = parse("look at @README.md and @docs/*.md please");
        assert_eq!(
            p.refs,
            vec![
                Ref::Path("README.md".into()),
                Ref::Path("docs/*.md".into()),
            ]
        );
    }

    #[test]
    fn parses_thread_ref() {
        let p = parse("continue @T-abc-123 with new ideas");
        assert_eq!(p.refs, vec![Ref::Thread("T-abc-123".into())]);
    }

    #[test]
    fn parses_search_ref_to_end_of_line() {
        let p = parse("background:\n@@phoenix rises again\ndo the thing");
        assert_eq!(p.refs, vec![Ref::Search("phoenix rises again".into())]);
    }

    #[test]
    fn bare_at_sign_followed_by_whitespace_is_ignored() {
        let p = parse("email me at @ work");
        assert!(p.refs.is_empty(), "bare @ should produce no ref, got: {:?}", p.refs);
    }

    #[test]
    fn multiple_refs_in_one_prompt() {
        let p = parse("see @README.md and @T-abc plus\n@@phoenix");
        assert_eq!(
            p.refs,
            vec![
                Ref::Path("README.md".into()),
                Ref::Thread("T-abc".into()),
                Ref::Search("phoenix".into()),
            ]
        );
    }

    #[test]
    fn expand_path_inlines_text_file_capped() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hello.md");
        std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
        let expanded = expand_path(temp.path(), "hello.md").unwrap();
        assert!(expanded.contains("line1"), "got: {expanded}");
        assert!(expanded.contains("line3"), "got: {expanded}");
        assert!(expanded.contains("hello.md"), "got: {expanded}");
    }

    #[test]
    fn expand_path_image_becomes_placeholder() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("pic.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nfake").unwrap();
        let expanded = expand_path(temp.path(), "pic.png").unwrap();
        assert!(expanded.contains("[image @ "), "got: {expanded}");
        assert!(expanded.contains("image/png"), "got: {expanded}");
    }

    #[test]
    fn expand_path_missing_returns_placeholder() {
        let temp = tempfile::tempdir().unwrap();
        let expanded = expand_path(temp.path(), "nope.md").unwrap();
        assert_eq!(expanded, "[missing @nope.md]");
    }

    #[test]
    fn expand_path_truncates_at_max_lines() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("big.md");
        let body: String = (0..(MAX_TEXT_LINES + 50))
            .map(|i| format!("line-{i}\n"))
            .collect();
        std::fs::write(&path, body).unwrap();
        let expanded = expand_path(temp.path(), "big.md").unwrap();
        assert!(expanded.contains(&format!("…truncated at {MAX_TEXT_LINES} lines")));
        assert!(!expanded.contains(&format!("line-{}", MAX_TEXT_LINES + 49)));
    }
}
