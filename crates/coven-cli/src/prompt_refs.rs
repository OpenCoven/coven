use anyhow::Result;
use std::path::{Path, PathBuf};

pub const MAX_TEXT_LINES: usize = 500;
pub const MAX_LINE_CHARS: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    /// `@path/to/file` or `@glob/*.md`
    Path(String),
    /// `@T-<uuid>` — thread/session id reference
    Thread(String),
    /// `@@search words` — FTS5 query (runs to end-of-line)
    Search(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPrompt {
    pub raw: String,
    pub refs: Vec<Ref>,
}

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
            let end = prompt[i..]
                .find('\n')
                .map(|n| i + n)
                .unwrap_or(prompt.len());
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

pub fn expand_path(cwd: &Path, raw: &str) -> Result<String> {
    if raw.contains('*') || raw.contains('?') || raw.contains('[') {
        return expand_glob(cwd, raw);
    }
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
    for (written, line) in raw_text.lines().enumerate() {
        if written >= MAX_TEXT_LINES {
            out.push_str(&format!("[…truncated at {MAX_TEXT_LINES} lines]\n"));
            break;
        }
        let truncated: String = line.chars().take(MAX_LINE_CHARS).collect();
        out.push_str(&truncated);
        out.push('\n');
    }
    out.push_str("--- end ---\n");
    Ok(out)
}

fn guess_mime(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png".into(),
        Some("jpg") | Some("jpeg") => "image/jpeg".into(),
        Some("gif") => "image/gif".into(),
        Some("webp") => "image/webp".into(),
        _ => "text/plain".into(),
    }
}

pub fn expand_thread(conn: &rusqlite::Connection, id: &str) -> Result<String> {
    use anyhow::Context;
    let mut stmt = conn
        .prepare(
            "SELECT kind, payload_json, created_at FROM events
             WHERE session_id = ?1 ORDER BY created_at ASC LIMIT 200",
        )
        .context("failed to prepare expand_thread query")?;
    let rows = stmt
        .query_map([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .context("failed to execute expand_thread query")?;
    let mut out = format!("--- @{id} ---\n");
    let mut found = false;
    for r in rows {
        let (kind, payload, ts) = r.context("failed to read expand_thread row")?;
        found = true;
        out.push_str(&format!("[{ts}] {kind}: {payload}\n"));
    }
    if !found {
        return Ok(format!("[no events for @{id}]"));
    }
    out.push_str("--- end ---\n");
    Ok(out)
}

fn expand_glob(cwd: &Path, pattern: &str) -> Result<String> {
    use globset::{Glob, GlobSetBuilder};
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern)?);
    let set = builder.build()?;
    let mut out = String::new();
    let mut count = 0usize;
    for entry in walkdir(cwd) {
        let rel = entry.strip_prefix(cwd).unwrap_or(&entry);
        if !set.is_match(rel) {
            continue;
        }
        let part = expand_path(cwd, &rel.to_string_lossy())?;
        out.push_str(&part);
        out.push('\n');
        count += 1;
        if count >= 20 {
            out.push_str("[…glob match cap reached at 20 files]\n");
            break;
        }
    }
    if count == 0 {
        return Ok(format!("[no matches for @{pattern}]"));
    }
    Ok(out)
}

fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|s| s.to_str()) == Some(".git") {
                    continue;
                }
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Expand every `@path`, `@T-<id>`, and `@@search` ref found in `prompt`.
/// Inlined content is prepended to the prompt with delimited blocks; the
/// original prompt text follows unchanged. Returns `prompt` unmodified when
/// it contains no refs.
pub fn expand_all(cwd: &Path, conn: &rusqlite::Connection, prompt: &str) -> Result<String> {
    let parsed = parse(prompt);
    if parsed.refs.is_empty() {
        return Ok(prompt.to_string());
    }
    let mut prefix = String::new();
    for r in &parsed.refs {
        let block = match r {
            Ref::Path(p) => expand_path(cwd, p)?,
            Ref::Thread(id) => expand_thread(conn, id)?,
            Ref::Search(q) => expand_search(conn, q)?,
        };
        prefix.push_str(&block);
        if !prefix.ends_with('\n') {
            prefix.push('\n');
        }
    }
    Ok(format!("{prefix}\n{prompt}"))
}

