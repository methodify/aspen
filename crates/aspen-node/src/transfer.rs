//! Moving repos around the mesh (PROPOSALS-2026-10-U): pick a path on a
//! node (a listing that never reads a file), send a bundle to a node in
//! pieces and read one back in pieces, and transfer a repo from one node
//! to another (export there, pull here, plan, install on the operator's
//! confirm). One `dispatch` serves the console's routes and the mesh ops.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::node::NodeInner;
use crate::repobundle::{exports_dir, imports_dir, EXT};

/// The ops this module answers, local or from a peer.
pub const OPS: &[&str] = &[
    "fs_list",
    "upload_chunk",
    "upload_size",
    "upload_done",
    "export_read",
    "export_delete",
    "transfer_start",
    "transfer_get",
    "transfer_install",
];

/// Most entries one listing returns (as `artifacts::list_dir`).
const LIST_MAX: usize = 2000;
/// Largest piece read or written at once (base64 on the wire).
pub const PIECE: u64 = 1024 * 1024;

pub async fn dispatch(inner: &Arc<NodeInner>, op: &str, body: &Value) -> Result<Value> {
    let data_dir = inner
        .data_dir
        .clone()
        .ok_or_else(|| anyhow!("node has no data dir"))?;
    let s = |k: &str| {
        body.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned()
    };
    let n = |k: &str| body.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    match op {
        "fs_list" => {
            let inner = inner.clone();
            let dir = body
                .get("dir")
                .and_then(|d| d.as_str())
                .filter(|d| !d.is_empty())
                .map(str::to_owned);
            tokio::task::spawn_blocking(move || fs_list(&inner, dir.as_deref()))
                .await
                .map_err(|e| anyhow!("{e}"))?
        }
        "upload_chunk" => upload_chunk(&data_dir, &s("id"), n("offset"), &s("data")),
        "upload_size" => Ok(json!({ "size": upload_size(&data_dir, &s("id"))? })),
        "upload_done" => upload_done(&data_dir, &s("id")),
        "export_read" => export_read(&data_dir, &s("file"), n("offset"), n("len")),
        "export_delete" => {
            let p = export_path(&data_dir, &s("file"))?;
            let _ = std::fs::remove_file(p);
            Ok(json!({ "ok": true }))
        }
        "transfer_start" => start(inner.clone(), body.clone()),
        "transfer_get" => get(&s("id")),
        "transfer_install" => install(inner, &s("id")).await,
        _ => bail!("unknown op {op}"),
    }
}

// ---------------------------------------------------------------- U-1

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// Places to start from: home, this node's exports and imports, the repos
/// it knows, the filesystem roots (each drive on Windows).
fn starts(inner: &NodeInner) -> Vec<Value> {
    let mut out = Vec::new();
    let mut push = |label: &str, p: PathBuf| {
        if p.is_dir() {
            out.push(json!({ "label": label, "path": p.to_string_lossy() }));
        }
    };
    if let Some(h) = home() {
        push("home", h);
    }
    if let Some(d) = inner.data_dir.as_deref() {
        push("exports", exports_dir(d));
        push("imports", imports_dir(d));
    }
    for r in inner.store.repos().unwrap_or_default().into_iter().take(12) {
        let label = format!("#{}", r.handle);
        push(&label, r.path);
    }
    if cfg!(windows) {
        for c in b'A'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", c as char));
            if p.is_dir() {
                push(&format!("{}:", c as char), p);
            }
        }
    } else {
        push("/", PathBuf::from("/"));
    }
    out
}

