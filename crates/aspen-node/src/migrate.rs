//! Session migration: a session bundle, and the path rules that let a
//! transcript written on one machine resume on another
//! (PROPOSALS-2026-09 §5; the rules descend from context-convergence).
//!
//! A session is trapped on its machine twice over: the harness keys its
//! transcript directory by the absolute repo path, and absolute paths are
//! embedded throughout the transcript. Moving one is a **path rewriting
//! problem**. Canonical form replaces the source machine's anchors with
//! sentinels — `{{REPO}}`, `{{CLAUDE_PROJECT_DIR}}`, `{{ENCODED_DIR}}`,
//! `{{HOME}}`, `{{TMP}}` — longest anchor first, boundary-anchored (an
//! anchor `…/catalog` never touches `…/catalog2`), on JSON string leaves
//! and keys (never raw text of a JSONL line), with Windows anchors matched
//! in their dialects (`C:\…`, `C:/…`, `/c/…`, `/mnt/c/…`). The receiving
//! node localizes against its own anchors. Session ids and file names are
//! never rewritten.
//!
//! The bundle (a staging directory; also a tar for the file verbs):
//! `manifest.json` + files by tier — A transcript, B the session's
//! subfolder (tool results, subagent transcripts), C project memory,
//! D session-written artifacts outside the repo, E a patch of uncommitted
//! work.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ------------------------------------------------------------------ paths

/// The anchors of one machine, as strings in that machine's native form.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathCtx {
    pub os: String, // "windows" | "unix"
    pub home: String,
    pub repo: String,
    /// `<claude home>/projects/<encoded repo>`
    pub project_dir: String,
    /// The encoded repo dir name alone (`-home-me-src-repo`).
    pub encoded: String,
    pub tmp: String,
    /// `$CODEX_HOME` (HARNESSES.md §6): Codex rollouts name skill roots
    /// and their own home; absent in bundles from before v0.23.
    #[serde(default)]
    pub codex_home: String,
}

const SENTINELS: [&str; 6] = [
    "{{REPO}}",
    "{{CLAUDE_PROJECT_DIR}}",
    "{{CODEX_HOME}}",
    "{{ENCODED_DIR}}",
    "{{HOME}}",
    "{{TMP}}",
];

