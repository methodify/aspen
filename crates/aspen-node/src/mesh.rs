//! Mesh membership on disk: this node's identity and the peer registry.
//!
//! Certs are public material; secrets stay in identity.json (0600) and —
//! only where the mesh was created — root.key. The root key is the mesh:
//! back it up, and never copy it to nodes that don't need to certify.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use aspen_wire::identity::{MeshRoot, NodeCert, NodeIdentity};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    pub cert: NodeCert,
    /// Dial URL (`ws://host:port/api/federation/ws`), if this node should
    /// dial out to the peer. Peers without URLs are reachable only when
    /// they dial us (or, later, via the rendezvous).
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshConfig {
    pub mesh: String,
    #[serde(with = "aspen_wire::b64")]
    pub root_public: Vec<u8>,
    #[serde(default)]
    pub peers: Vec<PeerConfig>,
    /// Legacy single relay URL (pre-0.7.1). Read into `relays`; kept so
    /// older daemons can still parse the file.
    #[serde(default)]
    pub relay: Option<String>,
    /// Rendezvous relays (wss://…/relay), in preference order. A node keeps
    /// a client on each; a peer is reached through whichever presents it
    /// first. Empty = direct/tailnet only.
    #[serde(default)]
    pub relays: Vec<String>,
    /// What peers of this mesh may do here (docs/MESHES.md): "full" (every
    /// capability, the single-mesh behavior) or "observe" (read only).
    /// None: full for the primary mesh, observe for an additional one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
}

impl MeshConfig {
    pub fn policy_or(&self, default: &str) -> String {
        self.policy.clone().unwrap_or_else(|| default.to_owned())
    }

    /// Every relay this node should sit on, legacy field folded in.
    pub fn relay_urls(&self) -> Vec<String> {
        let mut v = self.relays.clone();
        if let Some(r) = &self.relay {
            if !v.contains(r) {
                v.push(r.clone());
            }
        }
        v
    }
    /// Add a relay (idempotent). Returns whether it was new.
    pub fn add_relay(&mut self, url: &str) -> bool {
        let url = url.trim();
        if url.is_empty() || self.relay_urls().iter().any(|u| u == url) {
            return false;
        }
        self.relays.push(url.to_owned());
        true
    }
    /// Remove a relay (from either field). Returns whether it was present.
    pub fn remove_relay(&mut self, url: &str) -> bool {
        let before = self.relay_urls().len();
        self.relays.retain(|u| u != url);
        if self.relay.as_deref() == Some(url) {
            self.relay = None;
        }
        self.relay_urls().len() != before
    }
}

pub struct MeshFiles {
    pub data_dir: PathBuf,
}

