//! The mesh ceremony steps as plain functions, so both the `aspen mesh …`
//! subcommands and `aspen mesh apply` (executing console-authored
//! proposals) run exactly the same code. Each returns a human summary and,
//! where the step produces a public artifact (enroll blob, join bundle),
//! that artifact — for the console to show and deep-link onward.

use anyhow::{anyhow, bail, Result};

use aspen_node::mesh::{JoinBundle, MeshConfig, MeshFiles};
use aspen_wire::identity::{self, JoinRequest, MeshRoot, NodeCert, NodeIdentity};

pub struct Done {
    pub summary: String,
    pub artifact: Option<String>,
}

fn done(summary: impl Into<String>, artifact: Option<String>) -> Done {
    Done {
        summary: summary.into(),
        artifact,
    }
}

/// Create a mesh here: a new root key, and this node certified under it.
/// An uncertified identity left by an earlier `enroll` is reused (same
/// keypair, its name), so changing one's mind costs nothing.
pub fn init(files: &MeshFiles, mesh: &str, node_name: &str) -> Result<Done> {
    if files.load_mesh()?.is_some() {
        bail!("this node already belongs to a mesh (see `aspen mesh status`)");
    }
    let mesh = mesh.trim();
    if mesh.is_empty()
        || !mesh
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("mesh name must be letters, digits, - or _ (got {mesh:?})");
    }
    let root = MeshRoot::create(mesh);
    let mut id = match files.load_identity()? {
        Some(existing) if existing.cert.is_some() => bail!(
            "this node already has a certified identity ('{}')",
            existing.node
        ),
        Some(mut existing) => {
            if !node_name.is_empty() && existing.node != node_name {
                existing.node = node_name.to_owned();
            }
            existing
        }
        None => NodeIdentity::create(node_name),
    };
    let cert = root.certify(&id.join_request())?;
    id.install_cert(cert)?;
    files.save_root(&root)?;
    files.save_identity(&id)?;
    files.save_mesh(&MeshConfig {
        mesh: mesh.to_owned(),
        root_public: root.root_public.clone(),
        peers: vec![],
        relay: None,
    })?;
    let blob = identity::to_blob("cert", id.cert.as_ref().unwrap())?;
    Ok(done(
        format!(
            "mesh '{mesh}' created; this node is '{}' and holds the ROOT KEY ({}) — back it up",
            id.node,
            files.data_dir.join("root.key").display()
        ),
        Some(blob),
    ))
}

/// Forget a peer.
pub fn peers_remove(files: &MeshFiles, node: &str) -> Result<Done> {
    if files.remove_peer(node)? {
        Ok(done(
            format!("peer '{node}' removed from this node's mesh config"),
            None,
        ))
    } else {
        bail!("no peer named '{node}'")
    }
}

/// Leave the mesh (see MeshFiles::leave).
pub fn leave(files: &MeshFiles, discard_root: bool) -> Result<Done> {
    Ok(done(files.leave(discard_root)?, None))
}

/// Generate this node's identity (or reuse an uncertified one) and hand
/// back the enroll blob for the root holder.
pub fn enroll(files: &MeshFiles, node_name: &str) -> Result<Done> {
    let id = match files.load_identity()? {
        Some(existing) if existing.cert.is_some() => {
            bail!(
                "this node already has a certified identity ('{}') — `aspen mesh leave` first to re-enroll",
                existing.node
            )
        }
        // An uncertified identity keeps its keypair but takes the name
        // asked for: the name is not part of the keys, and a mistyped one
        // is the common reason to enroll again.
        Some(mut existing) => {
            if existing.node != node_name {
                existing.node = node_name.to_owned();
                files.save_identity(&existing)?;
            }
            existing
        }
        None => {
            let id = NodeIdentity::create(node_name);
            files.save_identity(&id)?;
            id
        }
    };
    let blob = identity::to_blob("enroll", &id.join_request())?;
    Ok(done(
        format!(
            "enroll blob for '{}' — run `aspen mesh certify <blob>` where the root key lives",
            id.node
        ),
        Some(blob),
    ))
}

