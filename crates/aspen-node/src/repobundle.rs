//! Repo bundles (PROPOSALS-2026-09-O §1; reference docs/BUNDLES.md): a
//! repo *with its context* as one file — the working tree (per a chosen
//! mode), every session of the repo in canonical form (the path rules of
//! `migrate.rs`), project memory, and the repo's Aspen names — so it can
//! cross any gap (two meshes, an air gap, a colleague) and be imported as
//! a new repo or on top of one that exists.
//!
//! The file is a gzip'd tar, optionally sealed with a passphrase:
//!
//! ```text
//! manifest.json            first entry: what was chosen, every file planned
//! repo/…                   the working tree (not path-rewritten: it is code)
//! context/claude/…         transcripts, sidecar folders, memory/ — canonical
//! context/codex/…          rollouts whose cwd is the repo — canonical
//! aspen/agents.json        names, charters, titles, bookmarks, lineage
//! sums.json                last entry: size + sha256 of every file
//! ```
//!
//! Sealed form: `ASPENREPO-ENC1\n`, a 16-byte salt, scrypt parameters, a
//! 16-byte nonce prefix, then chunks of the plain stream — each
//! `u32 length ‖ XChaCha20-Poly1305(chunk)` under nonce `prefix ‖ u64
//! counter` with the final chunk's associated data marked, so truncation
//! is detected.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest as _;

use crate::migrate::{canonicalize_str, git_origin, localize_str, rewrite_jsonl, PathCtx};

pub const EXT: &str = "aspen-repo";

// ----------------------------------------------------------------- options

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RepoMode {
    /// `git ls-files` plus `.git` (history and branches travel).
    #[default]
    Tracked,
    /// Tracked plus untracked files, honoring `.gitignore`.
    Untracked,
    /// The whole directory, dotfiles and ignored files included.
    All,
    /// Context only.
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOpts {
    #[serde(default)]
    pub repo_mode: RepoMode,
    /// Session ids to include; None = every session of the repo.
    #[serde(default)]
    pub sessions: Option<Vec<String>>,
    /// Sidecar folders (tool results, subagent and workflow transcripts).
    #[serde(default = "yes")]
    pub sidecars: bool,
    #[serde(default = "yes")]
    pub memory: bool,
    /// Aspen names, charters, titles, bookmarks, lineage.
    #[serde(default = "yes")]
    pub names: bool,
    /// Seal the file with this passphrase.
    #[serde(default, skip_serializing)]
    pub passphrase: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for ExportOpts {
    fn default() -> Self {
        Self {
            repo_mode: RepoMode::Tracked,
            sessions: None,
            sidecars: true,
            memory: true,
            names: true,
            passphrase: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Planned {
    pub rel: String,
    /// repo | transcript | sidecar | memory | rollout | aspen
    pub part: String,
    /// jsonl (canonicalized per line) | text (canonicalized) | binary | symlink
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: String,
    pub harness: aspen_core::Harness,
    pub title: Option<String>,
    /// The Aspen name on this transcript, if any.
    pub name: Option<String>,
    pub bytes: u64,
    pub sidecar_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    pub basename: String,
    pub handle: Option<String>,
    pub origin: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub is_git: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub kind: String,
    pub bundle_id: String,
    pub created_at: f64,
    pub source_node: String,
    pub source: PathCtx,
    pub repo: RepoInfo,
    pub options: ExportOpts,
    pub sealed: bool,
    pub sessions: Vec<SessionEntry>,
    pub files: Vec<Planned>,
    pub notes: Vec<String>,
    pub aspen_version: String,
}

// ------------------------------------------------------------------ sources

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    crate::gitstate::quiet_command("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn is_git(repo: &Path) -> bool {
    repo.join(".git").exists()
}

fn walk(root: &Path, rel: &str, out: &mut Vec<String>) {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut ents: Vec<_> = rd.flatten().collect();
    ents.sort_by_key(|e| e.file_name());
    for e in ents {
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(root, &r, out),
            Ok(_) => out.push(r),
            Err(_) => {}
        }
    }
}

/// The repo's files for a mode, relative, `/`-separated.
fn repo_files(repo: &Path, mode: RepoMode, notes: &mut Vec<String>) -> Vec<String> {
    let git_ok = is_git(repo);
    let mut files: Vec<String> = Vec::new();
    match mode {
        RepoMode::None => return files,
        RepoMode::All => {
            walk(repo, "", &mut files);
            return files;
        }
        RepoMode::Tracked | RepoMode::Untracked if !git_ok => {
            notes.push("not a git repository: every file is included".into());
            walk(repo, "", &mut files);
            return files;
        }
        _ => {}
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |s: &str, files: &mut Vec<String>| {
        if !s.is_empty() && seen.insert(s.to_owned()) {
            files.push(s.to_owned());
        }
    };
    for f in git(repo, &["ls-files", "-z"])
        .unwrap_or_default()
        .split('\0')
    {
        push(f, &mut files);
    }
    if mode == RepoMode::Untracked {
        for f in git(repo, &["ls-files", "-z", "--others", "--exclude-standard"])
            .unwrap_or_default()
            .split('\0')
        {
            push(f, &mut files);
        }
    }
    // History and branches travel with the tree.
    let mut g = Vec::new();
    walk(repo, ".git", &mut g);
    for f in g {
        push(&f, &mut files);
    }
    // A tracked path deleted in the working tree is not there to copy.
    files.retain(|f| std::fs::symlink_metadata(repo.join(f)).is_ok());
    files
}

fn texty(p: &str) -> bool {
    matches!(
        Path::new(p).extension().and_then(|e| e.to_str()),
        Some("md" | "txt" | "json" | "toml" | "yml" | "yaml" | "csv" | "log")
    )
}

fn file_size(p: &Path) -> u64 {
    std::fs::symlink_metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn dir_size(p: &Path) -> u64 {
    let mut v = Vec::new();
    walk(p, "", &mut v);
    v.iter().map(|r| file_size(&p.join(r))).sum()
}

/// Every session of the repo, both harnesses, with its Aspen name.
fn sessions_of(store: &crate::store::BusStore, repo: &Path) -> Vec<SessionEntry> {
    let names: HashMap<String, String> = store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| {
            a.moved_to.is_none()
                && !a.fork_pending
                && crate::node::normalize_repo(&a.repo) == crate::node::normalize_repo(repo)
        })
        .filter_map(|a| a.session_id.clone().map(|s| (s, a.name)))
        .collect();
    let mut out = Vec::new();
    let project_dir = PathBuf::from(PathCtx::local(repo).project_dir);
    let claude = aspen_claude::ClaudeStore;
    for s in aspen_core::SessionStore::enumerate(&claude, repo).unwrap_or_default() {
        let main = project_dir.join(format!("{}.jsonl", s.session_id));
        out.push(SessionEntry {
            name: names.get(&s.session_id).cloned(),
            bytes: file_size(&main),
            sidecar_bytes: dir_size(&project_dir.join(&s.session_id)),
            harness: aspen_core::Harness::Claude,
            title: s.title.clone(),
            id: s.session_id,
        });
    }
    let codex = aspen_codex::CodexStore::new();
    for s in aspen_core::SessionStore::enumerate(&codex, repo).unwrap_or_default() {
        let bytes: u64 = aspen_core::SessionStore::files(&codex, repo, &s.session_id)
            .iter()
            .map(|(_, p)| file_size(p))
            .sum();
        out.push(SessionEntry {
            name: names.get(&s.session_id).cloned(),
            bytes,
            sidecar_bytes: 0,
            harness: aspen_core::Harness::Codex,
            title: s.title.clone(),
            id: s.session_id,
        });
    }
    out
}

/// What an export would carry, with sizes, before anything is written.
pub fn export_preflight(store: &crate::store::BusStore, repo: &Path) -> Result<Value> {
    let repo = crate::node::normalize_repo(repo);
    if !repo.is_dir() {
        bail!("not a directory: {}", repo.display());
    }
    let mut notes = Vec::new();
    let mut modes = serde_json::Map::new();
    for (key, m) in [
        ("tracked", RepoMode::Tracked),
        ("untracked", RepoMode::Untracked),
        ("all", RepoMode::All),
    ] {
        let files = repo_files(&repo, m, &mut notes);
        let bytes: u64 = files.iter().map(|f| file_size(&repo.join(f))).sum();
        modes.insert(key.into(), json!({ "files": files.len(), "bytes": bytes }));
    }
    // The biggest top-level directories, so `node_modules` is a choice.
    let mut tops: Vec<(String, u64)> = std::fs::read_dir(&repo)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    let b = dir_size(&e.path());
                    (n, b)
                })
                .collect()
        })
        .unwrap_or_default();
    tops.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    tops.truncate(8);
    let sessions = sessions_of(store, &repo);
    let project_dir = PathBuf::from(PathCtx::local(&repo).project_dir);
    notes.dedup();
    Ok(json!({
        "repo": repo.to_string_lossy(),
        "is_git": is_git(&repo),
        "origin": git_origin(&repo),
        "modes": modes,
        "largest_dirs": tops.iter().map(|(n, b)| json!({ "name": n, "bytes": b })).collect::<Vec<_>>(),
        "sessions": sessions,
        "memory_bytes": dir_size(&project_dir.join("memory")),
        "names": store.agents().unwrap_or_default().iter().filter(|a| a.moved_to.is_none() && !a.fork_pending && crate::node::normalize_repo(&a.repo) == repo).count(),
        "notes": notes,
    }))
}

// --------------------------------------------------------------- sealing

const MAGIC: &[u8] = b"ASPENREPO-ENC1\n";
const CHUNK: usize = 1 << 20;
const SCRYPT_LOG_N: u8 = 15;

fn derive_key(pass: &str, salt: &[u8], log_n: u8, r: u32, p: u32) -> Result<[u8; 32]> {
    let params = scrypt::Params::new(log_n, r, p).map_err(|e| anyhow!("scrypt params: {e}"))?;
    let mut key = [0u8; 32];
    scrypt::scrypt(pass.as_bytes(), salt, &params, &mut key).map_err(|e| anyhow!("scrypt: {e}"))?;
    Ok(key)
}

fn nonce_for(prefix: &[u8; 16], counter: u64) -> chacha20poly1305::XNonce {
    let mut n = [0u8; 24];
    n[..16].copy_from_slice(prefix);
    n[16..].copy_from_slice(&counter.to_be_bytes());
    chacha20poly1305::XNonce::from(n)
}

/// A writer that seals what passes through it in 1 MiB chunks.
pub struct SealWriter<W: Write> {
    inner: W,
    cipher: chacha20poly1305::XChaCha20Poly1305,
    prefix: [u8; 16],
    counter: u64,
    buf: Vec<u8>,
}

impl<W: Write> SealWriter<W> {
    pub fn new(mut inner: W, pass: &str) -> Result<Self> {
        use chacha20poly1305::KeyInit;
        use rand_core::RngCore;
        let mut salt = [0u8; 16];
        let mut prefix = [0u8; 16];
        rand_core::OsRng.fill_bytes(&mut salt);
        rand_core::OsRng.fill_bytes(&mut prefix);
        let (r, p) = (8u32, 1u32);
        let key = derive_key(pass, &salt, SCRYPT_LOG_N, r, p)?;
        inner.write_all(MAGIC)?;
        inner.write_all(&salt)?;
        inner.write_all(&[SCRYPT_LOG_N])?;
        inner.write_all(&r.to_be_bytes())?;
        inner.write_all(&p.to_be_bytes())?;
        inner.write_all(&prefix)?;
        Ok(Self {
            inner,
            cipher: chacha20poly1305::XChaCha20Poly1305::new((&key).into()),
            prefix,
            counter: 0,
            buf: Vec::with_capacity(CHUNK),
        })
    }

    fn seal(&mut self, last: bool) -> std::io::Result<()> {
        use chacha20poly1305::aead::{Aead, Payload};
        let nonce = nonce_for(&self.prefix, self.counter);
        self.counter += 1;
        let ct = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &self.buf,
                    aad: &[last as u8],
                },
            )
            .map_err(|_| std::io::Error::other("seal failed"))?;
        self.inner.write_all(&(ct.len() as u32).to_be_bytes())?;
        self.inner.write_all(&ct)?;
        self.buf.clear();
        Ok(())
    }

    /// Seal the final chunk (marked, so a truncated file is detected).
    pub fn finish(mut self) -> std::io::Result<W> {
        self.seal(true)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for SealWriter<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut n = 0;
        while n < data.len() {
            let room = CHUNK - self.buf.len();
            let take = room.min(data.len() - n);
            self.buf.extend_from_slice(&data[n..n + take]);
            n += take;
            if self.buf.len() == CHUNK {
                self.seal(false)?;
            }
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The reader for a sealed file.
pub struct OpenReader<R: Read> {
    inner: R,
    cipher: chacha20poly1305::XChaCha20Poly1305,
    prefix: [u8; 16],
    counter: u64,
    plain: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> OpenReader<R> {
    fn next_chunk(&mut self) -> std::io::Result<()> {
        use chacha20poly1305::aead::{Aead, Payload};
        let mut len = [0u8; 4];
        match self.inner.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "sealed bundle ends early (truncated)",
                ))
            }
            Err(e) => return Err(e),
        }
        // A chunk is at most CHUNK bytes plus the 16-byte tag; a length
        // past that is damage or a crafted file, never an allocation.
        let n = u32::from_be_bytes(len) as usize;
        if n > CHUNK + 16 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "sealed bundle chunk is larger than any this version writes",
            ));
        }
        let mut ct = vec![0u8; n];
        self.inner.read_exact(&mut ct)?;
        let nonce = nonce_for(&self.prefix, self.counter);
        self.counter += 1;
        let open = |last: u8| {
            self.cipher.decrypt(
                &nonce,
                Payload {
                    msg: &ct,
                    aad: &[last],
                },
            )
        };
        if let Ok(p) = open(0) {
            self.plain = p;
        } else if let Ok(p) = open(1) {
            self.plain = p;
            self.done = true;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "wrong passphrase, or the bundle is damaged",
            ));
        }
        self.pos = 0;
        Ok(())
    }
}