impl MeshFiles {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_owned(),
        }
    }
    fn identity_path(&self) -> PathBuf {
        self.data_dir.join("identity.json")
    }
    fn mesh_path(&self) -> PathBuf {
        self.data_dir.join("mesh.json")
    }
    fn root_path(&self) -> PathBuf {
        self.data_dir.join("root.key")
    }
    fn extras_dir(&self) -> PathBuf {
        self.data_dir.join("meshes")
    }
    fn extra_path(&self, mesh: &str) -> PathBuf {
        self.extras_dir().join(format!("{}.json", mesh.replace(['/', '\\'], "_")))
    }

    /// Additional meshes this node belongs to (docs/MESHES.md), peers
    /// verified against each mesh's own root.
    pub fn load_extra_meshes(&self) -> Result<Vec<MeshConfig>> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.extras_dir()) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Some(mut m) = read_json::<MeshConfig>(&p)? {
                m.peers = verified(&m);
                out.push(m);
            }
        }
        out.sort_by(|a, b| a.mesh.cmp(&b.mesh));
        Ok(out)
    }
    pub fn save_extra_mesh(&self, m: &MeshConfig) -> Result<()> {
        write_json(&self.extra_path(&m.mesh), m)
    }
    pub fn remove_extra_mesh(&self, mesh: &str) -> Result<bool> {
        match std::fs::remove_file(self.extra_path(mesh)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
    /// Every mesh config, the primary first.
    pub fn load_all_meshes(&self) -> Result<Vec<MeshConfig>> {
        let mut v = Vec::new();
        if let Some(mut m) = self.load_mesh()? {
            m.peers = verified(&m);
            v.push(m);
        }
        v.extend(self.load_extra_meshes()?);
        Ok(v)
    }
    /// Every peer name across every mesh, for the uniqueness guard.
    pub fn all_peer_names(&self) -> Result<Vec<String>> {
        Ok(self
            .load_all_meshes()?
            .iter()
            .flat_map(|m| m.peers.iter().map(|p| p.cert.node.clone()))
            .collect())
    }

    pub fn load_identity(&self) -> Result<Option<NodeIdentity>> {
        read_json(&self.identity_path())
    }
    pub fn save_identity(&self, id: &NodeIdentity) -> Result<()> {
        write_json_private(&self.identity_path(), id)
    }
    pub fn load_mesh(&self) -> Result<Option<MeshConfig>> {
        read_json(&self.mesh_path())
    }
    pub fn save_mesh(&self, m: &MeshConfig) -> Result<()> {
        write_json(&self.mesh_path(), m)
    }
    pub fn load_root(&self) -> Result<Option<MeshRoot>> {
        read_json(&self.root_path())
    }
    pub fn save_root(&self, r: &MeshRoot) -> Result<()> {
        write_json_private(&self.root_path(), r)
    }

    /// Verified peer lookup: every cert re-checked against the trusted root
    /// on every load, so a hand-edited mesh.json cannot smuggle a peer in.
    pub fn verified_peers(&self) -> Result<Vec<PeerConfig>> {
        let Some(mesh) = self.load_mesh()? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for p in mesh.peers {
            match p.cert.verify_against(&mesh.root_public) {
                Ok(()) => out.push(p),
                Err(e) => tracing::warn!(peer = %p.cert.node, error = %e,
                    "peer cert failed verification; skipping"),
            }
        }
        Ok(out)
    }

    pub fn add_peer(&self, cert: NodeCert, url: Option<String>) -> Result<()> {
        let Some(primary) = self.load_mesh()? else {
            bail!("this node has not joined a mesh (run `aspen mesh init` or `aspen mesh join`)");
        };
        if let Some(id) = self.load_identity()? {
            if id.node == cert.node {
                bail!("that cert is this node's own");
            }
        }
        if cert.mesh == primary.mesh {
            let mut mesh = primary;
            cert.verify_against(&mesh.root_public)
                .context("peer cert does not verify against this mesh's root")?;
            // A re-record without a URL keeps the one on file.
            let url = url.or_else(|| mesh.peers.iter().find(|p| p.cert.node == cert.node).and_then(|p| p.url.clone()));
            mesh.peers.retain(|p| p.cert.node != cert.node);
            mesh.peers.push(PeerConfig { cert, url });
            return self.save_mesh(&mesh);
        }
        let mut extras = self.load_extra_meshes()?;
        let Some(m) = extras.iter_mut().find(|m| m.mesh == cert.mesh) else {
            bail!(
                "that cert is for mesh '{}', which this node has not joined (this node is in '{}'{})",
                cert.mesh,
                primary.mesh,
                if extras.is_empty() { String::new() } else { format!(" and {}", extras.iter().map(|m| m.mesh.as_str()).collect::<Vec<_>>().join(", ")) }
            );
        };
        cert.verify_against(&m.root_public)
            .context("peer cert does not verify against that mesh's root")?;
        let url = url.or_else(|| m.peers.iter().find(|p| p.cert.node == cert.node).and_then(|p| p.url.clone()));
        m.peers.retain(|p| p.cert.node != cert.node);
        m.peers.push(PeerConfig { cert, url });
        let m = m.clone();
        self.save_extra_mesh(&m)
    }

    /// Forget a peer. Its cert stays valid (only the root can revoke, and
    /// we have no revocation yet) — this node simply stops dialing and
    /// refusing it is up to the link check on the next hello.
    pub fn remove_peer(&self, node: &str) -> Result<bool> {
        let Some(mut mesh) = self.load_mesh()? else {
            bail!("this node has not joined a mesh");
        };
        let before = mesh.peers.len();
        mesh.peers.retain(|p| p.cert.node != node);
        let mut removed = mesh.peers.len() != before;
        if removed {
            self.save_mesh(&mesh)?;
        }
        for mut m in self.load_extra_meshes()? {
            let b = m.peers.len();
            m.peers.retain(|p| p.cert.node != node);
            if m.peers.len() != b {
                self.save_extra_mesh(&m)?;
                removed = true;
            }
        }
        Ok(removed)
    }

    /// Leave an additional mesh: drop its config and cert. The primary
    /// mesh is left with `leave`.
    pub fn leave_extra(&self, mesh: &str) -> Result<String> {
        if self.load_mesh()?.is_some_and(|m| m.mesh == mesh) {
            bail!("'{mesh}' is this node's primary mesh — `aspen mesh leave` (without --mesh) leaves it");
        }
        if !self.remove_extra_mesh(mesh)? {
            bail!("this node is not in a mesh named '{mesh}'");
        }
        if let Some(mut id) = self.load_identity()? {
            id.remove_extra_cert(mesh);
            self.save_identity(&id)?;
        }
        Ok(format!("left mesh '{mesh}'; its cert and peers dropped"))
    }

    /// Set what peers of a mesh may do here.
    pub fn set_policy(&self, mesh: &str, policy: &str) -> Result<()> {
        if !matches!(policy, "full" | "observe") {
            bail!("policy must be full|observe, not {policy:?}");
        }
        if let Some(mut m) = self.load_mesh()? {
            if m.mesh == mesh {
                m.policy = Some(policy.to_owned());
                return self.save_mesh(&m);
            }
        }
        for mut m in self.load_extra_meshes()? {
            if m.mesh == mesh {
                m.policy = Some(policy.to_owned());
                return self.save_extra_mesh(&m);
            }
        }
        bail!("this node is not in a mesh named '{mesh}'")
    }

    /// Leave the mesh: drop mesh.json and this node's cert (the keypair is
    /// kept so a re-enroll keeps the same identity). The root key is left
    /// alone unless `discard_root`, since deleting it ends the mesh for
    /// everyone who was certified by it.
    pub fn leave(&self, discard_root: bool) -> Result<String> {
        let mesh = self
            .load_mesh()?
            .ok_or_else(|| anyhow::anyhow!("this node is not in a mesh"))?;
        let has_root = self.load_root()?.is_some();
        if has_root && !discard_root {
            bail!(
                "this node holds the ROOT KEY of mesh '{}' — leaving would orphan every node it certified. Move the mesh elsewhere first, or pass --discard-root to end it.",
                mesh.mesh
            );
        }
        if let Some(mut id) = self.load_identity()? {
            id.cert = None;
            self.save_identity(&id)?;
        }
        std::fs::remove_file(self.mesh_path())?;
        if has_root {
            std::fs::remove_file(self.root_path())?;
        }
        Ok(format!(
            "left mesh '{}'{}; identity keys kept for a future enroll",
            mesh.mesh,
            if has_root {
                " and discarded its root key"
            } else {
                ""
            }
        ))
    }
}

fn verified(m: &MeshConfig) -> Vec<PeerConfig> {
    let mut out = Vec::new();
    for p in &m.peers {
        match p.cert.verify_against(&m.root_public) {
            Ok(()) => out.push(p.clone()),
            Err(e) => tracing::warn!(peer = %p.cert.node, mesh = %m.mesh, error = %e,
                "peer cert failed verification; skipping"),
        }
    }
    out
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(
            serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).ok();
    }
    std::fs::write(path, serde_json::to_string_pretty(v)?)
        .with_context(|| format!("writing {}", path.display()))
}

fn write_json_private<T: Serialize>(path: &Path, v: &T) -> Result<()> {
    write_json(path, v)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// What `aspen mesh certify` hands back to a joining node: its own cert plus
/// enough about the certifier to wire the first link without a second
/// round of copy-paste — the certifier's cert, how to dial it (if the
/// operator said), and the mesh's relay (if any).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JoinBundle {
    /// The joining node's root-signed cert.
    pub cert: NodeCert,
    /// The certifying node's cert, so the joiner can register it as a peer.
    pub certifier: NodeCert,
    /// How the joiner should dial the certifier (ws://…/api/federation/ws).
    /// None = the certifier will dial the joiner, or a relay carries it.
    #[serde(default)]
    pub certifier_url: Option<String>,
    /// The mesh relay, if the certifier has one configured.
    #[serde(default)]
    pub relay: Option<String>,
}
