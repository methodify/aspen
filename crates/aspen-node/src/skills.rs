//! Repo-scoped skills & commands management.
//!
//! Skills live at `.claude/skills/<name>/SKILL.md` and commands at
//! `.claude/commands/**.md` (reference §12.2). Aspen edits them on disk and
//! calls `reload_plugins` on live sessions in that repo so a saved edit
//! shows up without a restart. Every path is jailed under the repo's
//! `.claude/` — no traversal out.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SkillEntry {
    pub name: String,
    /// Repo-relative path to the editable file.
    pub rel: String,
    pub kind: String, // "skill" | "command"
    pub description: Option<String>,
}

/// Resolve and jail a repo-relative path to a real file under
/// `<repo>/.claude/`. Rejects anything that escapes.
pub fn jailed_path(repo: &Path, rel: &str) -> Result<PathBuf> {
    let claude = repo.join(".claude");
    let candidate = repo.join(rel);
    // Normalize without touching the filesystem for the non-existent case,
    // then confirm containment.
    let norm = normalize(&candidate);
    let claude_norm = normalize(&claude);
    if !norm.starts_with(&claude_norm) {
        bail!("path {rel:?} is outside this repo's .claude/ directory");
    }
    if norm.extension().and_then(|e| e.to_str()) != Some("md") {
        bail!("only .md files are editable here");
    }
    Ok(norm)
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn list(repo: &Path) -> Result<Vec<SkillEntry>> {
    let mut out = Vec::new();
    // Skills: .claude/skills/<name>/SKILL.md
    let skills_dir = repo.join(".claude/skills");
    if let Ok(entries) = std::fs::read_dir(&skills_dir) {
        for e in entries.flatten() {
            let skill_md = e.path().join("SKILL.md");
            if skill_md.is_file() {
                let name = e.file_name().to_string_lossy().into_owned();
                out.push(SkillEntry {
                    description: frontmatter_description(&skill_md),
                    rel: format!(".claude/skills/{name}/SKILL.md"),
                    kind: "skill".into(),
                    name,
                });
            }
        }
    }
    // Commands: .claude/commands/**.md
    let cmd_dir = repo.join(".claude/commands");
    collect_commands(&cmd_dir, &cmd_dir, &mut out);
    out.sort_by_key(|a| (a.kind.clone(), a.name.clone()));
    Ok(out)
}

fn collect_commands(root: &Path, dir: &Path, out: &mut Vec<SkillEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_commands(root, &p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("md") {
            let rel_from_root = p.strip_prefix(root).unwrap_or(&p).to_string_lossy();
            let name = rel_from_root.trim_end_matches(".md").replace('/', ":");
            out.push(SkillEntry {
                description: frontmatter_description(&p),
                rel: format!(".claude/commands/{}", rel_from_root),
                kind: "command".into(),
                name,
            });
        }
    }
}

fn frontmatter_description(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("description:") {
            return Some(rest.trim().trim_matches('"').to_owned());
        }
    }
    None
}

pub fn read(repo: &Path, rel: &str) -> Result<String> {
    let path = jailed_path(repo, rel)?;
    std::fs::read_to_string(&path).map_err(|e| anyhow!("reading {rel}: {e}"))
}

pub fn write(repo: &Path, rel: &str, content: &str) -> Result<()> {
    let path = jailed_path(repo, rel)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content).map_err(|e| anyhow!("writing {rel}: {e}"))
}

pub fn delete(repo: &Path, rel: &str) -> Result<()> {
    let path = jailed_path(repo, rel)?;
    std::fs::remove_file(&path).map_err(|e| anyhow!("deleting {rel}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jail_rejects_traversal() {
        let repo = Path::new("/repo");
        assert!(jailed_path(repo, ".claude/skills/x/SKILL.md").is_ok());
        assert!(jailed_path(repo, ".claude/../../etc/passwd.md").is_err());
        assert!(jailed_path(repo, "../outside.md").is_err());
        assert!(jailed_path(repo, ".claude/settings.json").is_err()); // not .md
    }

    #[test]
    fn list_and_edit_roundtrip() {
        let dir = std::env::temp_dir().join(format!("aspen-skills-{}", std::process::id()));
        let skill = dir.join(".claude/skills/greet");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: greet\ndescription: say hi\n---\nbody",
        )
        .unwrap();
        let entries = list(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "greet");
        assert_eq!(entries[0].description.as_deref(), Some("say hi"));
        write(&dir, &entries[0].rel, "changed").unwrap();
        assert_eq!(read(&dir, &entries[0].rel).unwrap(), "changed");
        std::fs::remove_dir_all(&dir).ok();
    }
}
