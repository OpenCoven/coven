use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const FAMILIARS_CONFIG_FILE: &str = "familiars.toml";
const SKILLS_DIR: &str = "skills";
const MEMORY_DIR: &str = "memory";
const RESEARCH_TSV: &str = "research/results.tsv";

#[derive(Debug, Clone, Serialize)]
pub struct FamiliarDto {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub emoji: String,
    /// Optional glyph hint. Either a literal emoji char (`"🐈"`) or a
    /// Phosphor icon name (`"ph:cat-fill"`). Clients use this in preference
    /// to `emoji` when they have a richer icon system — see CovenCave's
    /// glyph picker. Omitted from the wire when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub role: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_channel: Option<String>,
    pub last_seen: String,
    pub active_sessions: u32,
    pub memory_freshness: String,
    /// Explicit workspace path declared in familiars.toml. `None` means
    /// the daemon uses the conventional `~/.coven/familiars/<id>/` path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<std::path::PathBuf>,
}

#[derive(Debug, Deserialize)]
struct FamiliarsFile {
    #[serde(default)]
    familiar: Vec<FamiliarEntry>,
}

#[derive(Debug, Deserialize)]
struct FamiliarEntry {
    id: String,
    name: Option<String>,
    display_name: String,
    emoji: Option<String>,
    /// See [`FamiliarDto::icon`]. Free-form string at this layer — the
    /// renderer decides whether to treat a `ph:` prefix as an icon vs.
    /// treat anything else as an emoji literal.
    icon: Option<String>,
    role: String,
    description: String,
    pronouns: Option<String>,
    active_channel: Option<String>,
    /// Explicit workspace path for this familiar. When set, the daemon uses this
    /// instead of the conventional `~/.coven/familiars/<id>/` path.
    /// Accepts `~` expansion. Optional — most familiars do not need to set this.
    workspace: Option<String>,
}

pub fn read_familiars(coven_home: &Path) -> Result<Vec<FamiliarDto>> {
    let path = coven_home.join(FAMILIARS_CONFIG_FILE);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let parsed: FamiliarsFile =
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
    let memory_root = coven_home.join(MEMORY_DIR);
    let mut out = Vec::with_capacity(parsed.familiar.len());
    for entry in parsed.familiar {
        let memory_dir = memory_root.join(&entry.id);
        let memory_freshness = latest_mtime(&memory_dir)
            .map(relative_time)
            .unwrap_or_else(|| "—".to_string());
        out.push(FamiliarDto {
            name: entry.name.unwrap_or_else(|| entry.id.clone()),
            display_name: entry.display_name,
            emoji: entry.emoji.unwrap_or_default(),
            icon: entry.icon.and_then(|s| {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }),
            role: entry.role,
            description: entry.description,
            pronouns: entry.pronouns,
            status: "offline".to_string(),
            active_channel: entry.active_channel,
            last_seen: "—".to_string(),
            active_sessions: 0,
            memory_freshness,
            workspace: entry.workspace.map(|p| {
                // Expand leading ~ to home directory
                if let Some(rest) = p.strip_prefix("~/") {
                    dirs_next::home_dir()
                        .map(|home| home.join(rest))
                        .unwrap_or_else(|| std::path::PathBuf::from(&p))
                } else if p == "~" {
                    dirs_next::home_dir().unwrap_or_else(|| std::path::PathBuf::from(&p))
                } else {
                    std::path::PathBuf::from(p)
                }
            }),
            id: entry.id,
        });
    }
    Ok(out)
}

/// Outcome of a [`write_familiar_icon`] call.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteFamiliarIconOutcome {
    /// The named familiar's `icon` field was updated (or inserted) in-place.
    Updated,
    /// The named familiar's `icon` field was removed because the new value
    /// was `None` or whitespace-only.
    Cleared,
    /// No `[[familiar]]` block in `familiars.toml` has a matching `id`.
    NotFound,
}

