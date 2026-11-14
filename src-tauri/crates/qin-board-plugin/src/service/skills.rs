//! Discover local qin-code skills and custom slash commands for the composer menu.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_READ_BYTES: usize = 16 * 1024;
const MAX_DESC_CHARS: usize = 120;

fn qin_code_home() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("QIN_CODE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(custom));
    }
    dirs::home_dir().map(|h| h.join(".qin-code"))
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    let clean: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max {
        clean
    } else {
        let mut out: String = clean.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Extract `(name, description)` from optional YAML frontmatter or first heading/paragraph.
pub fn parse_markdown_meta(raw: &str, fallback_name: &str) -> (String, String) {
    let text = raw.trim_start_matches('\u{feff}');
    let mut name: Option<String> = None;
    let mut desc: Option<String> = None;

    if let Some(after_open) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    {
        if let Some(end) = after_open.find("\n---") {
            let fm = &after_open[..end];
            let mut lines = fm.lines().peekable();
            while let Some(line) = lines.next() {
                let trimmed = line.trim();
                if let Some(val) = trimmed.strip_prefix("name:") {
                    let v = unquote(val);
                    if !v.is_empty() {
                        name = Some(v.to_string());
                    }
                } else if let Some(val) = trimmed.strip_prefix("description:") {
                    let v = unquote(val);
                    if v == "|" || v == ">" || v == "|-" || v == ">-" || v.is_empty() {
                        let mut parts = Vec::new();
                        while let Some(next) = lines.peek() {
                            if next.starts_with(' ') || next.starts_with('\t') {
                                parts.push(next.trim());
                                lines.next();
                            } else {
                                break;
                            }
                        }
                        if !parts.is_empty() {
                            desc = Some(parts.join(" "));
                        }
                    } else {
                        desc = Some(v.to_string());
                    }
                }
            }
        }
    }

    if desc.is_none() {
        let body = if let Some(after_open) = text
            .strip_prefix("---\n")
            .or_else(|| text.strip_prefix("---\r\n"))
        {
            after_open
                .find("\n---")
                .map(|i| &after_open[i + 4..])
                .unwrap_or(text)
        } else {
            text
        };
        for line in body.lines() {
            let t = line.trim().trim_start_matches('#').trim();
            if !t.is_empty() && !t.starts_with("<!--") {
                desc = Some(t.to_string());
                break;
            }
        }
    }

    let final_name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| fallback_name.to_string());
    let final_desc = desc
        .map(|d| truncate_chars(&d, MAX_DESC_CHARS))
        .unwrap_or_default();
    (final_name, final_desc)
}

fn read_prefix(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let slice = &bytes[..bytes.len().min(MAX_READ_BYTES)];
    Some(String::from_utf8_lossy(slice).into_owned())
}

fn collect_skills_dir(dir: &Path, scope: &str, seen: &mut HashSet<String>, out: &mut Vec<Value>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for entry in paths {
        let Some(stem) = entry.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if stem.starts_with('.') {
            continue;
        }
        let skill_md = entry.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let Some(raw) = read_prefix(&skill_md) else {
            continue;
        };
        let (name, description) = parse_markdown_meta(&raw, stem);
        let clean = name.trim().trim_start_matches('/').to_string();
        if clean.is_empty() || !seen.insert(clean.clone()) {
            continue;
        }
        out.push(json!({
            "name": clean,
            "description": description,
            "scope": scope,
        }));
    }
}

fn collect_commands_dir(dir: &Path, scope: &str, seen: &mut HashSet<String>, out: &mut Vec<Value>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for entry in paths {
        if !entry.is_file() || entry.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = entry.file_stem().and_then(|n| n.to_str()) else {
            continue;
        };
        if stem.starts_with('.') {
            continue;
        }
        let Some(raw) = read_prefix(&entry) else {
            continue;
        };
        let (name, description) = parse_markdown_meta(&raw, stem);
        let clean = name.trim().trim_start_matches('/').to_string();
        if clean.is_empty() || !seen.insert(clean.clone()) {
            continue;
        }
        out.push(json!({
            "name": clean,
            "description": description,
            "scope": scope,
        }));
    }
}

pub fn list_slash_catalog(working_dir: Option<&str>) -> Value {
    let mut seen = HashSet::new();
    let mut skills = Vec::new();
    let mut commands = Vec::new();

    if let Some(wd) = working_dir.filter(|s| !s.trim().is_empty()) {
        let root = Path::new(wd);
        collect_commands_dir(
            &root.join(".qin-code/commands"),
            "project",
            &mut seen,
            &mut commands,
        );
        collect_skills_dir(
            &root.join(".qin-code/skills"),
            "project",
            &mut seen,
            &mut skills,
        );
        collect_skills_dir(
            &root.join(".claude/skills"),
            "project",
            &mut seen,
            &mut skills,
        );
    }

    if let Some(home) = qin_code_home() {
        collect_commands_dir(&home.join("commands"), "user", &mut seen, &mut commands);
        collect_skills_dir(&home.join("skills"), "user", &mut seen, &mut skills);
        collect_skills_dir(
            &home.join("skills-builtin"),
            "builtin",
            &mut seen,
            &mut skills,
        );
    }

    json!({
        "skills": skills,
        "commands": commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_folded_descriptions() {
        let doc = "---\nname: quality-review\ndescription: >\n  Independent code review\n  with severity ranking.\n---\n# Body\n";
        let (name, desc) = parse_markdown_meta(doc, "fallback");
        assert_eq!(name, "quality-review");
        assert_eq!(desc, "Independent code review with severity ranking.");
    }

    #[test]
    fn falls_back_to_stem_and_first_heading() {
        let doc = "# Quick Check\nRuns fast checks.\n";
        let (name, desc) = parse_markdown_meta(doc, "quick-check");
        assert_eq!(name, "quick-check");
        assert_eq!(desc, "Quick Check");
    }
}