impl PathCtx {
    pub fn local(repo: &Path) -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        let project_dir = aspen_claude::transcript::claude_home()
            .join("projects")
            .join(aspen_claude::transcript::project_slug(repo));
        Self {
            os: if cfg!(windows) { "windows" } else { "unix" }.into(),
            home,
            repo: repo.to_string_lossy().into_owned(),
            project_dir: project_dir.to_string_lossy().into_owned(),
            encoded: aspen_claude::transcript::project_slug(repo),
            tmp: std::env::temp_dir()
                .to_string_lossy()
                .trim_end_matches(['/', '\\'])
                .to_owned(),
            codex_home: aspen_codex::codex_home().to_string_lossy().into_owned(),
        }
    }

    /// (sentinel, anchor) pairs, longest anchor first. The project dir
    /// precedes the bare encoded name and home so nesting resolves right.
    fn anchors(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![
            ("{{CLAUDE_PROJECT_DIR}}", self.project_dir.clone()),
            ("{{CODEX_HOME}}", self.codex_home.clone()),
            ("{{REPO}}", self.repo.clone()),
            ("{{TMP}}", self.tmp.clone()),
            ("{{HOME}}", self.home.clone()),
        ];
        v.retain(|(_, a)| !a.is_empty());
        v.sort_by_key(|(_, a)| std::cmp::Reverse(a.len()));
        v
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Windows dialects of one anchor: `C:\a\b`, `C:/a/b`, `/c/a/b`,
/// `/mnt/c/a/b` — both drive-letter cases. Unix anchors have one form.
fn dialects(anchor: &str, windows: bool) -> Vec<String> {
    let mut out = vec![anchor.to_owned()];
    if !windows {
        return out;
    }
    let bytes = anchor.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let tail = anchor[3..].replace('\\', "/");
        let tail_b = anchor[3..].replace('/', "\\");
        for d in [drive, drive.to_ascii_uppercase()] {
            out.push(format!("{d}:\\{tail_b}"));
            out.push(format!("{d}:/{tail}"));
            out.push(format!("/{}/{tail}", d.to_ascii_lowercase()));
            out.push(format!("/mnt/{}/{tail}", d.to_ascii_lowercase()));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Replace every boundary-anchored occurrence of `anchor` in `s` with
/// `sentinel`; the path tail after it is normalized to `/`.
fn replace_anchor(s: &str, anchor: &str, sentinel: &str) -> String {
    if anchor.is_empty() || !s.contains(anchor) {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(off) = s[i..].find(anchor) {
        let start = i + off;
        let end = start + anchor.len();
        let before_ok = start == 0 || !is_word(s[..start].chars().last().unwrap());
        let after = s[end..].chars().next();
        let after_ok = match after {
            None => true,
            Some(c) => !is_word(c),
        };
        if before_ok && after_ok {
            out.push_str(&s[i..start]);
            out.push_str(sentinel);
            // Normalize the path tail that follows.
            let mut j = end;
            let mut tail = String::new();
            for c in s[end..].chars() {
                if c == '\\'
                    || c == '/'
                    || is_word(c)
                    || c == '.'
                    || c == '~'
                    || c == '+'
                    || c == '@'
                {
                    tail.push(if c == '\\' { '/' } else { c });
                    j += c.len_utf8();
                } else {
                    break;
                }
            }
            out.push_str(&tail);
            i = j;
        } else {
            out.push_str(&s[i..end]);
            i = end;
        }
    }
    out.push_str(&s[i..]);
    out
}

/// Source form → canonical form for one string.
pub fn canonicalize_str(s: &str, ctx: &PathCtx) -> String {
    if !s.contains('/') && !s.contains('\\') && !s.contains(&ctx.encoded) {
        return s.to_owned();
    }
    let windows = ctx.os == "windows";
    let mut cur = s.to_owned();
    for (sentinel, anchor) in ctx.anchors() {
        for d in dialects(&anchor, windows) {
            cur = replace_anchor(&cur, &d, sentinel);
        }
    }
    if !ctx.encoded.is_empty() {
        cur = replace_anchor(&cur, &ctx.encoded, "{{ENCODED_DIR}}");
    }
    cur
}

/// Canonical form → this machine's form for one string.
pub fn localize_str(s: &str, ctx: &PathCtx) -> String {
    if !SENTINELS.iter().any(|k| s.contains(k)) {
        return s.to_owned();
    }
    let windows = ctx.os == "windows";
    let mut out = String::with_capacity(s.len() + 32);
    let mut rest = s;
    'outer: while !rest.is_empty() {
        for (sentinel, value) in [
            ("{{CLAUDE_PROJECT_DIR}}", ctx.project_dir.as_str()),
            ("{{CODEX_HOME}}", ctx.codex_home.as_str()),
            ("{{REPO}}", ctx.repo.as_str()),
            ("{{ENCODED_DIR}}", ctx.encoded.as_str()),
            ("{{HOME}}", ctx.home.as_str()),
            ("{{TMP}}", ctx.tmp.as_str()),
        ] {
            if let Some(r) = rest.strip_prefix(sentinel) {
                out.push_str(value);
                // The tail: native separators.
                let mut j = 0;
                for c in r.chars() {
                    if c == '/' || is_word(c) || c == '.' || c == '~' || c == '+' || c == '@' {
                        out.push(if c == '/' && windows { '\\' } else { c });
                        j += c.len_utf8();
                    } else {
                        break;
                    }
                }
                rest = &r[j..];
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn map_value(v: &Value, f: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(f(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| map_value(x, f)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (f(k), map_value(x, f))).collect()),
        other => other.clone(),
    }
}

/// JSONL: every line parsed, leaves and keys rewritten, re-serialized
/// compactly. Unparseable lines pass through untouched.
pub fn rewrite_jsonl(text: &str, f: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match serde_json::from_str::<Value>(line) {
            Ok(v) => out.push_str(
                &serde_json::to_string(&map_value(&v, f)).unwrap_or_else(|_| line.to_owned()),
            ),
            Err(_) => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Does canonical → local → canonical come back unchanged? The guard
/// convergence taught: publish nothing that does not survive the trip.
pub fn round_trip_stable(canon: &str, ctx: &PathCtx) -> bool {
    canonicalize_str(&localize_str(canon, ctx), ctx) == canon
}

// ----------------------------------------------------------------- bundle

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleFile {
    /// A | B | C | D | E
    pub tier: String,
    /// Path inside the bundle.
    pub rel: String,
    /// "jsonl" (canonicalized per line) | "text" (canonicalized) | "binary"
    pub kind: String,
    /// transcript | sidechain | memory | artifact | patch
    pub dest: String,
    /// The source path in canonical form (artifacts), for localization.
    pub canon_path: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub bundle_id: String,
    pub created_at: f64,
    pub source_node: String,
    pub source: PathCtx,
    pub harness_version: Option<String>,
    /// "move" | "copy" | "export"
    pub mode: String,
    pub agent: AgentSpec,
    pub tiers: Vec<String>,
    pub files: Vec<BundleFile>,
    pub lineage: Vec<Value>,
    pub bookmarks: Vec<Value>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Bare name (`main@hub`), without a node.
    pub name: String,
    pub channel: String,
    pub session_id: String,
    pub charter: Option<String>,
    pub title: Option<String>,
    pub extra_args: Option<String>,
    /// Basename of the repo, for finding a counterpart on the target.
    pub repo_basename: String,
    /// `git remote get-url origin`, when the repo has one.
    pub repo_origin: Option<String>,
    /// Which runtime the session runs on (HARNESSES.md); bundles from
    /// before v0.23 are Claude's.
    #[serde(default)]
    pub harness: aspen_core::Harness,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExportOpts {
    pub mode: String,
    /// Tiers to include; default A B C D.
    pub tiers: Option<Vec<String>>,
}

pub fn staging_root(data_dir: &Path) -> PathBuf {
    data_dir.join("staging")
}

pub fn git_origin(repo: &Path) -> Option<String> {
    crate::gitstate::quiet_command("git")
        .args(["-C", &repo.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn copy_tree(src: &Path, rel_prefix: &str, out: &mut Vec<(PathBuf, String)>) {
    if let Ok(rd) = std::fs::read_dir(src) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if rel_prefix.is_empty() {
                name
            } else {
                format!("{rel_prefix}/{name}")
            };
            if p.is_dir() {
                copy_tree(&p, &rel, out);
            } else {
                out.push((p, rel));
            }
        }
    }
}

fn is_texty(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()),
        Some("md" | "txt" | "json" | "toml" | "yml" | "yaml" | "csv" | "log")
    )
}

/// Build a bundle for `agent` into `<staging>/<bundle_id>/`. The caller
/// has already stopped the session when `mode == "move"`.
pub fn export(
    data_dir: &Path,
    node_name: &str,
    row: &crate::store::AgentRow,
    store: &crate::store::BusStore,
    opts: &ExportOpts,
) -> Result<(PathBuf, Manifest)> {
    let sid = row
        .session_id
        .clone()
        .ok_or_else(|| anyhow!("@{} has no session to migrate", row.name))?;
    let repo = row.repo.clone();
    let ctx = PathCtx::local(&repo);
    let tiers: Vec<String> = opts
        .tiers
        .clone()
        .unwrap_or_else(|| vec!["A".into(), "B".into(), "C".into(), "D".into()]);
    let bundle_id = uuid::Uuid::new_v4().to_string();
    let dir = staging_root(data_dir).join(&bundle_id);
    std::fs::create_dir_all(&dir)?;
    let canon = |s: &str| canonicalize_str(s, &ctx);
    let mut files: Vec<BundleFile> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    let project_dir = PathBuf::from(&ctx.project_dir);
    let mut sources: Vec<(PathBuf, String, String, String, String, Option<String>)> = Vec::new(); // (src, rel, tier, kind, dest, canon_path)

    // A: the session's files, as its harness keeps them (HARNESSES.md §2).
    // Claude: the transcript (plus the ones bookmarks point at); Codex:
    // the rollout and the rollouts its history base chains to, kept
    // under the same date paths on the target.
    let codex_store = aspen_codex::CodexStore::new();
    let is_codex = row.harness == aspen_core::Harness::Codex;
    let tpath = if is_codex {
        aspen_core::SessionStore::main_path(&codex_store, &repo, &sid)
    } else {
        aspen_claude::transcript::transcript_path(&repo, &sid)
    };
    if !tpath.is_file() {
        return Err(anyhow!("no transcript for session {sid}"));
    }
    let bookmarks = store.bookmarks(&row.name).unwrap_or_default();
    if is_codex {
        let mut listed: Vec<String> = vec![sid.clone()];
        for b in &bookmarks {
            if !listed.contains(&b.session_id) {
                listed.push(b.session_id.clone());
            }
        }
        for id in listed {
            for (rel, p) in aspen_core::SessionStore::files(&codex_store, &repo, &id) {
                if sources.iter().any(|(sp, ..)| sp == &p) {
                    continue;
                }
                sources.push((
                    p,
                    format!("rollout/{rel}"),
                    "A".into(),
                    "jsonl".into(),
                    "rollout".into(),
                    None,
                ));
            }
        }
    } else {
        sources.push((
            tpath.clone(),
            format!("transcript/{sid}.jsonl"),
            "A".into(),
            "jsonl".into(),
            "transcript".into(),
            None,
        ));
        for b in &bookmarks {
            if b.session_id != sid {
                let p = aspen_claude::transcript::transcript_path(&repo, &b.session_id);
                if p.is_file() {
                    sources.push((
                        p,
                        format!("transcript/{}.jsonl", b.session_id),
                        "A".into(),
                        "jsonl".into(),
                        "transcript".into(),
                        None,
                    ));
                }
            }
        }
    }
    // B: the session's subfolder (Claude's subagents; Codex keeps none).
    if tiers.iter().any(|t| t == "B") && !is_codex {
        let sub = project_dir.join(&sid);
        if sub.is_dir() {
            let mut v = Vec::new();
            copy_tree(&sub, "", &mut v);
            for (p, rel) in v {
                let kind = if rel.ends_with(".jsonl") {
                    "jsonl"
                } else if is_texty(&p) {
                    "text"
                } else {
                    "binary"
                };
                sources.push((
                    p,
                    format!("sidechain/{rel}"),
                    "B".into(),
                    kind.into(),
                    "sidechain".into(),
                    None,
                ));
            }
        }
    }
    // C: project memory (Codex memory is global — nothing per project).
    if tiers.iter().any(|t| t == "C") && !is_codex {
        let mem = project_dir.join("memory");
        if mem.is_dir() {
            let mut v = Vec::new();
            copy_tree(&mem, "", &mut v);
            for (p, rel) in v {
                let kind = if is_texty(&p) { "text" } else { "binary" };
                sources.push((
                    p,
                    format!("memory/{rel}"),
                    "C".into(),
                    kind.into(),
                    "memory".into(),
                    None,
                ));
            }
        }
    }
    // D: artifacts the session wrote outside the repo.
    if tiers.iter().any(|t| t == "D") {
        let repo_c = std::fs::canonicalize(&repo).unwrap_or(repo.clone());
        let mut n = 0usize;
        for a in crate::artifacts::touched_paths_for(row.harness, &tpath) {
            if a.kind != "wrote" && a.kind != "edited" {
                continue;
            }
            let p = PathBuf::from(&a.path);
            let pc = std::fs::canonicalize(&p).unwrap_or(p.clone());
            if pc.starts_with(&repo_c) || !pc.is_file() {
                continue;
            }
            let size = std::fs::metadata(&pc).map(|m| m.len()).unwrap_or(0);
            if size > 32 * 1024 * 1024 {
                notes.push(format!("artifact skipped (over 32 MB): {}", a.path));
                continue;
            }
            n += 1;
            let kind = if is_texty(&pc) { "text" } else { "binary" };
            sources.push((
                pc,
                format!("artifacts/{n}-{}", pc_name(&p)),
                "D".into(),
                kind.into(),
                "artifact".into(),
                Some(canon(&a.path)),
            ));
        }
        // Attachments dir rides as artifacts too.
        let att = crate::artifacts::attachments_dir(data_dir, &sid);
        if att.is_dir() {
            let mut v = Vec::new();
            copy_tree(&att, "", &mut v);
            for (p, rel) in v {
                sources.push((
                    p.clone(),
                    format!("attachments/{rel}"),
                    "D".into(),
                    "binary".into(),
                    "attachment".into(),
                    Some(canon(&p.to_string_lossy())),
                ));
            }
        }
    }
    // E: uncommitted work as a patch + untracked files.
    if tiers.iter().any(|t| t == "E") {
        let diff = crate::gitstate::quiet_command("git")
            .args(["-C", &repo.to_string_lossy(), "diff", "--binary", "HEAD"])
            .output();
        if let Ok(o) = diff {
            if o.status.success() && !o.stdout.is_empty() {
                let p = dir.join("repo.patch");
                std::fs::write(&p, &o.stdout)?;
                files.push(BundleFile {
                    tier: "E".into(),
                    rel: "repo.patch".into(),
                    kind: "binary".into(),
                    dest: "patch".into(),
                    canon_path: None,
                    size: o.stdout.len() as u64,
                });
            }
        }
        let untracked = crate::gitstate::quiet_command("git")
            .args([
                "-C",
                &repo.to_string_lossy(),
                "ls-files",
                "--others",
                "--exclude-standard",
            ])
            .output();
        if let Ok(o) = untracked {
            for rel in String::from_utf8_lossy(&o.stdout).lines() {
                let p = repo.join(rel);
                if p.is_file() {
                    sources.push((
                        p,
                        format!("untracked/{rel}"),
                        "E".into(),
                        "binary".into(),
                        "untracked".into(),
                        None,
                    ));
                }
            }
        }
    }

    for (src, rel, tier, kind, dest, canon_path) in sources {
        let out = dir.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let size = match kind.as_str() {
            "jsonl" => {
                let text = std::fs::read_to_string(&src)
                    .with_context(|| format!("reading {}", src.display()))?;
                let c = rewrite_jsonl(&text, &canon);
                std::fs::write(&out, &c)?;
                c.len() as u64
            }
            "text" => {
                let text = std::fs::read_to_string(&src).unwrap_or_default();
                let c = canon(&text);
                std::fs::write(&out, &c)?;
                c.len() as u64
            }
            _ => {
                std::fs::copy(&src, &out)?;
                std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0)
            }
        };
        files.push(BundleFile {
            tier,
            rel,
            kind,
            dest,
            canon_path,
            size,
        });
    }

    let lineage: Vec<Value> = store
        .lineage_of(&sid)
        .unwrap_or_default()
        .into_iter()
        .map(|(child, fork)| json!({ "child": child, "fork_message": fork }))
        .collect();
    let bookmarks_v: Vec<Value> = bookmarks
        .iter()
        .map(|b| json!({ "session_id": b.session_id, "message_uuid": b.message_uuid, "label": b.label, "reason": b.reason, "created_at": b.created_at }))
        .collect();
    let manifest = Manifest {
        version: 1,
        bundle_id: bundle_id.clone(),
        created_at: crate::store::now_epoch(),
        source_node: node_name.to_owned(),
        source: ctx.clone(),
        harness_version: if is_codex {
            aspen_codex::store::read_meta(&tpath).and_then(|m| m.cli_version)
        } else {
            harness_version_of(&tpath)
        },
        mode: if opts.mode.is_empty() {
            "export".into()
        } else {
            opts.mode.clone()
        },
        agent: AgentSpec {
            name: row.name.clone(),
            channel: row.channel.clone(),
            session_id: sid.clone(),
            charter: row.charter.clone(),
            title: row.title.clone(),
            extra_args: row.extra_args.clone(),
            repo_basename: repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            repo_origin: git_origin(&repo),
            harness: row.harness,
        },
        tiers,
        files,
        lineage,
        bookmarks: bookmarks_v,
        notes,
    };
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok((dir, manifest))
}

/// The harness version stamped on the transcript's newest line.
fn harness_version_of(transcript: &Path) -> Option<String> {
    let text = std::fs::read_to_string(transcript).ok()?;
    text.lines().rev().find_map(|l| {
        serde_json::from_str::<Value>(l)
            .ok()
            .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(str::to_owned))
    })
}

fn pc_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ImportOpts {
    /// Target repo path; default: the counterpart found by origin/basename.
    pub repo: Option<String>,
    /// Register under this bare name; default: the source's.
    pub name: Option<String>,
    /// "move" (same session id, revive in place) | "copy" (fork on revive).
    pub mode: Option<String>,
    /// Apply tier E onto the target repo.
    pub apply_patch: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub name: String,
    pub repo: String,
    pub session_id: String,
    pub files: usize,
    pub memory_conflicts: Vec<String>,
    pub notes: Vec<String>,
    /// Paths in the transcript that could not be localized (still carry
    /// a foreign anchor); advisory.
    pub residue: usize,
}

/// Find a repo on this node for a bundle: same origin, else same basename.
pub fn find_counterpart(store: &crate::store::BusStore, spec: &AgentSpec) -> Option<PathBuf> {
    let repos = store.repos().ok()?;
    if let Some(origin) = &spec.repo_origin {
        for r in &repos {
            if git_origin(&r.path).as_deref() == Some(origin.as_str()) {
                return Some(r.path.clone());
            }
        }
    }
    repos
        .iter()
        .find(|r| {
            r.path
                .file_name()
                .map(|n| n.to_string_lossy() == spec.repo_basename)
                .unwrap_or(false)
        })
        .map(|r| r.path.clone())
}

/// Install a bundle directory on this node: localize every file into its
/// place, register the agent (not live), record lineage and bookmarks.
pub fn import(
    data_dir: &Path,
    store: &crate::store::BusStore,
    dir: &Path,
    opts: &ImportOpts,
) -> Result<ImportReport> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
    let repo = match &opts.repo {
        Some(r) => PathBuf::from(r),
        None => find_counterpart(store, &manifest.agent).ok_or_else(|| {
            anyhow!(
                "no repo on this node matches {} ({}); pass one",
                manifest.agent.repo_basename,
                manifest.agent.repo_origin.as_deref().unwrap_or("no origin")
            )
        })?,
    };
    if !repo.is_dir() {
        return Err(anyhow!("target repo does not exist: {}", repo.display()));
    }
    let ctx = PathCtx::local(&repo);
    let local = |s: &str| localize_str(s, &ctx);
    let project_dir = PathBuf::from(&ctx.project_dir);
    std::fs::create_dir_all(&project_dir)?;
    let sid = manifest.agent.session_id.clone();
    let mut notes = manifest.notes.clone();
    let mut conflicts = Vec::new();
    let mut residue = 0usize;
    let mut n = 0usize;
    let foreign = &manifest.source;

    for f in &manifest.files {
        let src = dir.join(&f.rel);
        let target: Option<PathBuf> = match f.dest.as_str() {
            "transcript" => Some(project_dir.join(f.rel.trim_start_matches("transcript/"))),
            // Codex rollouts keep their date path under the target's home.
            "rollout" => Some(aspen_codex::codex_home().join(f.rel.trim_start_matches("rollout/"))),
            "sidechain" => Some(
                project_dir
                    .join(&sid)
                    .join(f.rel.trim_start_matches("sidechain/")),
            ),
            "memory" => Some(
                project_dir
                    .join("memory")
                    .join(f.rel.trim_start_matches("memory/")),
            ),
            "artifact" | "attachment" => {
                let want = f.canon_path.as_deref().map(local);
                match want {
                    Some(p)
                        if SENTINELS.iter().all(|k| !p.contains(k))
                            && !p.contains(&foreign.home) =>
                    {
                        Some(PathBuf::from(p))
                    }
                    _ => {
                        let p = data_dir
                            .join("imported")
                            .join(&manifest.bundle_id)
                            .join(&f.rel);
                        notes.push(format!(
                            "{} kept at {} (its original location is not on this node)",
                            f.rel,
                            p.display()
                        ));
                        Some(p)
                    }
                }
            }
            "patch" | "untracked" => None,
            _ => None,
        };
        let Some(target) = target else { continue };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match f.kind.as_str() {
            "jsonl" => {
                let text = std::fs::read_to_string(&src)?;
                let out = rewrite_jsonl(&text, &local);
                if !foreign.home.is_empty() {
                    residue += out.matches(&foreign.home).count();
                }
                std::fs::write(&target, out)?;
            }
            "text" => {
                let text = std::fs::read_to_string(&src).unwrap_or_default();
                let out = local(&text);
                if f.dest == "memory" && target.exists() {
                    let existing = std::fs::read_to_string(&target).unwrap_or_default();
                    if existing != out {
                        // Keep both; the operator reconciles. (3-way merge
                        // needs a common base we do not have yet.)
                        let alt = target.with_file_name(format!(
                            "{}.from-{}{}",
                            target
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            manifest.source_node,
                            target
                                .extension()
                                .map(|e| format!(".{}", e.to_string_lossy()))
                                .unwrap_or_default()
                        ));
                        std::fs::write(&alt, out)?;
                        conflicts.push(alt.to_string_lossy().into_owned());
                        continue;
                    }
                }
                std::fs::write(&target, out)?;
            }
            _ => {
                std::fs::copy(&src, &target)?;
            }
        }
        n += 1;
    }

    // Tier E: apply the patch and untracked files when asked.
    if opts.apply_patch {
        let patch = dir.join("repo.patch");
        if patch.is_file() {
            let st = crate::gitstate::quiet_command("git")
                .args([
                    "-C",
                    &repo.to_string_lossy(),
                    "apply",
                    "--3way",
                    &patch.to_string_lossy(),
                ])
                .status();
            match st {
                Ok(s) if s.success() => {
                    notes.push("applied uncommitted changes (repo.patch)".into())
                }
                _ => notes.push("repo.patch did not apply cleanly; left in the bundle".into()),
            }
        }
        for f in manifest.files.iter().filter(|f| f.dest == "untracked") {
            let rel = f.rel.trim_start_matches("untracked/");
            let target = repo.join(rel);
            if target.exists() {
                notes.push(format!(
                    "untracked {rel} exists on the target; not overwritten"
                ));
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(dir.join(&f.rel), &target)?;
        }
    }

    // Register: same bare name unless told otherwise; the channel follows
    // the target repo's handle.
    let name = opts
        .name
        .clone()
        .unwrap_or_else(|| manifest.agent.name.clone());
    let bare = name.split('@').next().unwrap_or(&name).to_owned();
    let handle = store
        .repos()
        .unwrap_or_default()
        .iter()
        .find(|r| r.path == repo)
        .map(|r| r.handle.clone())
        .unwrap_or_else(|| {
            repo.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let full = format!("{bare}@{handle}");
    store.register_agent(
        &full,
        &repo,
        &handle,
        &sid,
        manifest.agent.charter.as_deref(),
        manifest.agent.extra_args.as_deref(),
        manifest.agent.harness,
    )?;
    if let Some(t) = &manifest.agent.title {
        let _ = store.set_agent_title(&full, Some(t));
    }
    for b in &manifest.bookmarks {
        let _ = store.add_bookmark(
            &full,
            b.get("session_id").and_then(|s| s.as_str()).unwrap_or(&sid),
            b.get("message_uuid").and_then(|s| s.as_str()),
            b.get("label").and_then(|s| s.as_str()),
            b.get("reason")
                .and_then(|s| s.as_str())
                .unwrap_or("migrated"),
        );
    }
    for l in &manifest.lineage {
        if let Some(child) = l.get("child").and_then(|c| c.as_str()) {
            let _ = store.record_lineage(
                &full,
                child,
                &sid,
                l.get("fork_message").and_then(|m| m.as_str()),
            );
        }
    }
    let _ = store.record_event(
        &full,
        "migrated",
        json!({ "from": manifest.source_node, "mode": opts.mode.clone().unwrap_or_else(|| manifest.mode.clone()), "files": n }),
    );
    Ok(ImportReport {
        name: full,
        repo: repo.to_string_lossy().into_owned(),
        session_id: sid,
        files: n,
        memory_conflicts: conflicts,
        notes,
        residue,
    })
}

// ------------------------------------------------------------- tar verbs

/// Pack a bundle dir into a `.aspen-session` tar (system tar; present on
/// Linux, macOS, and Windows 10+).
pub fn pack(dir: &Path, out: &Path) -> Result<()> {
    let st = crate::gitstate::quiet_command("tar")
        .args([
            "-cf",
            &out.to_string_lossy(),
            "-C",
            &dir.to_string_lossy(),
            ".",
        ])
        .status()?;
    if !st.success() {
        return Err(anyhow!("tar failed packing {}", dir.display()));
    }
    Ok(())
}

pub fn unpack(file: &Path, into: &Path) -> Result<()> {
    std::fs::create_dir_all(into)?;
    let st = crate::gitstate::quiet_command("tar")
        .args([
            "-xf",
            &file.to_string_lossy(),
            "-C",
            &into.to_string_lossy(),
        ])
        .status()?;
    if !st.success() {
        return Err(anyhow!("tar failed unpacking {}", file.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix() -> PathCtx {
        PathCtx {
            os: "unix".into(),
            home: "/home/me".into(),
            repo: "/home/me/src/catalog".into(),
            project_dir: "/home/me/.claude/projects/-home-me-src-catalog".into(),
            encoded: "-home-me-src-catalog".into(),
            tmp: "/tmp".into(),
            codex_home: String::new(),
        }
    }
    fn mac() -> PathCtx {
        PathCtx {
            os: "unix".into(),
            home: "/Users/me".into(),
            repo: "/Users/me/src/catalog".into(),
            project_dir: "/Users/me/.claude/projects/-Users-me-src-catalog".into(),
            encoded: "-Users-me-src-catalog".into(),
            tmp: "/var/folders/xx/T".into(),
            codex_home: String::new(),
        }
    }
    fn win() -> PathCtx {
        PathCtx {
            os: "windows".into(),
            home: "C:\\Users\\me".into(),
            repo: "C:\\Users\\me\\src\\catalog".into(),
            project_dir: "C:\\Users\\me\\.claude\\projects\\C--Users-me-src-catalog".into(),
            encoded: "C--Users-me-src-catalog".into(),
            tmp: "C:\\Users\\me\\AppData\\Local\\Temp".into(),
            codex_home: String::new(),
        }
    }

    #[test]
    fn repo_and_boundaries() {
        let c = unix();
        assert_eq!(
            canonicalize_str("/home/me/src/catalog/src/main.rs", &c),
            "{{REPO}}/src/main.rs"
        );
        // A sibling that shares the prefix is not the repo.
        assert_eq!(
            canonicalize_str("/home/me/src/catalog2/x", &c),
            "{{HOME}}/src/catalog2/x"
        );
        assert_eq!(
            canonicalize_str("/home/me/src/catalog-backup/x", &c),
            "{{HOME}}/src/catalog-backup/x"
        );
        // Trailing punctuation is outside the path.
        assert_eq!(
            canonicalize_str("see /home/me/src/catalog.", &c),
            "see {{REPO}}."
        );
    }

    #[test]
    fn project_dir_and_encoded() {
        let c = unix();
        assert_eq!(
            canonicalize_str(
                "/home/me/.claude/projects/-home-me-src-catalog/abc.jsonl",
                &c
            ),
            "{{CLAUDE_PROJECT_DIR}}/abc.jsonl"
        );
        assert_eq!(
            canonicalize_str("dir -home-me-src-catalog here", &c),
            "dir {{ENCODED_DIR}} here"
        );
    }

    #[test]
    fn localize_to_mac_and_back() {
        let u = unix();
        let m = mac();
        let canon = canonicalize_str("/home/me/src/catalog/README.md and /tmp/x.png", &u);
        assert_eq!(canon, "{{REPO}}/README.md and {{TMP}}/x.png");
        let on_mac = localize_str(&canon, &m);
        assert_eq!(
            on_mac,
            "/Users/me/src/catalog/README.md and /var/folders/xx/T/x.png"
        );
        assert!(round_trip_stable(&canon, &m));
    }

    #[test]
    fn windows_dialects() {
        let w = win();
        for s in [
            "C:\\Users\\me\\src\\catalog\\a\\b.rs",
            "C:/Users/me/src/catalog/a/b.rs",
            "c:/users/me/src/catalog/a/b.rs"
                .replace("users", "Users")
                .as_str(),
            "/c/Users/me/src/catalog/a/b.rs",
            "/mnt/c/Users/me/src/catalog/a/b.rs",
        ] {
            assert_eq!(canonicalize_str(s, &w), "{{REPO}}/a/b.rs", "{s}");
        }
        let local = localize_str("{{REPO}}/a/b.rs", &w);
        assert_eq!(local, "C:\\Users\\me\\src\\catalog\\a\\b.rs");
    }

    #[test]
    fn jsonl_leaves_and_keys() {
        let u = unix();
        let line = r#"{"cwd":"/home/me/src/catalog","message":{"content":[{"type":"text","text":"wrote /home/me/src/catalog/x.md"}]},"n":1}"#;
        let out = rewrite_jsonl(line, &|s| canonicalize_str(s, &u));
        assert!(out.contains(r#""cwd":"{{REPO}}""#));
        assert!(out.contains("wrote {{REPO}}/x.md"));
        assert!(out.contains(r#""n":1"#));
        let back = rewrite_jsonl(&out, &|s| localize_str(s, &mac()));
        assert!(back.contains(r#""cwd":"/Users/me/src/catalog""#));
    }
}