impl<R: Read> Read for OpenReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.pos < self.plain.len() {
                let n = (self.plain.len() - self.pos).min(out.len());
                out[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }
            if self.done {
                return Ok(0);
            }
            self.next_chunk()?;
        }
    }
}

/// Is this file sealed?
pub fn is_sealed(file: &Path) -> Result<bool> {
    let mut f = std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?;
    let mut head = [0u8; 15];
    let n = f.read(&mut head)?;
    Ok(n == MAGIC.len() && head == MAGIC)
}

/// The bundle's plain tar stream (gunzipped, unsealed when it is sealed).
fn open_stream(file: &Path, pass: Option<&str>) -> Result<Box<dyn Read>> {
    use chacha20poly1305::KeyInit;
    let mut f = std::io::BufReader::new(std::fs::File::open(file)?);
    let mut head = [0u8; 15];
    f.read_exact(&mut head)
        .context("reading the bundle header")?;
    let inner: Box<dyn Read> = if head == MAGIC {
        let pass = pass.ok_or_else(|| anyhow!("this bundle is sealed: a passphrase is needed"))?;
        let mut salt = [0u8; 16];
        f.read_exact(&mut salt)?;
        let mut ln = [0u8; 1];
        f.read_exact(&mut ln)?;
        let mut r = [0u8; 4];
        f.read_exact(&mut r)?;
        let mut p = [0u8; 4];
        f.read_exact(&mut p)?;
        let mut prefix = [0u8; 16];
        f.read_exact(&mut prefix)?;
        // The file names its own key-derivation cost; a crafted header
        // asked for terabytes and aborted the daemon. Only what this
        // writer produces is accepted (the 2026-10 quality pass).
        let (r, p) = (u32::from_be_bytes(r), u32::from_be_bytes(p));
        if ln[0] != SCRYPT_LOG_N || r != 8 || p != 1 {
            bail!("the sealed bundle's header asks for key-derivation parameters this version does not write");
        }
        let key = derive_key(pass, &salt, ln[0], r, p)?;
        Box::new(OpenReader {
            inner: f,
            cipher: chacha20poly1305::XChaCha20Poly1305::new((&key).into()),
            prefix,
            counter: 0,
            plain: Vec::new(),
            pos: 0,
            done: false,
        })
    } else if head[0] == 0x1f && head[1] == 0x8b {
        // Plain gzip: put the header bytes back in front.
        Box::new(std::io::Cursor::new(head.to_vec()).chain(f))
    } else {
        bail!("not an Aspen repo bundle");
    };
    Ok(Box::new(flate2::read::GzDecoder::new(inner)))
}

