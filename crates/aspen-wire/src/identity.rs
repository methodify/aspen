//! Mesh root, node identity, certificates, and the enroll/certify/join
//! handshake blobs.
//!
//! The flow (all blobs are paste-able base64 JSON, offline-friendly):
//! 1. `mesh init` — first node creates the root and self-certifies.
//! 2. `mesh enroll` — a new node generates keys and prints a JoinRequest.
//! 3. `mesh certify <blob>` — run where the root key lives; verifies and
//!    prints a NodeCert bundle.
//! 4. `mesh join <blob>` — the new node installs its cert + the root's
//!    public key. From here every peer can verify it with no root contact.

use anyhow::{anyhow, bail, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::b64;

/// What a signature over a cert covers — versioned, order-fixed.
fn cert_signing_bytes(mesh: &str, node: &str, ed_pub: &[u8], x_pub: &[u8]) -> Vec<u8> {
    let mut v = b"aspen-cert-v1\0".to_vec();
    v.extend_from_slice(mesh.as_bytes());
    v.push(0);
    v.extend_from_slice(node.as_bytes());
    v.push(0);
    v.extend_from_slice(ed_pub);
    v.extend_from_slice(x_pub);
    v
}

/// The mesh root: the user-held keypair. Lives only where the user puts it;
/// peers never need it — only its public half rides in certs and configs.
#[derive(Serialize, Deserialize)]
pub struct MeshRoot {
    pub mesh: String,
    #[serde(with = "b64")]
    pub root_secret: Vec<u8>,
    #[serde(with = "b64")]
    pub root_public: Vec<u8>,
}

impl MeshRoot {
    pub fn create(mesh: &str) -> Self {
        let key = SigningKey::generate(&mut rand_core::OsRng);
        Self {
            mesh: mesh.to_owned(),
            root_public: key.verifying_key().to_bytes().to_vec(),
            root_secret: key.to_bytes().to_vec(),
        }
    }

    fn signing_key(&self) -> Result<SigningKey> {
        let bytes: [u8; 32] = self
            .root_secret
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("malformed root secret"))?;
        Ok(SigningKey::from_bytes(&bytes))
    }

    /// Verify an enroll request and mint the node's certificate.
    pub fn certify(&self, req: &JoinRequest) -> Result<NodeCert> {
        if req.node.is_empty()
            || !req
                .node
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("node names are [A-Za-z0-9_-]+");
        }
        let key = self.signing_key()?;
        let msg = cert_signing_bytes(&self.mesh, &req.node, &req.ed_public, &req.x_public);
        let sig = key.sign(&msg);
        Ok(NodeCert {
            mesh: self.mesh.clone(),
            node: req.node.clone(),
            ed_public: req.ed_public.clone(),
            x_public: req.x_public.clone(),
            root_public: self.root_public.clone(),
            root_sig: sig.to_bytes().to_vec(),
        })
    }
}

/// A node's own keys (secret halves stay on the node, mode-0600).
#[derive(Serialize, Deserialize)]
pub struct NodeIdentity {
    pub node: String,
    #[serde(with = "b64")]
    pub ed_secret: Vec<u8>,
    #[serde(with = "b64")]
    pub ed_public: Vec<u8>,
    #[serde(with = "b64")]
    pub x_secret: Vec<u8>,
    #[serde(with = "b64")]
    pub x_public: Vec<u8>,
    /// Present once certified.
    pub cert: Option<NodeCert>,
}

impl NodeIdentity {
    pub fn create(node: &str) -> Self {
        let ed = SigningKey::generate(&mut rand_core::OsRng);
        let x = x25519_dalek::StaticSecret::random_from_rng(rand_core::OsRng);
        let x_pub = x25519_dalek::PublicKey::from(&x);
        Self {
            node: node.to_owned(),
            ed_public: ed.verifying_key().to_bytes().to_vec(),
            ed_secret: ed.to_bytes().to_vec(),
            x_public: x_pub.to_bytes().to_vec(),
            x_secret: x.to_bytes().to_vec(),
            cert: None,
        }
    }

    pub fn join_request(&self) -> JoinRequest {
        JoinRequest {
            node: self.node.clone(),
            ed_public: self.ed_public.clone(),
            x_public: self.x_public.clone(),
        }
    }