/// Update (or clear) a familiar's `icon` field in `~/.coven/familiars.toml`,
/// preserving the rest of the file's formatting + comments.
///
/// `icon = None` (or a whitespace-only `Some`) removes the field entirely.
/// `icon = Some("ph:cat-fill")` or `Some("🐈‍⬛")` either inserts or replaces
/// the value. Returns the [`WriteFamiliarIconOutcome`] so callers can map
/// `NotFound` → 404 without re-reading the file.
///
/// Writes are atomic via `tempfile + rename` inside the same directory so a
/// crash mid-write can never leave a half-written `familiars.toml`.
pub fn write_familiar_icon(
    coven_home: &Path,
    familiar_id: &str,
    icon: Option<&str>,
) -> Result<WriteFamiliarIconOutcome> {
    use toml_edit::{value, DocumentMut};

    let path = coven_home.join(FAMILIARS_CONFIG_FILE);
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut doc: DocumentMut = raw
        .parse()
        .with_context(|| format!("failed to parse {}", path.display()))?;

    // Normalize whitespace-only icons to None at the boundary so the file
    // never carries an empty glyph.
    let normalized: Option<&str> = icon.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });

    // `[[familiar]]` arrays of tables live under the top-level `familiar`
    // key as an `ArrayOfTables`. Scan for the table whose `id` matches.
    let array = match doc
        .get_mut("familiar")
        .and_then(|item| item.as_array_of_tables_mut())
    {
        Some(arr) => arr,
        None => return Ok(WriteFamiliarIconOutcome::NotFound),
    };

    let target = array.iter_mut().find(|tbl| {
        tbl.get("id")
            .and_then(|item| item.as_str())
            .map(|s| s == familiar_id)
            .unwrap_or(false)
    });

    let table = match target {
        Some(t) => t,
        None => return Ok(WriteFamiliarIconOutcome::NotFound),
    };

    let outcome = match normalized {
        Some(s) => {
            table["icon"] = value(s);
            WriteFamiliarIconOutcome::Updated
        }
        None => {
            if table.remove("icon").is_some() {
                WriteFamiliarIconOutcome::Cleared
            } else {
                // Nothing to clear, but still a successful no-op write.
                WriteFamiliarIconOutcome::Cleared
            }
        }
    };

    // Atomic write: write to a sibling tempfile in the same directory so the
    // subsequent `rename` is on the same filesystem and POSIX-atomic. A crash
    // mid-write can leave `.familiars.toml.tmp` behind but never a half-
    // written `familiars.toml`.
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_path = dir.join(".familiars.toml.tmp");
    fs::write(&tmp_path, doc.to_string())
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("failed to rename tempfile over {}", path.display()))?;

    Ok(outcome)
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillDto {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub category: String,
    pub tags: Vec<String>,
    pub score: f64,
    pub effective_rate: f64,
    pub applied_rate: f64,
    pub completion_rate: f64,
    pub fallback_rate: f64,
    pub version: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct SkillMetadata {
    name: String,
    description: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    category: Option<String>,
}

pub fn scan_skills(coven_home: &Path) -> Result<Vec<SkillDto>> {
    let root = coven_home.join(SKILLS_DIR);
    let entries = match fs::read_dir(&root) {
        Ok(it) => it,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", root.display()));
        }
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        let dir = entry.path();
        match fs::metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => continue,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("failed to inspect {}", dir.display()));
            }
        }
        let metadata_path = dir.join("metadata.json");
        let raw = match fs::read_to_string(&metadata_path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read {}", metadata_path.display()));
            }
        };
        let meta: SkillMetadata = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", metadata_path.display()))?;
        let id = entry.file_name().to_string_lossy().into_owned();
        out.push(SkillDto {
            id,
            name: meta.name,
            owner: meta.author.unwrap_or_else(|| "unknown".to_string()),
            category: meta.category.unwrap_or_else(|| "general".to_string()),
            tags: meta.tags,
            score: 0.0,
            effective_rate: 0.0,
            applied_rate: 0.0,
            completion_rate: 0.0,
            fallback_rate: 0.0,
            version: meta.version.unwrap_or_else(|| "0.0.0".to_string()),
            description: meta.description,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryFileDto {
    pub id: String,
    pub familiar_id: String,
    pub title: String,
    pub path: String,
    pub updated_at: String,
    pub excerpt: String,
}

pub fn scan_memory(coven_home: &Path) -> Result<Vec<MemoryFileDto>> {
    let root = coven_home.join(MEMORY_DIR);
    let familiar_dirs = match fs::read_dir(&root) {
        Ok(it) => it,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", root.display()));
        }
    };
    let mut out = Vec::new();
    for familiar_entry in familiar_dirs {
        let familiar_entry = familiar_entry?;
        if !familiar_entry.file_type()?.is_dir() {
            continue;
        }
        let familiar_id = familiar_entry.file_name().to_string_lossy().into_owned();
        let familiar_dir = familiar_entry.path();
        let file_iter = fs::read_dir(&familiar_dir)
            .with_context(|| format!("failed to read {}", familiar_dir.display()))?;
        for file_entry in file_iter {
            let file_entry = file_entry?;
            if !file_entry.file_type()?.is_file() {
                continue;
            }
            let file_path = file_entry.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let stem = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled")
                .to_string();
            let file_name = file_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled.md")
                .to_string();
            let metadata = file_entry.metadata()?;
            let updated_at = metadata
                .modified()
                .ok()
                .map(relative_time)
                .unwrap_or_else(|| "—".to_string());
            let body = fs::read_to_string(&file_path)
                .with_context(|| format!("failed to read {}", file_path.display()))?;
            let excerpt = first_paragraph(&body, 200);
            out.push(MemoryFileDto {
                id: format!("{familiar_id}-{stem}"),
                familiar_id: familiar_id.clone(),
                title: stem,
                path: format!("{familiar_id}/{file_name}"),
                updated_at,
                excerpt,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct ResearchRowDto {
    pub iteration: u32,
    pub topic: String,
    pub score: f64,
    pub delta: f64,
    pub decision: String,
    pub source: String,
}

pub fn read_research(coven_home: &Path) -> Result<Vec<ResearchRowDto>> {
    let path = coven_home.join(RESEARCH_TSV);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        if cols.len() < 6 {
            continue;
        }
        // First non-numeric column means a header row — skip it.
        let Ok(iteration) = cols[0].parse::<u32>() else {
            continue;
        };
        out.push(ResearchRowDto {
            iteration,
            topic: cols[1].to_string(),
            score: cols[2].parse().unwrap_or(0.0),
            delta: cols[3].parse().unwrap_or(0.0),
            decision: cols[4].to_string(),
            source: cols[5].to_string(),
        });
    }
    Ok(out)
}

fn latest_mtime(dir: &Path) -> Option<SystemTime> {
    let entries = fs::read_dir(dir).ok()?;
    let mut latest: Option<SystemTime> = None;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                latest = Some(latest.map_or(modified, |cur| cur.max(modified)));
            }
        }
    }
    latest
}

fn relative_time(then: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(then) else {
        return "future".to_string();
    };
    let secs = elapsed.as_secs();
    if secs < 60 {
        return "now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    let months = days / 30;
    format!("{months}mo ago")
}

fn first_paragraph(body: &str, cap: usize) -> String {
    let mut buf = String::new();
    let mut in_frontmatter = false;
    let mut saw_frontmatter_open = false;
    for line in body.lines() {
        let trimmed = line.trim();
        // Skip a leading YAML frontmatter block (--- ... ---).
        if !saw_frontmatter_open && trimmed == "---" {
            in_frontmatter = true;
            saw_frontmatter_open = true;
            continue;
        }
        if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
            }
            continue;
        }
        if trimmed.is_empty() {
            if !buf.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(trimmed);
        if buf.len() >= cap {
            break;
        }
    }
    if buf.chars().count() > cap {
        buf = buf.chars().take(cap).collect::<String>();
        buf.push('…');
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn read_familiars_returns_empty_when_config_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(read_familiars(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn read_familiars_parses_toml_entries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"
[[familiar]]
id = "sage"
display_name = "Sage"
emoji = "🌿"
role = "Research familiar"
description = "Reads, synthesizes."
pronouns = "they/them"
active_channel = "telegram"

[[familiar]]
id = "cody"
display_name = "Cody"
emoji = "⚡"
role = "Code"
description = "Builds and debugs."
"#,
        )?;
        let out = read_familiars(temp.path())?;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "sage");
        assert_eq!(out[0].name, "sage");
        assert_eq!(out[0].display_name, "Sage");
        assert_eq!(out[0].emoji, "🌿");
        assert_eq!(out[0].pronouns.as_deref(), Some("they/them"));
        assert_eq!(out[0].active_channel.as_deref(), Some("telegram"));
        assert_eq!(out[0].status, "offline");
        assert_eq!(out[0].active_sessions, 0);
        assert_eq!(out[0].memory_freshness, "—");
        assert_eq!(out[1].id, "cody");
        assert!(out[1].pronouns.is_none());
        assert!(out[1].active_channel.is_none());
        // No `icon` field set in this fixture — must round-trip as None.
        assert!(out[0].icon.is_none());
        assert!(out[1].icon.is_none());
        Ok(())
    }

    #[test]
    fn read_familiars_carries_icon_field_for_both_shapes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"
[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
icon = "ph:lightning-fill"

[[familiar]]
id = "kitten"
display_name = "Kitten"
role = "General"
description = "..."
icon = "🐈‍⬛"

[[familiar]]
id = "whitespace"
display_name = "Whitespace"
role = "Edge case"
description = "..."
icon = "   "

[[familiar]]
id = "no-icon"
display_name = "No icon"
role = "Edge case"
description = "..."
"#,
        )?;
        let out = read_familiars(temp.path())?;
        assert_eq!(out[0].icon.as_deref(), Some("ph:lightning-fill"));
        assert_eq!(out[1].icon.as_deref(), Some("🐈‍⬛"));
        // Whitespace-only icon must normalize to None so clients don't try to
        // render an empty glyph.
        assert!(
            out[2].icon.is_none(),
            "whitespace icon should normalize to None"
        );
        assert!(out[3].icon.is_none());
        Ok(())
    }

    #[test]
    fn familiar_dto_skips_serializing_absent_icon() -> Result<()> {
        let dto_without = FamiliarDto {
            id: "sage".to_string(),
            name: "sage".to_string(),
            display_name: "Sage".to_string(),
            emoji: "🌿".to_string(),
            icon: None,
            role: "Research".to_string(),
            description: "...".to_string(),
            pronouns: None,
            status: "offline".to_string(),
            active_channel: None,
            last_seen: "—".to_string(),
            active_sessions: 0,
            memory_freshness: "—".to_string(),
            workspace: None,
        };
        let json = serde_json::to_string(&dto_without)?;
        assert!(
            !json.contains("\"icon\""),
            "absent icon must not appear on the wire: {json}"
        );
        let dto_with = FamiliarDto {
            icon: Some("ph:cat-fill".to_string()),
            ..dto_without
        };
        let json = serde_json::to_string(&dto_with)?;
        assert!(json.contains("\"icon\":\"ph:cat-fill\""), "got {json}");
        Ok(())
    }

    #[test]
    fn write_familiar_icon_inserts_when_absent_and_preserves_other_fields() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"# top of file
[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "Builds and debugs."
# trailing comment
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "cody", Some("ph:lightning-fill"))?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::Updated);
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(raw.contains("icon = \"ph:lightning-fill\""), "got {raw}");
        // Existing fields + comments must be preserved.
        assert!(raw.contains("display_name = \"Cody\""));
        assert!(raw.contains("# top of file"));
        assert!(raw.contains("# trailing comment"));
        // Round-trip through the reader.
        let read = read_familiars(temp.path())?;
        assert_eq!(read[0].icon.as_deref(), Some("ph:lightning-fill"));
        Ok(())
    }

    #[test]
    fn write_familiar_icon_replaces_existing_value() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