/// Turn an enroll blob into a join bundle (needs the root key here).
pub fn certify(files: &MeshFiles, blob: &str, url: Option<&str>) -> Result<Done> {
    let root = files
        .load_root()?
        .ok_or_else(|| anyhow!("no root key here — run this on the mesh's root node"))?;
    let req: JoinRequest = identity::from_blob("enroll", blob)?;
    // Names route the mesh; a duplicate would make two nodes
    // indistinguishable (the Windows+WSL hostname trap).
    if files.load_identity()?.is_some_and(|id| id.node == req.node) {
        bail!(
            "'{}' is THIS node's name — re-enroll the other node with a distinct one: aspen mesh enroll --node <name>",
            req.node
        );
    }
    if files
        .verified_peers()?
        .iter()
        .any(|p| p.cert.node == req.node)
    {
        bail!(
            "a peer named '{}' already exists in this mesh — re-enroll with a distinct name",
            req.node
        );
    }
    let cert = root.certify(&req)?;
    files.add_peer(cert.clone(), None).ok(); // register them here too (inbound-only)
    let me = files
        .load_identity()?
        .and_then(|id| id.cert)
        .ok_or_else(|| anyhow!("this node has no cert of its own"))?;
    // The mesh's relay if one is configured; otherwise, since every node
    // hosts a relay at /api/federation/relay, offer THIS node's — it is the
    // one reachable at `url`, and joiners that only dial out (loopback
    // listeners) can then reach each other through it.
    let relay = files
        .load_mesh()?
        .and_then(|m| m.relay)
        .or_else(|| url.map(|u| u.replacen("/api/federation/ws", "/api/federation/relay", 1)));
    let bundle = JoinBundle {
        cert: cert.clone(),
        certifier: me,
        certifier_url: url.map(str::to_owned),
        relay: relay.clone(),
    };
    let blob = identity::to_blob("bundle", &bundle)?;
    Ok(done(
        format!(
            "certified '{}'; peer registered here as inbound-only. The bundle carries this node's cert{}{} — run `aspen mesh join <bundle>` on '{}'",
            cert.node,
            if url.is_some() { " + dial URL" } else { " (no dial URL — the joiner will be inbound-only unless you pass --url)" },
            if relay.is_some() { " + relay (this node's, so joiners reach each other through it)" } else { "" },
            cert.node,
        ),
        Some(blob),
    ))
}

/// Install a join bundle (or bare cert) on this node.
pub fn join(files: &MeshFiles, blob: &str) -> Result<Done> {
    let bundle: Option<JoinBundle> = identity::from_blob("bundle", blob).ok();
    let cert: NodeCert = match &bundle {
        Some(b) => b.cert.clone(),
        None => identity::from_blob("cert", blob)?,
    };
    let mut id = files
        .load_identity()?
        .ok_or_else(|| anyhow!("no identity here — run `aspen mesh enroll` first"))?;
    id.install_cert(cert.clone())?;
    files.save_identity(&id)?;
    if files.load_mesh()?.is_none() {
        // First join: trust the root key this cert carries (verified against
        // itself at install; the operator carried the blob).
        files.save_mesh(&MeshConfig {
            mesh: cert.mesh.clone(),
            root_public: cert.root_public.clone(),
            peers: vec![],
            relay: None,
        })?;
    }
    let mut summary = format!("joined mesh '{}' as node '{}'", cert.mesh, cert.node);
    if let Some(b) = bundle {
        files.add_peer(b.certifier.clone(), b.certifier_url.clone())?;
        summary.push_str(&format!(
            "; peer '{}' registered{}",
            b.certifier.node,
            match &b.certifier_url {
                Some(u) => format!(" — dialing {u}"),
                None => " — inbound only (it dials us, or a relay carries it)".into(),
            }
        ));
        if let Some(relay) = b.relay {
            let mut m = files.load_mesh()?.expect("saved above");
            if m.relay.is_none() {
                m.relay = Some(relay.clone());
                files.save_mesh(&m)?;
                summary.push_str(&format!("; relay set from bundle: {relay}"));
            }
        }
    } else {
        summary.push_str(" (bare cert: add peers with `aspen mesh peers-add`)");
    }
    Ok(done(summary, None))
}

pub fn peers_add(files: &MeshFiles, blob: &str, url: Option<&str>) -> Result<Done> {
    let cert: NodeCert = identity::from_blob("cert", blob)?;
    if files
        .load_identity()?
        .is_some_and(|id| id.node == cert.node)
    {
        bail!(
            "that cert names '{}' — this node's own name. Two nodes can't share a name.",
            cert.node
        );
    }
    files.add_peer(cert.clone(), url.map(str::to_owned))?;
    Ok(done(
        format!(
            "peer '{}' registered{}",
            cert.node,
            url.map(|u| format!(" (dialing {u})")).unwrap_or_default()
        ),
        None,
    ))
}

pub fn relay(files: &MeshFiles, url: Option<&str>) -> Result<Done> {
    let mut mesh = files
        .load_mesh()?
        .ok_or_else(|| anyhow!("this node has not joined a mesh"))?;
    mesh.relay = url.map(str::to_owned);
    files.save_mesh(&mesh)?;
    Ok(done(
        match url {
            Some(u) => format!("relay set: {u}"),
            None => "relay cleared (takes effect at next daemon start)".into(),
        },
        None,
    ))
}
