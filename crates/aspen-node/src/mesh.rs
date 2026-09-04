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
    /// Rendezvous relay URL (wss://…/relay), the universal fallback for
    /// peers with no direct path. None = direct/tailnet only.
    #[serde(default)]
    pub relay: Option<String>,
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
        let Some(mut mesh) = self.load_mesh()? else {
            bail!("this node has not joined a mesh (run `aspen mesh init` or `aspen mesh join`)");
        };
        cert.verify_against(&mesh.root_public)
            .context("peer cert does not verify against this mesh's root")?;
        if let Some(id) = self.load_identity()? {
            if id.node == cert.node {
                bail!("that cert is this node's own");
            }
        }
        mesh.peers.retain(|p| p.cert.node != cert.node);
        mesh.peers.push(PeerConfig { cert, url });
        self.save_mesh(&mesh)
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
        let removed = mesh.peers.len() != before;
        if removed {
            self.save_mesh(&mesh)?;
        }
        Ok(removed)
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