icon = "ph:lightning-fill"
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "cody", Some("🐈"))?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::Updated);
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(raw.contains("icon = \"🐈\""), "got {raw}");
        assert!(
            !raw.contains("ph:lightning-fill"),
            "old icon should be gone"
        );
        Ok(())
    }

    #[test]
    fn write_familiar_icon_clears_field_when_value_is_none() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
icon = "ph:lightning-fill"
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "cody", None)?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::Cleared);
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(!raw.contains("icon ="), "icon line must be removed: {raw}");
        let read = read_familiars(temp.path())?;
        assert!(read[0].icon.is_none());
        Ok(())
    }

    #[test]
    fn write_familiar_icon_treats_whitespace_as_clear() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
icon = "ph:lightning-fill"
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "cody", Some("   "))?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::Cleared);
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(!raw.contains("icon ="), "icon line must be removed: {raw}");
        Ok(())
    }

    #[test]
    fn write_familiar_icon_returns_not_found_for_unknown_id() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "ghost", Some("ph:ghost-fill"))?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::NotFound);
        // File must be unchanged when not found.
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(!raw.contains("ph:ghost-fill"));
        Ok(())
    }

    #[test]
    fn write_familiar_icon_leaves_no_tempfile_on_success() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
"#,
        )?;
        write_familiar_icon(temp.path(), "cody", Some("ph:cat-fill"))?;
        let tmp_path = temp.path().join(".familiars.toml.tmp");
        assert!(!tmp_path.exists(), "atomic write left a tempfile behind");
        Ok(())
    }

    #[test]
    fn read_familiars_memory_freshness_reflects_recent_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"