// ------------------------------------------------------------------ export

#[derive(Debug, Clone, Serialize)]
pub struct ExportSummary {
    pub out: String,
    pub bytes: u64,
    pub files: usize,
    pub sessions: usize,
    pub sealed: bool,
    pub notes: Vec<String>,
}

enum Sink {
    Plain(flate2::write::GzEncoder<std::io::BufWriter<std::fs::File>>),
    Sealed(flate2::write::GzEncoder<SealWriter<std::io::BufWriter<std::fs::File>>>),
}

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Sink::Plain(w) => w.write(b),
            Sink::Sealed(w) => w.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Sink::Plain(w) => w.flush(),
            Sink::Sealed(w) => w.flush(),
        }
    }
}

fn add_bytes<W: Write>(tar: &mut tar::Builder<W>, rel: &str, data: &[u8], mode: u32) -> Result<()> {
    add_bytes_at(tar, rel, data, mode, None)
}

/// As `add_bytes`, carrying a source file's modification time: an
/// imported transcript keeps its age (the one-writer gate reads a freshly
/// written transcript as live elsewhere; the Mesh list sorts by it).
fn add_bytes_at<W: Write>(
    tar: &mut tar::Builder<W>,
    rel: &str,
    data: &[u8],
    mode: u32,
    mtime: Option<u64>,
) -> Result<()> {
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(mode);
    h.set_mtime(mtime.unwrap_or(crate::store::now_epoch() as u64));
    h.set_cksum();
    tar.append_data(&mut h, rel, data)
        .with_context(|| format!("adding {rel}"))?;
    Ok(())
}