fn expand_search(conn: &rusqlite::Connection, query: &str) -> Result<String> {
    let hits = crate::store::search_events(conn, query)?;
    if hits.is_empty() {
        return Ok(format!("[no search hits for @@{query}]"));
    }
    let mut s = format!("--- @@{query} ---\n");
    for hit in hits.into_iter().take(5) {
        s.push_str(&format!(
            "[{}] {} {} {}\n",
            hit.created_at, hit.session_id, hit.kind, hit.snippet
        ));
    }
    s.push_str("--- end ---\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_path_refs() {
        let p = parse("look at @README.md and @docs/*.md please");
        assert_eq!(
            p.refs,
            vec![Ref::Path("README.md".into()), Ref::Path("docs/*.md".into()),]
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
        assert!(
            p.refs.is_empty(),
            "bare @ should produce no ref, got: {:?}",
            p.refs
        );
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

    #[test]
    fn expand_glob_includes_all_matches() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("docs")).unwrap();
        std::fs::write(temp.path().join("docs/a.md"), "AAA").unwrap();
        std::fs::write(temp.path().join("docs/b.md"), "BBB").unwrap();
        let expanded = expand_path(temp.path(), "docs/*.md").unwrap();
        assert!(expanded.contains("AAA"), "got: {expanded}");
        assert!(expanded.contains("BBB"), "got: {expanded}");
    }

    #[test]
    fn expand_glob_no_matches_returns_placeholder() {
        let temp = tempfile::tempdir().unwrap();
        let expanded = expand_path(temp.path(), "docs/*.md").unwrap();
        assert_eq!(expanded, "[no matches for @docs/*.md]");
    }

    #[test]
    fn expand_glob_caps_at_twenty() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("many")).unwrap();
        for i in 0..25 {
            std::fs::write(temp.path().join(format!("many/f{i}.txt")), format!("F{i}")).unwrap();
        }
        let expanded = expand_path(temp.path(), "many/*.txt").unwrap();
        assert!(expanded.contains("glob match cap reached at 20 files"));
    }

    #[test]
    fn expand_thread_inlines_payloads() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp.path().join("test.sqlite3"))?;
        conn.execute(
            "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
             VALUES('T-abc', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
            [],
        )?;
        conn.execute(
            "INSERT INTO events(id, session_id, kind, payload_json, created_at)
             VALUES('e1', 'T-abc', 'user', '{\"text\":\"hello world\"}', '2026-01-01')",
            [],
        )?;
        let out = expand_thread(&conn, "T-abc")?;
        assert!(out.contains("hello world"), "got: {out}");
        assert!(out.contains("T-abc"), "got: {out}");
        Ok(())
    }

    #[test]
    fn expand_thread_missing_returns_placeholder() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp.path().join("test.sqlite3"))?;
        let out = expand_thread(&conn, "T-nope")?;
        assert_eq!(out, "[no events for @T-nope]");
        Ok(())
    }

    #[test]
    fn expand_all_passes_through_when_no_refs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp.path().join("test.sqlite3"))?;
        let out = expand_all(temp.path(), &conn, "just a plain prompt with no refs")?;
        assert_eq!(out, "just a plain prompt with no refs");
        Ok(())
    }

    #[test]
    fn expand_all_inlines_path_ref_before_prompt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(temp.path().join("notes.md"), "alpha beta gamma\n")?;
        let conn = crate::store::open_store(&temp.path().join("test.sqlite3"))?;
        let out = expand_all(temp.path(), &conn, "summarise @notes.md please")?;
        assert!(out.contains("alpha beta gamma"), "got: {out}");
        let body_idx = out
            .find("summarise @notes.md please")
            .expect("original prompt should appear");
        let content_idx = out
            .find("alpha beta gamma")
            .expect("file content should appear");
        assert!(
            content_idx < body_idx,
            "inlined content should precede the original prompt; got: {out}"
        );
        Ok(())
    }

    #[test]
    fn expand_all_search_no_hits_inlines_placeholder() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp.path().join("test.sqlite3"))?;
        let out = expand_all(
            temp.path(),
            &conn,
            "context:\n@@phoenix rising\nthen answer",
        )?;
        assert!(
            out.contains("[no search hits for @@phoenix rising]"),
            "got: {out}"
        );
        assert!(
            out.contains("then answer"),
            "original prompt preserved; got: {out}"
        );
        Ok(())
    }

    #[test]
    fn expand_all_search_inlines_hits() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp.path().join("test.sqlite3"))?;
        conn.execute(
            "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
             VALUES('s-1', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
            [],
        )?;
        conn.execute(
            "INSERT INTO events(id, session_id, kind, payload_json, created_at)
             VALUES('e1', 's-1', 'user', '{\"text\":\"phoenix rising over the city\"}', '2026-01-01')",
            [],
        )?;
        let out = expand_all(temp.path(), &conn, "context:\n@@phoenix\ngo")?;
        assert!(
            out.contains("phoenix"),
            "search snippet should appear; got: {out}"
        );
        assert!(
            out.contains("--- @@phoenix ---"),
            "search delimiter; got: {out}"
        );
        Ok(())
    }

    #[test]
    fn expand_all_combines_path_and_thread_refs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(temp.path().join("intro.md"), "INTRO_BODY\n")?;
        let conn = crate::store::open_store(&temp.path().join("test.sqlite3"))?;
        conn.execute(
            "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
             VALUES('T-prev', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
            [],
        )?;
        conn.execute(
            "INSERT INTO events(id, session_id, kind, payload_json, created_at)
             VALUES('e1', 'T-prev', 'user', '{\"text\":\"PREV_PAYLOAD\"}', '2026-01-01')",
            [],
        )?;
        let out = expand_all(temp.path(), &conn, "read @intro.md and continue @T-prev")?;
        assert!(out.contains("INTRO_BODY"), "got: {out}");
        assert!(out.contains("PREV_PAYLOAD"), "got: {out}");
        assert!(
            out.contains("read @intro.md and continue @T-prev"),
            "original prompt preserved; got: {out}"
        );
        Ok(())
    }
}