    pub fn signing_key(&self) -> Result<SigningKey> {
        let bytes: [u8; 32] = self
            .ed_secret
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("malformed node secret"))?;
        Ok(SigningKey::from_bytes(&bytes))
    }

    /// Sign a relay's challenge nonce, proving key possession at register.
    pub fn sign_relay_challenge(&self, mesh: &str, nonce: &[u8]) -> Result<Vec<u8>> {
        let ctx = crate::relay::challenge_context(mesh, &self.node, nonce);
        Ok(self.signing_key()?.sign(&ctx).to_vec())
    }

    pub fn x_secret_key(&self) -> Result<x25519_dalek::StaticSecret> {
        let bytes: [u8; 32] = self
            .x_secret
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("malformed node x25519 secret"))?;
        Ok(x25519_dalek::StaticSecret::from(bytes))
    }

    /// Install a cert minted for this identity, verifying it actually is.
    pub fn install_cert(&mut self, cert: NodeCert) -> Result<()> {
        if cert.node != self.node {
            bail!("cert names node {:?}, this node is {:?}", cert.node, self.node);
        }
        if cert.ed_public != self.ed_public || cert.x_public != self.x_public {
            bail!("cert covers different keys than this node holds");
        }
        cert.verify_against(&cert.root_public)?;
        self.cert = Some(cert);
        Ok(())
    }
}

/// The enroll blob a new node prints for the root holder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    pub node: String,
    #[serde(with = "b64")]
    pub ed_public: Vec<u8>,
    #[serde(with = "b64")]
    pub x_public: Vec<u8>,
}

/// A root-signed statement binding a node name to its public keys. Any
/// mesh member can verify any other with only the root public key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCert {
    pub mesh: String,
    pub node: String,
    #[serde(with = "b64")]
    pub ed_public: Vec<u8>,
    #[serde(with = "b64")]
    pub x_public: Vec<u8>,
    #[serde(with = "b64")]
    pub root_public: Vec<u8>,
    #[serde(with = "b64")]
    pub root_sig: Vec<u8>,
}

impl NodeCert {
    /// Verify against a trusted root public key — the one from local mesh
    /// config, NEVER the cert's own embedded copy (that one is only a
    /// convenience for the joining node's first install).
    pub fn verify_against(&self, trusted_root_public: &[u8]) -> Result<()> {
        let root_bytes: [u8; 32] = trusted_root_public
            .try_into()
            .map_err(|_| anyhow!("malformed root public key"))?;
        let root = VerifyingKey::from_bytes(&root_bytes).context("root public key")?;
        let sig_bytes: [u8; 64] = self
            .root_sig
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("malformed cert signature"))?;
        let msg = cert_signing_bytes(&self.mesh, &self.node, &self.ed_public, &self.x_public);
        root.verify(&msg, &Signature::from_bytes(&sig_bytes))
            .map_err(|_| anyhow!("certificate signature invalid for this mesh root"))
    }

    pub fn ed_key(&self) -> Result<VerifyingKey> {
        let b: [u8; 32] = self
            .ed_public
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("malformed node public key"))?;
        Ok(VerifyingKey::from_bytes(&b)?)
    }

    pub fn x_key(&self) -> Result<x25519_dalek::PublicKey> {
        let b: [u8; 32] = self
            .x_public
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("malformed node x25519 public key"))?;
        Ok(x25519_dalek::PublicKey::from(b))
    }
}

/// Paste-able blob helpers: `aspen:<tag>:<base64 json>`.
pub fn to_blob<T: Serialize>(tag: &str, v: &T) -> Result<String> {
    Ok(format!(
        "aspen:{tag}:{}",
        b64::encode(serde_json::to_vec(v)?.as_slice())
    ))
}

pub fn from_blob<T: for<'de> Deserialize<'de>>(tag: &str, blob: &str) -> Result<T> {
    let rest = blob
        .trim()
        .strip_prefix(&format!("aspen:{tag}:"))
        .ok_or_else(|| anyhow!("expected an aspen:{tag}:… blob"))?;
    Ok(serde_json::from_slice(&b64::decode(rest)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enroll_certify_join_verify() {
        let root = MeshRoot::create("home");
        let mut node = NodeIdentity::create("laptop");
        let cert = root.certify(&node.join_request()).unwrap();
        node.install_cert(cert.clone()).unwrap();
        // A peer verifies with only the trusted root public key.
        cert.verify_against(&root.root_public).unwrap();
        // A different root's key must fail.
        let other = MeshRoot::create("evil");
        assert!(cert.verify_against(&other.root_public).is_err());
    }

    #[test]
    fn tampered_cert_fails() {
        let root = MeshRoot::create("home");
        let node = NodeIdentity::create("laptop");
        let mut cert = root.certify(&node.join_request()).unwrap();
        cert.node = "impostor".into();
        assert!(cert.verify_against(&root.root_public).is_err());
    }

    #[test]
    fn blob_roundtrip() {
        let node = NodeIdentity::create("laptop");
        let blob = to_blob("enroll", &node.join_request()).unwrap();
        let back: JoinRequest = from_blob("enroll", &blob).unwrap();
        assert_eq!(back.node, "laptop");
        assert!(from_blob::<JoinRequest>("cert", &blob).is_err());
    }
}