/// Write a bundle of `repo` to `out`.
pub fn export(
    store: &crate::store::BusStore,
    node_name: &str,
    repo: &Path,
    opts: &ExportOpts,
    out: &Path,
) -> Result<ExportSummary> {
    let repo = crate::node::normalize_repo(repo);
    if !repo.is_dir() {
        bail!("not a directory: {}", repo.display());
    }
    let ctx = PathCtx::local(&repo);
    let project_dir = PathBuf::from(&ctx.project_dir);
    let mut notes: Vec<String> = Vec::new();
    let canon = |s: &str| canonicalize_str(s, &ctx);

    // Sessions chosen.
    let all_sessions = sessions_of(store, &repo);
    let sessions: Vec<SessionEntry> = match &opts.sessions {
        Some(ids) => all_sessions
            .into_iter()
            .filter(|s| ids.iter().any(|i| i == &s.id))
            .collect(),
        None => all_sessions,
    };
    let chosen: HashSet<String> = sessions.iter().map(|s| s.id.clone()).collect();

    // Plan: (rel in bundle, source path, part, kind).
    let mut plan: Vec<(String, PathBuf, String, String)> = Vec::new();
    for f in repo_files(&repo, opts.repo_mode, &mut notes) {
        let p = repo.join(&f);
        let kind = match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => "symlink",
            _ => "binary",
        };
        plan.push((format!("repo/{f}"), p, "repo".into(), kind.into()));
    }
    for s in sessions
        .iter()
        .filter(|s| s.harness == aspen_core::Harness::Claude)
    {
        let t = project_dir.join(format!("{}.jsonl", s.id));
        if t.is_file() {
            plan.push((
                format!("context/claude/{}.jsonl", s.id),
                t,
                "transcript".into(),
                "jsonl".into(),
            ));
        }
        if opts.sidecars {
            let side = project_dir.join(&s.id);
            let mut v = Vec::new();
            walk(&side, "", &mut v);
            for r in v {
                let kind = if r.ends_with(".jsonl") {
                    "jsonl"
                } else if texty(&r) {
                    "text"
                } else {
                    "binary"
                };
                plan.push((
                    format!("context/claude/{}/{r}", s.id),
                    side.join(&r),
                    "sidecar".into(),
                    kind.into(),
                ));
            }
        }
    }
    if opts.memory {
        let mem = project_dir.join("memory");
        let mut v = Vec::new();
        walk(&mem, "", &mut v);
        for r in v {
            let kind = if texty(&r) { "text" } else { "binary" };
            plan.push((
                format!("context/claude/memory/{r}"),
                mem.join(&r),
                "memory".into(),
                kind.into(),
            ));
        }
    }
    let codex = aspen_codex::CodexStore::new();
    for s in sessions
        .iter()
        .filter(|s| s.harness == aspen_core::Harness::Codex)
    {
        for (rel, p) in aspen_core::SessionStore::files(&codex, &repo, &s.id) {
            if plan.iter().any(|(_, sp, ..)| sp == &p) {
                continue;
            }
            plan.push((
                format!("context/codex/{rel}"),
                p,
                "rollout".into(),
                "jsonl".into(),
            ));
        }
    }

    // Names: the repo's agent rows on the chosen transcripts, with their
    // bookmarks and lineage.
    let agents_json = if opts.names {
        let rows: Vec<Value> = store
            .agents()
            .unwrap_or_default()
            .into_iter()
            // One name per transcript (v0.36): a pending fork still points
            // at its parent's transcript and is not a name for it.
            .filter(|a| {
                a.moved_to.is_none()
                    && !a.fork_pending
                    && crate::node::normalize_repo(&a.repo) == repo
            })
            .filter(|a| a.session_id.as_ref().is_some_and(|s| chosen.contains(s)))
            .map(|a| {
                let sid = a.session_id.clone().unwrap_or_default();
                json!({
                    "name": a.name,
                    "session_id": sid,
                    "charter": a.charter.as_deref().map(canon),
                    "title": a.title,
                    "extra_args": a.extra_args,
                    "harness": a.harness,
                    "bookmarks": store.bookmarks(&a.name).unwrap_or_default().iter()
                        .filter(|b| chosen.contains(&b.session_id))
                        .map(|b| json!({ "session_id": b.session_id, "message_uuid": b.message_uuid, "label": b.label, "reason": b.reason }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        let lineage: Vec<Value> = chosen
            .iter()
            .filter_map(|c| {
                store
                    .lineage_of(c)
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .filter(|(p, _)| chosen.contains(p))
                    .map(|(p, at)| json!({ "child": c, "parent": p, "fork_message": at }))
            })
            .collect();
        Some(json!({ "agents": rows, "lineage": lineage }))
    } else {
        None
    };

    let manifest = Manifest {
        version: 1,
        kind: "aspen-repo".into(),
        bundle_id: uuid::Uuid::new_v4().to_string(),
        created_at: crate::store::now_epoch(),
        source_node: node_name.to_owned(),
        source: ctx.clone(),
        repo: RepoInfo {
            basename: repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            handle: store
                .repos()
                .unwrap_or_default()
                .into_iter()
                .find(|r| crate::node::normalize_repo(&r.path) == repo)
                .map(|r| r.handle),
            origin: git_origin(&repo),
            branch: git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_owned()),
            head: git(&repo, &["rev-parse", "HEAD"]).map(|s| s.trim().to_owned()),
            is_git: is_git(&repo),
        },
        options: opts.clone(),
        sealed: opts.passphrase.as_deref().is_some_and(|p| !p.is_empty()),
        sessions: sessions.clone(),
        files: plan
            .iter()
            .map(|(rel, _, part, kind)| Planned {
                rel: rel.clone(),
                part: part.clone(),
                kind: kind.clone(),
            })
            .chain(agents_json.as_ref().map(|_| Planned {
                rel: "aspen/agents.json".into(),
                part: "aspen".into(),
                kind: "text".into(),
            }))
            .collect(),
        notes: notes.clone(),
        aspen_version: env!("CARGO_PKG_VERSION").into(),
    };

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let tmp = out.with_extension("part");
    let file = std::io::BufWriter::new(
        std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?,
    );
    let level = flate2::Compression::new(6);
    let sink = match opts.passphrase.as_deref().filter(|p| !p.is_empty()) {
        Some(pass) => Sink::Sealed(flate2::write::GzEncoder::new(
            SealWriter::new(file, pass)?,
            level,
        )),
        None => Sink::Plain(flate2::write::GzEncoder::new(file, level)),
    };
    let mut tar = tar::Builder::new(sink);
    tar.follow_symlinks(false);
    add_bytes(
        &mut tar,
        "manifest.json",
        &serde_json::to_vec_pretty(&manifest)?,
        0o644,
    )?;

    let mut sums: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut count = 0usize;
    for (rel, src, _part, kind) in &plan {
        match kind.as_str() {
            "symlink" => {
                let target = std::fs::read_link(src).unwrap_or_default();
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_size(0);
                h.set_mode(0o777);
                tar.append_link(&mut h, rel, &target)
                    .with_context(|| format!("adding link {rel}"))?;
            }
            "jsonl" | "text" => {
                let text = match std::fs::read_to_string(src) {
                    Ok(t) => t,
                    Err(_) => {
                        notes.push(format!("skipped unreadable {}", src.display()));
                        continue;
                    }
                };
                let c = if kind == "jsonl" {
                    rewrite_jsonl(&text, &canon)
                } else {
                    canon(&text)
                };
                sums.insert(
                    rel.clone(),
                    json!({ "size": c.len(), "sha256": hex(&sha2::Sha256::digest(c.as_bytes())) }),
                );
                let mt = std::fs::metadata(src)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());
                add_bytes_at(&mut tar, rel, c.as_bytes(), 0o644, mt)?;
            }
            _ => {
                let Ok(mut f) = std::fs::File::open(src) else {
                    notes.push(format!("skipped unreadable {}", src.display()));
                    continue;
                };
                let meta = f.metadata()?;
                let mut h = tar::Header::new_gnu();
                h.set_metadata(&meta);
                h.set_cksum();
                // Hash while the tar copies: one read of the file.
                let mut hasher = HashingReader {
                    inner: &mut f,
                    h: sha2::Sha256::new(),
                    n: 0,
                };
                tar.append_data(&mut h, rel, &mut hasher)
                    .with_context(|| format!("adding {rel}"))?;
                sums.insert(
                    rel.clone(),
                    json!({ "size": hasher.n, "sha256": hex(&hasher.h.finalize()) }),
                );
            }
        }
        count += 1;
    }
    if let Some(a) = &agents_json {
        let b = serde_json::to_vec_pretty(a)?;
        sums.insert(
            "aspen/agents.json".into(),
            json!({ "size": b.len(), "sha256": hex(&sha2::Sha256::digest(&b)) }),
        );
        add_bytes(&mut tar, "aspen/agents.json", &b, 0o644)?;
    }
    add_bytes(
        &mut tar,
        "sums.json",
        &serde_json::to_vec(&json!({ "files": sums, "notes": notes }))?,
        0o644,
    )?;
    let sink = tar.into_inner()?;
    match sink {
        Sink::Plain(gz) => {
            gz.finish()?.flush()?;
        }
        Sink::Sealed(gz) => {
            gz.finish()?.finish()?.flush()?;
        }
    }
    std::fs::rename(&tmp, out).with_context(|| format!("writing {}", out.display()))?;
    Ok(ExportSummary {
        out: out.to_string_lossy().into_owned(),
        bytes: file_size(out),
        files: count,
        sessions: sessions.len(),
        sealed: manifest.sealed,
        notes,
    })
}

struct HashingReader<'a, R: Read> {
    inner: &'a mut R,
    h: sha2::Sha256,
    n: u64,
}

impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(b)?;
        self.h.update(&b[..n]);
        self.n += n as u64;
        Ok(n)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ------------------------------------------------------------------ import

/// The manifest alone (the tar's first entry): cheap, for a first look.
pub fn read_manifest(file: &Path, pass: Option<&str>) -> Result<Manifest> {
    let mut ar = tar::Archive::new(open_stream(file, pass)?);
    let mut entries = ar.entries()?;
    let mut e = entries.next().ok_or_else(|| anyhow!("empty bundle"))??;
    if e.path()?.to_string_lossy() != "manifest.json" {
        bail!("not an Aspen repo bundle (no manifest first)");
    }
    let mut b = Vec::new();
    e.read_to_end(&mut b)?;
    let m: Manifest = serde_json::from_slice(&b).context("reading the manifest")?;
    if m.kind != "aspen-repo" {
        bail!("not a repo bundle ({})", m.kind);
    }
    Ok(m)
}

pub fn staging_dir(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("staging").join(format!("repo-{id}"))
}

/// Unpack a bundle into staging, verifying every checksum. Returns the
/// staging id and the manifest.
/// The prefixes install() cuts off a file's `rel` before joining the rest
/// onto a destination directory.
const REL_PREFIXES: [&str; 3] = ["context/claude/memory/", "context/codex/", "repo/"];

/// Refuse a manifest that names anything but plain relative paths and
/// plain ids, or a file the bundle does not hold. install() joins these
/// onto real directories: a `rel` of `context/claude/memory//home/u/x`
/// trimmed to `/home/u/x` replaced the base and wrote anywhere (the
/// 2026-10 quality pass).
fn check_manifest_paths<'a>(
    rels: impl Iterator<Item = &'a str>,
    session_ids: impl Iterator<Item = &'a str>,
    dir: &Path,
) -> Result<()> {
    for rel in rels {
        aspen_core::paths::safe_rel(rel).map_err(|e| anyhow!("the bundle's manifest: {e}"))?;
        for p in REL_PREFIXES {
            if let Some(rest) = rel.strip_prefix(p) {
                aspen_core::paths::safe_rel(rest)
                    .map_err(|e| anyhow!("the bundle's manifest: {e}"))?;
            }
        }
        if std::fs::symlink_metadata(dir.join(rel)).is_err() {
            bail!("the bundle's manifest names a file it does not hold: {rel}");
        }
    }
    for id in session_ids {
        if !aspen_core::paths::safe_id(id) {
            bail!("the bundle's manifest names an unsafe session id: {id:?}");
        }
    }
    Ok(())
}

pub fn stage(data_dir: &Path, file: &Path, pass: Option<&str>) -> Result<(String, Manifest)> {
    let id = uuid::Uuid::new_v4().to_string();
    let dir = staging_dir(data_dir, &id);
    std::fs::create_dir_all(&dir)?;
    let result = (|| -> Result<Manifest> {
        let mut ar = tar::Archive::new(open_stream(file, pass)?);
        ar.set_preserve_permissions(true);
        ar.set_overwrite(true);
        for e in ar.entries()? {
            let mut e = e?;
            // unpack_in refuses `..` and absolute paths.
            if !e.unpack_in(&dir)? {
                bail!(
                    "the bundle names a path outside itself: {}",
                    e.path()?.display()
                );
            }
        }
        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
        check_manifest_paths(
            m.files.iter().map(|f| f.rel.as_str()),
            m.sessions.iter().map(|s| s.id.as_str()),
            &dir,
        )?;
        let sums: Value = serde_json::from_slice(
            &std::fs::read(dir.join("sums.json")).context("the bundle has no sums (truncated?)")?,
        )?;
        // Verify: the context and names (the repo tree is checked too).
        if let Some(files) = sums.get("files").and_then(|f| f.as_object()) {
            for (rel, want) in files {
                let p = dir.join(rel);
                let Ok(b) = std::fs::read(&p) else {
                    bail!("missing from the bundle: {rel}");
                };
                let got = hex(&sha2::Sha256::digest(&b));
                if want.get("sha256").and_then(|s| s.as_str()) != Some(got.as_str()) {
                    bail!("checksum mismatch: {rel} (the bundle is damaged)");
                }
            }
        }
        Ok(m)
    })();
    match result {
        Ok(m) => Ok((id, m)),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// A new repo at the target path (absent or empty).
    #[default]
    New,
    /// Merge into the repo at the target path.
    TopUp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RepoFiles {
    /// Leave the target's files alone (git carries code).
    #[default]
    None,
    /// Write files the target does not have.
    Missing,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImportOpts {
    pub target: String,
    #[serde(default)]
    pub mode: ImportMode,
    /// Top-up only: what to do with the bundle's repo files.
    #[serde(default)]
    pub repo_files: RepoFiles,
    /// Register the bundle's names (not started).
    #[serde(default = "yes")]
    pub names: bool,
}

/// A line's identity for comparing two copies of one transcript: its
/// `uuid` (Claude), else its timestamp and type (Codex). Paths inside a
/// line may differ between copies; identities do not.
fn line_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            v.get("uuid")
                .and_then(|u| u.as_str())
                .map(str::to_owned)
                .or_else(|| {
                    let ts = v.get("timestamp").and_then(|t| t.as_str())?;
                    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    Some(format!("{ts}|{ty}"))
                })
        })
        .collect()
}

/// How an incoming transcript relates to the one already here.
fn relate(local: &str, incoming: &str) -> &'static str {
    let a = line_ids(local);
    let b = line_ids(incoming);
    if a == b {
        // Lines with no identity (cost-state, summaries) still count: the
        // copy with more of them is further along.
        let (la, lb) = (local.lines().count(), incoming.lines().count());
        if lb > la {
            "incoming_longer"
        } else if la > lb {
            "local_longer"
        } else {
            "same"
        }
    } else if b.len() > a.len() && b[..a.len()] == a[..] {
        "incoming_longer"
    } else if a.len() > b.len() && a[..b.len()] == b[..] {
        "local_longer"
    } else {
        "diverged"
    }
}