/// One directory's entries: name, kind, size, modified time. Never opens
/// a file. No `dir`: the starting points.
pub fn fs_list(inner: &NodeInner, dir: Option<&str>) -> Result<Value> {
    let Some(dir) = dir else {
        return Ok(
            json!({ "dir": null, "parent": null, "entries": [], "starts": starts(inner), "truncated": false }),
        );
    };
    let expanded = match dir.strip_prefix('~') {
        Some(rest) => home()
            .map(|h| h.join(rest.trim_start_matches(['/', '\\'])))
            .unwrap_or_else(|| PathBuf::from(dir)),
        None => PathBuf::from(dir),
    };
    let target = std::path::absolute(&expanded).unwrap_or(expanded);
    let rd = std::fs::read_dir(&target).map_err(|e| anyhow!("{}: {e}", target.display()))?;
    let mut entries: Vec<Value> = Vec::new();
    let mut truncated = false;
    for e in rd.flatten() {
        if entries.len() >= LIST_MAX {
            truncated = true;
            break;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(lm) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        let meta = std::fs::metadata(e.path()).ok();
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        entries.push(json!({
            "name": name,
            "kind": if lm.file_type().is_symlink() { "link" } else if is_dir { "dir" } else { "file" },
            "is_dir": is_dir,
            "size": meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
            "mtime": meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()),
            "hidden": name.starts_with('.'),
        }));
    }
    entries.sort_by(|a, b| {
        let (ad, bd) = (
            a["is_dir"].as_bool().unwrap_or(false),
            b["is_dir"].as_bool().unwrap_or(false),
        );
        bd.cmp(&ad).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    Ok(json!({
        "dir": target.to_string_lossy(),
        "parent": target.parent().map(|p| p.to_string_lossy().into_owned()),
        "entries": entries,
        "starts": starts(inner),
        "truncated": truncated,
        "sep": std::path::MAIN_SEPARATOR.to_string(),
    }))
}

// ---------------------------------------------------------------- U-2

fn part_path(data_dir: &Path, id: &str) -> Result<PathBuf> {
    if !aspen_core::paths::safe_id(id) {
        bail!("bad upload id");
    }
    Ok(imports_dir(data_dir).join(format!("upload-{id}.part")))
}

fn upload_size(data_dir: &Path, id: &str) -> Result<u64> {
    Ok(std::fs::metadata(part_path(data_dir, id)?)
        .map(|m| m.len())
        .unwrap_or(0))
}

/// Append one piece at `offset`. A piece at the wrong offset is refused
/// with the size so far, so the console resumes from there.
fn upload_chunk(data_dir: &Path, id: &str, offset: u64, data: &str) -> Result<Value> {
    use std::io::Write;
    let p = part_path(data_dir, id)?;
    std::fs::create_dir_all(imports_dir(data_dir))?;
    let have = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    if offset != have {
        bail!("upload is at {have}, not {offset}");
    }
    let bytes = aspen_wire::b64::decode(data).map_err(|e| anyhow!("bad piece: {e}"))?;
    if bytes.len() as u64 > PIECE {
        bail!("a piece is at most {PIECE} bytes");
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)?;
    f.write_all(&bytes)?;
    Ok(json!({ "size": have + bytes.len() as u64 }))
}

/// The upload is whole: it becomes `upload-<id>.aspen-repo` in imports,
/// a bare name the import preflight takes.
fn upload_done(data_dir: &Path, id: &str) -> Result<Value> {
    let p = part_path(data_dir, id)?;
    let name = format!("upload-{id}.{EXT}");
    let out = imports_dir(data_dir).join(&name);
    std::fs::rename(&p, &out)?;
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    Ok(json!({ "file": name, "path": out.to_string_lossy(), "bytes": bytes }))
}

/// A bundle in this node's exports folder, by bare name only.
fn export_path(data_dir: &Path, file: &str) -> Result<PathBuf> {
    if file.is_empty()
        || file.contains(['/', '\\'])
        || file.starts_with('.')
        || !file.ends_with(EXT)
    {
        bail!("bad file name");
    }
    Ok(exports_dir(data_dir).join(file))
}

/// `len` bytes of an exported bundle from `offset` (at most a piece).
fn export_read(data_dir: &Path, file: &str, offset: u64, len: u64) -> Result<Value> {
    use std::io::{Read, Seek, SeekFrom};
    let p = export_path(data_dir, file)?;
    let mut f = std::fs::File::open(&p).map_err(|e| anyhow!("{file}: {e}"))?;
    let size = f.metadata()?.len();
    let len = len.clamp(1, PIECE).min(size.saturating_sub(offset));
    let mut buf = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(&mut buf)?;
    Ok(json!({
        "data": aspen_wire::b64::encode(&buf),
        "size": size,
        "eof": offset + len >= size,
    }))
}

// ---------------------------------------------------------------- U-3

/// A transfer, run by the target node.
#[derive(Clone, Default, serde::Serialize)]
struct Job {
    id: String,
    from: String,
    repo: String,
    /// exporting | pulling | planning | ready | installing | done | failed
    step: String,
    bytes: u64,
    total: u64,
    /// the source's file in its exports folder, once written
    source_file: Option<String>,
    /// source and target are this node: no pull (v0.52.1)
    local: bool,
    /// the pulled file in this node's imports folder
    file: Option<String>,
    plan: Option<Value>,
    report: Option<Value>,
    error: Option<String>,
    started_at: f64,
    #[serde(skip)]
    import: Value,
}

fn jobs() -> &'static Mutex<HashMap<String, Job>> {
    static J: std::sync::OnceLock<Mutex<HashMap<String, Job>>> = std::sync::OnceLock::new();
    J.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set(id: &str, f: impl FnOnce(&mut Job)) {
    if let Some(j) = jobs().lock().unwrap().get_mut(id) {
        f(j);
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Start a transfer here. `body`: `from` (the source node; empty or this
/// node's own name for a transfer within this node, say a session moved
/// to another repo here), `path` (the repo there) and the export choices
/// as export takes them; `import`:
/// `{target, mode, repo_files, names}` for the import here (the export's
/// `passphrase`, if it seals, opens it here too).
fn start(inner: Arc<NodeInner>, body: Value) -> Result<Value> {
    let from = body
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let repo = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    if repo.is_empty() {
        bail!("a transfer needs the repo's path on the source");
    }
    // The import here, apart from the export there (both say `names`).
    let imp = body.get("import").cloned().unwrap_or(Value::Null);
    if imp
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        bail!("say where the repo goes on this node");
    }
    let self_name = inner.mesh().map(|m| m.identity.node.clone());
    let local = from.is_empty() || self_name.as_deref() == Some(from.as_str());
    if !local && self_name.is_none() {
        bail!("this node is not in a mesh");
    }
    // Old jobs go after a day.
    jobs()
        .lock()
        .unwrap()
        .retain(|_, j| now() - j.started_at < 86400.0);
    let id = uuid::Uuid::new_v4().simple().to_string();
    let import = json!({
        "target": imp.get("target").cloned().unwrap_or(Value::Null),
        "mode": imp.get("mode").cloned().unwrap_or(json!("new")),
        "repo_files": imp.get("repo_files").cloned().unwrap_or(json!("none")),
        "names": imp.get("names").cloned().unwrap_or(json!(true)),
        "passphrase": body.get("passphrase").cloned().unwrap_or(Value::Null),
    });
    jobs().lock().unwrap().insert(
        id.clone(),
        Job {
            id: id.clone(),
            from: from.clone(),
            repo: repo.clone(),
            step: "exporting".into(),
            started_at: now(),
            local,
            import,
            ..Default::default()
        },
    );
    let id2 = id.clone();
    tokio::spawn(async move {
        let r = if local {
            run_local(&inner, &id2, body).await
        } else {
            run(&inner, &id2, &from, body).await
        };
        if let Err(e) = r {
            set(&id2, |j| {
                j.step = "failed".into();
                j.error = Some(format!("{e:#}"));
            });
        }
    });
    Ok(json!({ "id": id }))
}

async fn run(inner: &Arc<NodeInner>, id: &str, from: &str, body: Value) -> Result<()> {
    let mesh = inner
        .mesh()
        .ok_or_else(|| anyhow!("this node is not in a mesh"))?;
    let data_dir = inner
        .data_dir
        .clone()
        .ok_or_else(|| anyhow!("node has no data dir"))?;
    // 1. Export on the source, into its exports folder.
    let mut export = body.clone();
    for k in ["from", "to", "node", "import", "out"] {
        export.as_object_mut().map(|o| o.remove(k));
    }
    let done = mesh
        .api_call(
            from,
            "repo_export",
            "",
            export,
            std::time::Duration::from_secs(3600),
        )
        .await
        .map_err(|e| anyhow!("exporting on {from}: {e}"))?;
    if let Some(e) = done.get("error").and_then(|e| e.as_str()) {
        bail!("exporting on {from}: {e}");
    }
    let file = done
        .get("file")
        .and_then(|f| f.as_str())
        .ok_or_else(|| anyhow!("{from} wrote the bundle outside its exports folder"))?
        .to_owned();
    let total = done.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0);
    set(id, |j| {
        j.step = "pulling".into();
        j.source_file = Some(file.clone());
        j.total = total;
    });
    // 2. Pull it here, piece by piece, into imports.
    let name = format!("transfer-{id}.{EXT}");
    std::fs::create_dir_all(imports_dir(&data_dir))?;
    let out = imports_dir(&data_dir).join(&name);
    let mut f = std::fs::File::create(&out)?;
    let mut offset = 0u64;
    loop {
        let piece = mesh
            .api_call(
                from,
                "export_read",
                "",
                json!({ "file": file, "offset": offset, "len": PIECE }),
                std::time::Duration::from_secs(120),
            )
            .await
            .map_err(|e| anyhow!("reading the bundle from {from}: {e}"))?;
        let bytes =
            aspen_wire::b64::decode(piece.get("data").and_then(|d| d.as_str()).unwrap_or(""))
                .map_err(|e| anyhow!("bad piece from {from}: {e}"))?;
        let size = piece.get("size").and_then(|s| s.as_u64()).unwrap_or(total);
        if bytes.is_empty() && offset < size {
            bail!("{from} sent an empty piece at {offset} of {size}");
        }
        std::io::Write::write_all(&mut f, &bytes)?;
        offset += bytes.len() as u64;
        if offset > size {
            bail!("the bundle from {from} ran past its size");
        }
        set(id, |j| {
            j.bytes = offset;
            j.total = size;
        });
        if piece
            .get("eof")
            .and_then(|e| e.as_bool())
            .unwrap_or(offset >= size)
        {
            break;
        }
    }
    drop(f);
    // 3. Plan it here; the operator confirms (`transfer_install`).
    set(id, |j| {
        j.step = "planning".into();
        j.file = Some(name.clone());
    });
    let mut req = jobs()
        .lock()
        .unwrap()
        .get(id)
        .map(|j| j.import.clone())
        .unwrap_or_default();
    req["file"] = json!(name);
    let inner2 = inner.clone();
    let plan = tokio::task::spawn_blocking(move || {
        crate::repobundle::verb_import_preflight(&inner2, &req)
    })
    .await
    .map_err(|e| anyhow!("{e}"))??;
    set(id, |j| {
        j.step = "ready".into();
        j.plan = Some(plan);
    });
    Ok(())
}

/// A transfer within this node: export into its exports folder, plan the
/// import straight from there. Nothing crosses the mesh; the plan and the
/// confirm are the same.
async fn run_local(inner: &Arc<NodeInner>, id: &str, body: Value) -> Result<()> {
    let data_dir = inner
        .data_dir
        .clone()
        .ok_or_else(|| anyhow!("node has no data dir"))?;
    let mut export = body.clone();
    for k in ["from", "to", "node", "import", "out"] {
        export.as_object_mut().map(|o| o.remove(k));
    }
    let inner2 = inner.clone();
    let done =
        tokio::task::spawn_blocking(move || crate::repobundle::verb_export(&inner2, &export))
            .await
            .map_err(|e| anyhow!("{e}"))??;
    let file = done
        .get("file")
        .and_then(|f| f.as_str())
        .ok_or_else(|| anyhow!("the bundle was written outside the exports folder"))?
        .to_owned();
    let path = exports_dir(&data_dir).join(&file);
    let total = done.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0);
    set(id, |j| {
        j.step = "planning".into();
        j.source_file = Some(file.clone());
        j.file = Some(path.to_string_lossy().into_owned());
        j.bytes = total;
        j.total = total;
    });
    let mut req = jobs()
        .lock()
        .unwrap()
        .get(id)
        .map(|j| j.import.clone())
        .unwrap_or_default();
    req["file"] = json!(path.to_string_lossy());
    let inner2 = inner.clone();
    let plan = tokio::task::spawn_blocking(move || {
        crate::repobundle::verb_import_preflight(&inner2, &req)
    })
    .await
    .map_err(|e| anyhow!("{e}"))??;
    set(id, |j| {
        j.step = "ready".into();
        j.plan = Some(plan);
    });
    Ok(())
}

fn get(id: &str) -> Result<Value> {
    let j = jobs()
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| anyhow!("no such transfer (it may have expired)"))?;
    Ok(serde_json::to_value(j)?)
}

/// Install a planned transfer, then drop the source's copy.
async fn install(inner: &Arc<NodeInner>, id: &str) -> Result<Value> {
    let j = jobs()
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| anyhow!("no such transfer"))?;
    if j.step != "ready" {
        bail!("the transfer is {}, not ready", j.step);
    }
    let staging = j
        .plan
        .as_ref()
        .and_then(|p| p.get("staging_id"))
        .cloned()
        .ok_or_else(|| anyhow!("the plan has no staging id"))?;
    set(id, |j| j.step = "installing".into());
    let mut req = j.import.clone();
    req["file"] = json!(j.file);
    req["staging_id"] = staging;
    let inner2 = inner.clone();
    let report =
        match tokio::task::spawn_blocking(move || crate::repobundle::verb_import(&inner2, &req))
            .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                set(id, |x| {
                    x.step = "failed".into();
                    x.error = Some(format!("{e:#}"));
                });
                return Err(e);
            }
            Err(e) => bail!("{e}"),
        };
    // The source's copy goes (the operator's call); this node keeps its
    // pulled copy in imports, like an upload. Within this node, the export
    // is the only copy, and it goes.
    if let (true, Some(file), Some(dir)) =
        (j.local, j.source_file.as_deref(), inner.data_dir.as_deref())
    {
        if let Ok(p) = export_path(dir, file) {
            let _ = std::fs::remove_file(p);
        }
    } else if let (Some(mesh), Some(file)) = (inner.mesh(), j.source_file.as_deref()) {
        let _ = mesh
            .api_call(
                &j.from,
                "export_delete",
                "",
                json!({ "file": file }),
                std::time::Duration::from_secs(30),
            )
            .await;
    }
    set(id, |x| {
        x.step = "done".into();
        x.report = Some(report.clone());
    });
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_comes_in_pieces_and_resumes() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let a = aspen_wire::b64::encode(b"hello ");
        let b = aspen_wire::b64::encode(b"world");
        assert_eq!(upload_chunk(d, "abc", 0, &a).unwrap()["size"], 6);
        // A repeat of the first piece is refused with where it stands.
        assert!(upload_chunk(d, "abc", 0, &a)
            .unwrap_err()
            .to_string()
            .contains("at 6"));
        assert_eq!(upload_size(d, "abc").unwrap(), 6);
        upload_chunk(d, "abc", 6, &b).unwrap();
        let done = upload_done(d, "abc").unwrap();
        assert_eq!(done["bytes"], 11);
        assert_eq!(
            std::fs::read(imports_dir(d).join(done["file"].as_str().unwrap())).unwrap(),
            b"hello world"
        );
        assert!(part_path(d, "../x").is_err());
    }

    #[test]
    fn an_export_is_read_in_pieces_by_bare_name_only() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::create_dir_all(exports_dir(d)).unwrap();
        std::fs::write(exports_dir(d).join("r.aspen-repo"), b"0123456789").unwrap();
        let p = export_read(d, "r.aspen-repo", 4, 3).unwrap();
        assert_eq!(
            aspen_wire::b64::decode(p["data"].as_str().unwrap()).unwrap(),
            b"456"
        );
        assert_eq!(p["eof"], false);
        let last = export_read(d, "r.aspen-repo", 8, 100).unwrap();
        assert_eq!(last["eof"], true);
        assert!(export_read(d, "../r.aspen-repo", 0, 1).is_err());
        assert!(export_read(d, "notes.txt", 0, 1).is_err());
    }
}