[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "..."
"#,
        )?;
        let sage_dir = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage_dir)?;
        fs::write(sage_dir.join("note.md"), "hi")?;
        let out = read_familiars(temp.path())?;
        assert_ne!(out[0].memory_freshness, "—");
        Ok(())
    }

    #[test]
    fn scan_skills_returns_empty_when_dir_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(scan_skills(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn scan_skills_parses_metadata_per_subdir_and_skips_subdirs_without_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let skills_root = temp.path().join(SKILLS_DIR);
        let alpha = skills_root.join("alpha");
        let beta = skills_root.join("beta");
        let gamma_no_meta = skills_root.join("gamma");
        fs::create_dir_all(&alpha)?;
        fs::create_dir_all(&beta)?;
        fs::create_dir_all(&gamma_no_meta)?;
        fs::write(
            alpha.join("metadata.json"),
            r#"{"name":"Alpha","description":"A","version":"1.0.0","author":"sage","tags":["x"],"category":"research"}"#,
        )?;
        fs::write(
            beta.join("metadata.json"),
            r#"{"name":"Beta","description":"B"}"#,
        )?;
        let out = scan_skills(temp.path())?;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "alpha");
        assert_eq!(out[0].name, "Alpha");
        assert_eq!(out[0].owner, "sage");
        assert_eq!(out[0].version, "1.0.0");
        assert_eq!(out[0].tags, vec!["x"]);
        assert_eq!(out[0].category, "research");
        assert_eq!(out[0].score, 0.0);
        assert_eq!(out[1].id, "beta");
        assert_eq!(out[1].owner, "unknown");
        assert_eq!(out[1].version, "0.0.0");
        assert_eq!(out[1].category, "general");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scan_skills_follows_symlinked_skill_dirs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let canonical = temp.path().join("canonical").join("delta");
        fs::create_dir_all(&canonical)?;
        fs::write(
            canonical.join("metadata.json"),
            r#"{"name":"Delta","description":"D","author":"coven","category":"operations"}"#,
        )?;

        let skills_root = temp.path().join(SKILLS_DIR);
        fs::create_dir_all(&skills_root)?;
        std::os::unix::fs::symlink(&canonical, skills_root.join("delta"))?;

        let out = scan_skills(temp.path())?;

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "delta");
        assert_eq!(out[0].name, "Delta");
        assert_eq!(out[0].owner, "coven");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scan_skills_skips_dangling_symlinks() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let skills_root = temp.path().join(SKILLS_DIR);
        fs::create_dir_all(&skills_root)?;
        std::os::unix::fs::symlink(
            temp.path().join("missing-skill"),
            skills_root.join("missing-skill"),
        )?;

        let out = scan_skills(temp.path())?;

        assert!(out.is_empty());
        Ok(())
    }

    #[test]
    fn scan_memory_returns_empty_when_dir_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(scan_memory(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn scan_memory_groups_md_files_by_familiar_with_excerpts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        let echo = temp.path().join(MEMORY_DIR).join("echo");
        fs::create_dir_all(&sage)?;
        fs::create_dir_all(&echo)?;
        fs::write(
            sage.join("notes.md"),
            "# Title\n\nFirst paragraph about research synthesis.\n\nSecond paragraph ignored.",
        )?;
        fs::write(sage.join("ignored.txt"), "not a markdown file")?;
        fs::write(
            echo.join("reflections.md"),
            "---\nfrontmatter: skipped\n---\n\nReflection excerpt body.",
        )?;
        let out = scan_memory(temp.path())?;
        assert_eq!(out.len(), 2);
        // sorted by path
        assert_eq!(out[0].familiar_id, "echo");
        assert_eq!(out[0].path, "echo/reflections.md");
        assert_eq!(out[0].excerpt, "Reflection excerpt body.");
        assert_eq!(out[1].familiar_id, "sage");
        assert_eq!(out[1].path, "sage/notes.md");
        assert!(out[1].excerpt.starts_with("First paragraph"));
        Ok(())
    }

    #[test]
    fn read_research_returns_empty_when_file_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(read_research(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn read_research_parses_tsv_and_skips_header_and_blank_rows() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let research_dir = temp.path().join("research");
        fs::create_dir_all(&research_dir)?;
        let body = "iteration\ttopic\tscore\tdelta\tdecision\tsource\n\
                    1\tHarness landscape\t0.61\t0.00\taccepted\tweb research\n\
                    \n\
                    # comment line\n\
                    2\tEval awareness\t0.68\t0.07\twatch\tpaper synthesis\n";
        fs::write(research_dir.join("results.tsv"), body)?;
        let out = read_research(temp.path())?;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].iteration, 1);
        assert_eq!(out[0].topic, "Harness landscape");
        assert_eq!(out[0].score, 0.61);
        assert_eq!(out[0].decision, "accepted");
        assert_eq!(out[1].iteration, 2);
        assert_eq!(out[1].decision, "watch");
        Ok(())
    }

    #[test]
    fn first_paragraph_truncates_long_bodies_with_ellipsis() {
        let body = "x".repeat(500);
        let excerpt = first_paragraph(&body, 100);
        assert_eq!(excerpt.chars().count(), 101);
        assert!(excerpt.ends_with('…'));
    }
}