fn last_common(local: &str, incoming: &str) -> Option<String> {
    let a = line_ids(local);
    let b = line_ids(incoming);
    let mut last = None;
    for (x, y) in a.iter().zip(b.iter()) {
        if x != y {
            break;
        }
        // A fork point is a message uuid; stand-in ids (timestamp|type)
        // are for comparing only.
        if !x.contains('|') {
            last = Some(x.clone());
        }
    }
    last
}

/// What an import would do, before it does it.
pub fn plan(
    store: &crate::store::BusStore,
    data_dir: &Path,
    staging_id: &str,
    opts: &ImportOpts,
) -> Result<Value> {
    let dir = staging_dir(data_dir, staging_id);
    let m: Manifest = serde_json::from_slice(
        &std::fs::read(dir.join("manifest.json")).context("no such staged bundle (expired?)")?,
    )?;
    let target = PathBuf::from(&opts.target);
    let mut blockers: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let exists = target.exists();
    let empty = exists
        && std::fs::read_dir(&target)
            .map(|mut r| r.next().is_none())
            .unwrap_or(false);
    let repo_count = m.files.iter().filter(|f| f.part == "repo").count();
    let mut repo_missing = 0usize;
    match opts.mode {
        ImportMode::New => {
            if exists && !empty {
                blockers.push(format!(
                    "{} exists and is not empty — import as a top-up, or pick a new path",
                    target.display()
                ));
            }
            if repo_count == 0 {
                warnings.push("the bundle carries no repo files (context only): the new directory will be empty".into());
            }
        }
        ImportMode::TopUp => {
            if !target.is_dir() {
                blockers.push(format!(
                    "{} does not exist — import as a new repo",
                    target.display()
                ));
            } else {
                repo_missing = m
                    .files
                    .iter()
                    .filter(|f| f.part == "repo")
                    .filter(|f| !target.join(f.rel.trim_start_matches("repo/")).exists())
                    .count();
            }
        }
    }
    // Sessions: against where they would land on this node.
    let ctx = PathCtx::local(&if target.is_dir() {
        crate::node::normalize_repo(&target)
    } else {
        target.clone()
    });
    let project_dir = PathBuf::from(&ctx.project_dir);
    let taken = ids_taken_elsewhere(store, &target);
    let mut sessions: Vec<Value> = Vec::new();
    for s in &m.sessions {
        let (status, detail) = match s.harness {
            aspen_core::Harness::Claude => {
                let here = project_dir.join(format!("{}.jsonl", s.id));
                let incoming = dir.join(format!("context/claude/{}.jsonl", s.id));
                match (
                    std::fs::read_to_string(&here),
                    std::fs::read_to_string(&incoming),
                ) {
                    // The same conversation is a name's in another repo on
                    // this node (a copy of a repo here): a new id, so the
                    // two never share one (the one-writer rule).
                    (Err(_), _) if taken.contains(&s.id) => ("copy", None),
                    (Err(_), _) => ("new", None),
                    (Ok(l), Ok(i)) => (relate(&l, &i), None),
                    (Ok(_), Err(_)) => ("missing_in_bundle", None),
                }
            }
            aspen_core::Harness::Codex => {
                let codex = aspen_codex::CodexStore::new();
                if aspen_core::SessionStore::exists(&codex, &target, &s.id) {
                    (
                        "exists",
                        Some("a Codex rollout with this id is here already; kept"),
                    )
                } else {
                    ("new", None)
                }
            }
        };
        sessions.push(json!({
            "id": s.id, "harness": s.harness, "title": s.title, "name": s.name,
            "status": status, "detail": detail,
            "action": match status {
                "new" => "install",
                "copy" => "install under a new id (this node has that session in another repo)",
                "incoming_longer" => "replace (the incoming one is the same conversation, further along; the local copy is kept as .bak)",
                "local_longer" => "keep local (it is further along)",
                "same" => "nothing to do",
                "diverged" => "install as a fork (new id, lineage to the local one)",
                _ => "keep local",
            },
        }));
    }
    // Memory conflicts.
    let mut memory_new = 0usize;
    let mut memory_conflicts = 0usize;
    for f in m.files.iter().filter(|f| f.part == "memory") {
        let rel = f.rel.trim_start_matches("context/claude/memory/");
        let here = project_dir.join("memory").join(rel);
        if !here.exists() {
            memory_new += 1;
        } else {
            let incoming = std::fs::read_to_string(dir.join(&f.rel)).unwrap_or_default();
            let local = std::fs::read_to_string(&here).unwrap_or_default();
            if localize_str(&incoming, &ctx) != local {
                memory_conflicts += 1;
            }
        }
    }
    // Names.
    let mut names: Vec<Value> = Vec::new();
    if opts.names {
        if let Ok(a) = std::fs::read(dir.join("aspen/agents.json")) {
            let v: Value = serde_json::from_slice(&a).unwrap_or(Value::Null);
            let handle = target_handle(store, &target);
            let rows = store.agents().unwrap_or_default();
            for r in v
                .get("agents")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                let name = r.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let sid = r.get("session_id").and_then(|n| n.as_str()).unwrap_or("");
                let (final_name, action) = name_for(&rows, name, &handle, sid);
                names.push(json!({ "name": name, "as": final_name, "action": action }));
            }
        }
    }
    Ok(json!({
        "staging_id": staging_id,
        "manifest": {
            "source_node": m.source_node, "created_at": m.created_at, "repo": m.repo,
            "options": m.options, "sealed": m.sealed, "aspen_version": m.aspen_version, "notes": m.notes,
        },
        "target": opts.target,
        "mode": opts.mode,
        "repo": { "files": repo_count, "missing_here": repo_missing },
        "sessions": sessions,
        "memory": { "new": memory_new, "conflicts": memory_conflicts },
        "names": names,
        "blockers": blockers,
        "warnings": warnings,
    }))
}

/// A repo's handle on this node, matched on normalized paths (`~/src/hub/`
/// and `~/src/hub` are one repo); its basename when not registered. Shared
/// with session-bundle import, whose copy compared raw paths and missed
/// registered handles (the 2026-10 quality pass).
pub(crate) fn target_handle(store: &crate::store::BusStore, target: &Path) -> String {
    let t = crate::node::normalize_repo(target);
    store
        .repos()
        .unwrap_or_default()
        .into_iter()
        .find(|r| crate::node::normalize_repo(&r.path) == t)
        .map(|r| r.handle)
        .unwrap_or_else(|| {
            t.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "repo".into())
        })
}

/// Session ids a name on this node holds in a repo other than `target`.
fn ids_taken_elsewhere(store: &crate::store::BusStore, target: &Path) -> HashSet<String> {
    let t = crate::node::normalize_repo(target);
    store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.moved_to.is_none() && crate::node::normalize_repo(&a.repo) != t)
        .filter_map(|a| a.session_id)
        .collect()
}

/// The name a bundle's agent lands under: its bare name on this repo's
/// handle; if that is taken by another transcript, `bare-2`, `bare-3`….
fn name_for(
    rows: &[crate::store::AgentRow],
    name: &str,
    handle: &str,
    sid: &str,
) -> (String, &'static str) {
    let bare = name.split('@').next().unwrap_or(name);
    let want = format!("{bare}@{handle}");
    match rows.iter().find(|r| r.name == want) {
        None => (want, "register"),
        Some(r) if r.session_id.as_deref() == Some(sid) => (want, "already here"),
        Some(_) => {
            for i in 2..100 {
                let alt = format!("{bare}-{i}@{handle}");
                if !rows.iter().any(|r| r.name == alt) {
                    return (alt, "register (name taken here)");
                }
            }
            (
                format!("{bare}-{}@{handle}", uuid::Uuid::new_v4().simple()),
                "register (name taken here)",
            )
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ImportReport {
    pub repo: String,
    pub handle: String,
    pub repo_files_written: usize,
    pub sessions_installed: Vec<String>,
    pub sessions_replaced: Vec<String>,
    pub sessions_forked: Vec<(String, String)>,
    pub sessions_kept: Vec<String>,
    pub memory_written: usize,
    pub memory_conflicts: Vec<String>,
    pub names: Vec<String>,
    pub residue: usize,
    pub notes: Vec<String>,
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<usize> {
    let mut v = Vec::new();
    walk(src, "", &mut v);
    let mut n = 0;
    for r in v {
        let s = src.join(&r);
        let d = dst.join(&r);
        if let Some(p) = d.parent() {
            std::fs::create_dir_all(p)?;
        }
        let meta = std::fs::symlink_metadata(&s)?;
        if meta.file_type().is_symlink() {
            #[cfg(unix)]
            {
                let t = std::fs::read_link(&s)?;
                let _ = std::fs::remove_file(&d);
                std::os::unix::fs::symlink(t, &d)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::copy(&s, &d).ok();
            }
        } else {
            std::fs::copy(&s, &d)?;
        }
        n += 1;
    }
    Ok(n)
}

/// Give `dst` the modification time of `src` (the staged file, which kept
/// the source machine's time).
fn keep_mtime(src: &Path, dst: &Path) {
    if let Ok(t) = std::fs::metadata(src).and_then(|m| m.modified()) {
        if let Ok(f) = std::fs::File::options().write(true).open(dst) {
            let _ = f.set_modified(t);
        }
    }
}

/// Rewrite a transcript onto a new session id (a fork of a diverged copy).
fn refork(text: &str, old: &str, new: &str) -> String {
    rewrite_jsonl(text, &|s: &str| {
        if s == old {
            new.to_owned()
        } else {
            s.to_owned()
        }
    })
}

/// Install a staged bundle.
pub fn install(
    store: &crate::store::BusStore,
    data_dir: &Path,
    staging_id: &str,
    opts: &ImportOpts,
) -> Result<ImportReport> {
    let dir = staging_dir(data_dir, staging_id);
    let m: Manifest = serde_json::from_slice(
        &std::fs::read(dir.join("manifest.json")).context("no such staged bundle (expired?)")?,
    )?;
    let p = plan(store, data_dir, staging_id, opts)?;
    if let Some(b) = p
        .get("blockers")
        .and_then(|b| b.as_array())
        .filter(|b| !b.is_empty())
    {
        bail!(
            "{}",
            b.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    let mut rep = ImportReport::default();
    let target = PathBuf::from(&opts.target);

    // 1. The tree.
    let staged_repo = dir.join("repo");
    match opts.mode {
        ImportMode::New => {
            std::fs::create_dir_all(&target)?;
            if staged_repo.is_dir() {
                rep.repo_files_written = copy_dir_all(&staged_repo, &target)?;
            }
        }
        ImportMode::TopUp => {
            if opts.repo_files == RepoFiles::Missing && staged_repo.is_dir() {
                let mut v = Vec::new();
                walk(&staged_repo, "", &mut v);
                for r in v {
                    // Never touch the target's own .git.
                    if r == ".git" || r.starts_with(".git/") {
                        continue;
                    }
                    let d = target.join(&r);
                    if d.exists() {
                        continue;
                    }
                    if let Some(pp) = d.parent() {
                        std::fs::create_dir_all(pp)?;
                    }
                    std::fs::copy(staged_repo.join(&r), &d)?;
                    rep.repo_files_written += 1;
                }
            }
        }
    }
    let repo = crate::node::normalize_repo(&target);
    store.add_repo(&repo, None)?;
    let handle = target_handle(store, &repo);
    rep.repo = repo.to_string_lossy().into_owned();
    rep.handle = handle.clone();

    // 2. Context, localized to this node.
    let ctx = PathCtx::local(&repo);
    let local = |s: &str| localize_str(s, &ctx);
    let project_dir = PathBuf::from(&ctx.project_dir);
    std::fs::create_dir_all(&project_dir)?;
    let foreign_home = m.source.home.clone();
    let mut renamed: HashMap<String, String> = HashMap::new();
    let statuses: HashMap<String, String> = p
        .get("sessions")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some((
                s.get("id")?.as_str()?.to_owned(),
                s.get("status")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    for s in m
        .sessions
        .iter()
        .filter(|s| s.harness == aspen_core::Harness::Claude)
    {
        let incoming = dir.join(format!("context/claude/{}.jsonl", s.id));
        let Ok(text) = std::fs::read_to_string(&incoming) else {
            continue;
        };
        let out = rewrite_jsonl(&text, &local);
        if !foreign_home.is_empty() && foreign_home != ctx.home {
            rep.residue += out.matches(&foreign_home).count();
        }
        let here = project_dir.join(format!("{}.jsonl", s.id));
        let status = statuses.get(&s.id).map(String::as_str).unwrap_or("new");
        let side_src = dir.join(format!("context/claude/{}", s.id));
        let install_side = |into: &Path| -> Result<()> {
            if side_src.is_dir() {
                let mut v = Vec::new();
                walk(&side_src, "", &mut v);
                for r in v {
                    let d = into.join(&r);
                    let s2 = side_src.join(&r);
                    // Sidecar files are append-only or write-once: keep the
                    // bigger copy.
                    if d.exists() && file_size(&d) >= file_size(&s2) {
                        continue;
                    }
                    if let Some(pp) = d.parent() {
                        std::fs::create_dir_all(pp)?;
                    }
                    if r.ends_with(".jsonl") {
                        let t = std::fs::read_to_string(&s2).unwrap_or_default();
                        std::fs::write(&d, rewrite_jsonl(&t, &local))?;
                    } else if texty(&r) {
                        let t = std::fs::read_to_string(&s2).unwrap_or_default();
                        std::fs::write(&d, local(&t))?;
                    } else {
                        std::fs::copy(&s2, &d)?;
                    }
                    keep_mtime(&s2, &d);
                }
            }
            Ok(())
        };
        match status {
            "new" => {
                std::fs::write(&here, &out)?;
                keep_mtime(&incoming, &here);
                install_side(&project_dir.join(&s.id))?;
                rep.sessions_installed.push(s.id.clone());
            }
            "incoming_longer" => {
                let bak = project_dir.join(format!(
                    "{}.jsonl.bak-{}",
                    s.id,
                    crate::store::now_epoch() as u64
                ));
                std::fs::rename(&here, &bak)?;
                std::fs::write(&here, &out)?;
                keep_mtime(&incoming, &here);
                install_side(&project_dir.join(&s.id))?;
                rep.sessions_replaced.push(s.id.clone());
                rep.notes.push(format!(
                    "{}: the local copy was kept as {}",
                    s.id,
                    bak.display()
                ));
            }
            "copy" => {
                let new_id = uuid::Uuid::new_v4().to_string();
                let forked = project_dir.join(format!("{new_id}.jsonl"));
                std::fs::write(&forked, refork(&out, &s.id, &new_id))?;
                keep_mtime(&incoming, &forked);
                install_side(&project_dir.join(&new_id))?;
                let _ = store.record_lineage(s.name.as_deref().unwrap_or(""), &new_id, &s.id, None);
                renamed.insert(s.id.clone(), new_id.clone());
                rep.sessions_installed.push(new_id.clone());
                rep.notes.push(format!(
                    "{} installed as {new_id}: this node has that session in another repo",
                    s.id
                ));
            }
            "diverged" => {
                let new_id = uuid::Uuid::new_v4().to_string();
                let local_text = std::fs::read_to_string(&here).unwrap_or_default();
                let at = last_common(&local_text, &out);
                let forked = project_dir.join(format!("{new_id}.jsonl"));
                std::fs::write(&forked, refork(&out, &s.id, &new_id))?;
                keep_mtime(&incoming, &forked);
                install_side(&project_dir.join(&new_id))?;
                let _ = store.record_lineage(
                    s.name.as_deref().unwrap_or(""),
                    &new_id,
                    &s.id,
                    at.as_deref(),
                );
                renamed.insert(s.id.clone(), new_id.clone());
                rep.sessions_forked.push((s.id.clone(), new_id));
            }
            _ => {
                install_side(&project_dir.join(&s.id))?;
                rep.sessions_kept.push(s.id.clone());
            }
        }
    }
    // Codex rollouts: new ones installed under this node's Codex home.
    for f in m.files.iter().filter(|f| f.part == "rollout") {
        let rel = f.rel.trim_start_matches("context/codex/");
        let d = aspen_codex::codex_home().join(rel);
        if d.exists() {
            continue;
        }
        if let Some(pp) = d.parent() {
            std::fs::create_dir_all(pp)?;
        }
        let t = std::fs::read_to_string(dir.join(&f.rel)).unwrap_or_default();
        std::fs::write(&d, rewrite_jsonl(&t, &local))?;
        keep_mtime(&dir.join(&f.rel), &d);
    }
    for s in m
        .sessions
        .iter()
        .filter(|s| s.harness == aspen_core::Harness::Codex)
    {
        if statuses.get(&s.id).map(String::as_str) == Some("new") {
            rep.sessions_installed.push(s.id.clone());
        } else {
            rep.sessions_kept.push(s.id.clone());
        }
    }
    // Memory: new files written; a differing one kept beside as
    // `<name>.from-<node>.<ext>`.
    for f in m.files.iter().filter(|f| f.part == "memory") {
        let rel = f.rel.trim_start_matches("context/claude/memory/");
        let d = project_dir.join("memory").join(rel);
        if let Some(pp) = d.parent() {
            std::fs::create_dir_all(pp)?;
        }
        let src = dir.join(&f.rel);
        let bytes = if f.kind == "text" {
            local(&std::fs::read_to_string(&src).unwrap_or_default()).into_bytes()
        } else {
            std::fs::read(&src).unwrap_or_default()
        };
        if d.exists() {
            if std::fs::read(&d).unwrap_or_default() == bytes {
                continue;
            }
            let alt = d.with_file_name(format!(
                "{}.from-{}{}",
                d.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                m.source_node,
                d.extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default()
            ));
            std::fs::write(&alt, bytes)?;
            rep.memory_conflicts
                .push(alt.to_string_lossy().into_owned());
        } else {
            std::fs::write(&d, bytes)?;
            rep.memory_written += 1;
        }
    }

    // 3. Names, not started.
    if opts.names {
        if let Ok(a) = std::fs::read(dir.join("aspen/agents.json")) {
            let v: Value = serde_json::from_slice(&a).unwrap_or(Value::Null);
            for r in v
                .get("agents")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                let name = r.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let sid0 = r.get("session_id").and_then(|n| n.as_str()).unwrap_or("");
                let sid = renamed
                    .get(sid0)
                    .cloned()
                    .unwrap_or_else(|| sid0.to_owned());
                let rows = store.agents().unwrap_or_default();
                let (full, action) = name_for(&rows, name, &handle, &sid);
                if action == "already here" {
                    continue;
                }
                let harness: aspen_core::Harness = r
                    .get("harness")
                    .and_then(|h| serde_json::from_value(h.clone()).ok())
                    .unwrap_or_default();
                store.register_agent(
                    &full,
                    &repo,
                    &handle,
                    &sid,
                    r.get("charter")
                        .and_then(|c| c.as_str())
                        .map(local)
                        .as_deref(),
                    r.get("extra_args").and_then(|c| c.as_str()),
                    harness,
                )?;
                if let Some(t) = r.get("title").and_then(|t| t.as_str()) {
                    let _ = store.set_agent_title(&full, Some(t));
                }
                for b in r
                    .get("bookmarks")
                    .and_then(|b| b.as_array())
                    .into_iter()
                    .flatten()
                {
                    let bs = b.get("session_id").and_then(|s| s.as_str()).unwrap_or(&sid);
                    let bs = renamed.get(bs).cloned().unwrap_or_else(|| bs.to_owned());
                    let _ = store.add_bookmark(
                        &full,
                        &bs,
                        b.get("message_uuid").and_then(|s| s.as_str()),
                        b.get("label").and_then(|s| s.as_str()),
                        b.get("reason")
                            .and_then(|s| s.as_str())
                            .unwrap_or("imported"),
                    );
                }
                rep.names.push(full);
            }
            for l in v
                .get("lineage")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                let (Some(c), Some(pp)) = (
                    l.get("child").and_then(|x| x.as_str()),
                    l.get("parent").and_then(|x| x.as_str()),
                ) else {
                    continue;
                };
                let c = renamed.get(c).cloned().unwrap_or_else(|| c.to_owned());
                let pp = renamed.get(pp).cloned().unwrap_or_else(|| pp.to_owned());
                let _ = store.record_lineage(
                    "",
                    &c,
                    &pp,
                    l.get("fork_message").and_then(|x| x.as_str()),
                );
            }
        }
    }
    let _ = store.record_event(
        "node",
        "repo_imported",
        json!({ "repo": rep.repo, "from": m.source_node, "bundle": m.bundle_id, "mode": opts.mode, "sessions": m.sessions.len() }),
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(rep)
}

/// Drop staged bundles older than a day (a preflight nobody confirmed).
pub fn sweep_staging(data_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(data_dir.join("staging")) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.starts_with("repo-") {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|d| d.as_secs() > 86_400);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

// ------------------------------------------------------------ node verbs
//
// The same four verbs serve the local API and the mesh ops (a peer's
// repo): each takes the request body as JSON and answers JSON. They block
// (disk and git), so callers run them off the runtime.

pub fn exports_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("exports")
}

pub fn imports_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("imports")
}

fn path_arg(body: &Value, key: &str) -> Result<PathBuf> {
    body.get(key)
        .and_then(|p| p.as_str())
        .filter(|p| !p.trim().is_empty())
        .map(|p| PathBuf::from(expand_home(p.trim())))
        .ok_or_else(|| anyhow!("missing {key}"))
}

fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| (p == "~").then_some("")) {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        return format!("{home}/{rest}");
    }
    p.to_owned()
}

/// `{path}` → the preflight.
pub fn verb_export_preflight(inner: &crate::node::NodeInner, body: &Value) -> Result<Value> {
    export_preflight(&inner.store, &path_arg(body, "path")?)
}

/// `{path, repo_mode, sessions, sidecars, memory, names, passphrase, out?}`
/// → the export summary, with `file` (the name under this node's exports
/// dir, for a download) when no `out` was given.
pub fn verb_export(inner: &crate::node::NodeInner, body: &Value) -> Result<Value> {
    let repo = path_arg(body, "path")?;
    let opts: ExportOpts = serde_json::from_value(body.clone()).context("export options")?;
    let data_dir = inner
        .data_dir
        .clone()
        .ok_or_else(|| anyhow!("node has no data dir"))?;
    let (out, in_exports) = match body
        .get("out")
        .and_then(|o| o.as_str())
        .filter(|o| !o.trim().is_empty())
    {
        Some(o) => {
            let mut p = PathBuf::from(expand_home(o.trim()));
            if p.is_dir() {
                p = p.join(default_name(&repo));
            }
            (p, false)
        }
        None => (exports_dir(&data_dir).join(default_name(&repo)), true),
    };
    let out = std::path::absolute(&out).unwrap_or(out);
    let sum = export(&inner.store, &inner.node_name(), &repo, &opts, &out)?;
    let _ = inner.store.record_event(
        "node",
        "repo_exported",
        json!({ "repo": repo.to_string_lossy(), "out": sum.out, "bytes": sum.bytes, "sessions": sum.sessions, "sealed": sum.sealed }),
    );
    let mut v = serde_json::to_value(&sum)?;
    if in_exports {
        v["file"] = json!(out.file_name().map(|n| n.to_string_lossy().into_owned()));
    }
    Ok(v)
}

fn default_name(repo: &Path) -> String {
    let base = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M");
    format!("{base}-{stamp}.{EXT}")
}

/// `{file, passphrase?, target, mode, repo_files, names}` → stage the
/// bundle and answer the plan (with `staging_id` for the install).
pub fn verb_import_preflight(inner: &crate::node::NodeInner, body: &Value) -> Result<Value> {
    let data_dir = inner
        .data_dir
        .clone()
        .ok_or_else(|| anyhow!("node has no data dir"))?;
    sweep_staging(&data_dir);
    let mut file = path_arg(body, "file")?;
    // A bare name is one uploaded to this node's imports dir.
    if file.components().count() == 1 {
        let up = imports_dir(&data_dir).join(&file);
        if up.is_file() {
            file = up;
        }
    }
    if !file.is_file() {
        bail!("no such file on this node: {}", file.display());
    }
    let pass = body
        .get("passphrase")
        .and_then(|p| p.as_str())
        .filter(|p| !p.is_empty());
    let (id, _m) = stage(&data_dir, &file, pass)?;
    let mut opts: ImportOpts = serde_json::from_value(body.clone()).context("import options")?;
    opts.target = expand_home(opts.target.trim());
    plan(&inner.store, &data_dir, &id, &opts)
}

/// `{staging_id, target, mode, repo_files, names}` → the report.
pub fn verb_import(inner: &crate::node::NodeInner, body: &Value) -> Result<Value> {
    let data_dir = inner
        .data_dir
        .clone()
        .ok_or_else(|| anyhow!("node has no data dir"))?;
    let id = body
        .get("staging_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.contains(['/', '\\', '.']))
        .ok_or_else(|| anyhow!("missing staging_id (run the preflight first)"))?;
    let mut opts: ImportOpts = serde_json::from_value(body.clone()).context("import options")?;
    opts.target = expand_home(opts.target.trim());
    Ok(serde_json::to_value(install(
        &inner.store,
        &data_dir,
        id,
        &opts,
    )?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_path_that_escapes_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::create_dir_all(d.join("context/claude/memory/home/u")).unwrap();
        std::fs::write(d.join("context/claude/memory/home/u/x"), "evil").unwrap();
        std::fs::create_dir_all(d.join("context/claude/memory")).unwrap();
        std::fs::write(d.join("context/claude/memory/notes.md"), "ok").unwrap();
        let ok = check_manifest_paths(
            ["context/claude/memory/notes.md"].into_iter(),
            ["0a1b-2c"].into_iter(),
            d,
        );
        assert!(ok.is_ok());
        // The prefix-trim trick: whole path looks relative, the rest is absolute.
        let evil = check_manifest_paths(
            ["context/claude/memory//home/u/x"].into_iter(),
            std::iter::empty(),
            d,
        );
        assert!(evil.is_err());
        assert!(check_manifest_paths(["../outside"].into_iter(), std::iter::empty(), d).is_err());
        assert!(check_manifest_paths(
            ["context/claude/memory/missing.md"].into_iter(),
            std::iter::empty(),
            d
        )
        .is_err());
        assert!(check_manifest_paths(std::iter::empty(), ["../x"].into_iter(), d).is_err());
    }

    #[test]
    fn seal_round_trip_and_wrong_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.bin");
        let data: Vec<u8> = (0..(CHUNK * 2 + 777)).map(|i| (i % 251) as u8).collect();
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut w = SealWriter::new(f, "hunter2").unwrap();
            w.write_all(&data).unwrap();
            w.finish().unwrap();
        }
        let read = |pass: &str| -> std::io::Result<Vec<u8>> {
            use chacha20poly1305::KeyInit;
            let mut f = std::fs::File::open(&p).unwrap();
            let mut head = [0u8; 15];
            f.read_exact(&mut head).unwrap();
            let mut salt = [0u8; 16];
            f.read_exact(&mut salt).unwrap();
            let mut rest = [0u8; 9];
            f.read_exact(&mut rest).unwrap();
            let mut prefix = [0u8; 16];
            f.read_exact(&mut prefix).unwrap();
            let key = derive_key(pass, &salt, rest[0], 8, 1).unwrap();
            let mut r = OpenReader {
                inner: f,
                cipher: chacha20poly1305::XChaCha20Poly1305::new((&key).into()),
                prefix,
                counter: 0,
                plain: Vec::new(),
                pos: 0,
                done: false,
            };
            let mut out = Vec::new();
            r.read_to_end(&mut out)?;
            Ok(out)
        };
        assert_eq!(read("hunter2").unwrap(), data);
        assert!(read("wrong").is_err());
        // Truncated: drop the last sealed chunk.
        let full = std::fs::read(&p).unwrap();
        std::fs::write(&p, &full[..full.len() - 900]).unwrap();
        assert!(read("hunter2").is_err());
    }

    #[test]
    fn transcripts_relate_by_line_identity() {
        let a = "{\"uuid\":\"1\",\"cwd\":\"/home/a/r\"}\n{\"uuid\":\"2\"}\n";
        let b = "{\"uuid\":\"1\",\"cwd\":\"/home/b/r\"}\n{\"uuid\":\"2\"}\n{\"uuid\":\"3\"}\n";
        let c = "{\"uuid\":\"1\"}\n{\"uuid\":\"9\"}\n";
        assert_eq!(relate(a, a), "same");
        let a2 = format!("{a}{{\"type\":\"cost-state\"}}\n");
        assert_eq!(relate(a, &a2), "incoming_longer");
        assert_eq!(relate(a, b), "incoming_longer");
        assert_eq!(relate(b, a), "local_longer");
        assert_eq!(relate(a, c), "diverged");
        assert_eq!(last_common(a, c).as_deref(), Some("1"));
        let r = refork("{\"sessionId\":\"old\",\"x\":\"old-ish\"}\n", "old", "new");
        assert!(r.contains("\"sessionId\":\"new\"") && r.contains("old-ish"));
    }
}
