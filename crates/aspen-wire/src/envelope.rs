//! The sealed envelope: what actually crosses between nodes, over any
//! transport. Signed by the sender's ed25519 key, encrypted to the
//! recipient's x25519 key (XChaCha20-Poly1305 over an x25519 shared
//! secret). The relay sees: version, sender name, recipient name, sizes.

use anyhow::{anyhow, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, Verifier};
use serde::{Deserialize, Serialize};

use crate::b64;
use crate::identity::{NodeCert, NodeIdentity};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealedEnvelope {
    pub v: u8,
    pub from: String,
    pub to: String,
    #[serde(with = "b64")]
    pub nonce: Vec<u8>,
    #[serde(with = "b64")]
    pub ciphertext: Vec<u8>,
    #[serde(with = "b64")]
    pub sig: Vec<u8>,
}

fn signing_bytes(env: &SealedEnvelope) -> Vec<u8> {
    let mut v = b"aspen-env-v1\0".to_vec();
    v.push(env.v);
    v.extend_from_slice(env.from.as_bytes());
    v.push(0);
    v.extend_from_slice(env.to.as_bytes());
    v.push(0);
    v.extend_from_slice(&env.nonce);
    v.extend_from_slice(&env.ciphertext);
    v
}

fn shared_cipher(
    my_x: &x25519_dalek::StaticSecret,
    their_x: &x25519_dalek::PublicKey,
) -> XChaCha20Poly1305 {
    let shared = my_x.diffie_hellman(their_x);
    // The DH output is uniformly random enough for a key here; both sides
    // derive the identical key, direction disambiguated by nonce freshness.
    XChaCha20Poly1305::new(shared.as_bytes().into())
}

impl SealedEnvelope {
    /// Seal `payload` from `me` to the node described by `their_cert`.
    pub fn seal(me: &NodeIdentity, their_cert: &NodeCert, payload: &[u8]) -> Result<Self> {
        let cipher = shared_cipher(&me.x_secret_key()?, &their_cert.x_key()?);
        let mut nonce_bytes = [0u8; 24];
        use rand_core::RngCore;
        rand_core::OsRng.fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce_bytes), payload)
            .map_err(|_| anyhow!("encryption failed"))?;
        let mut env = Self {
            v: 1,
            from: me.node.clone(),
            to: their_cert.node.clone(),
            nonce: nonce_bytes.to_vec(),
            ciphertext,
            sig: Vec::new(),
        };
        env.sig = me.signing_key()?.sign(&signing_bytes(&env)).to_vec();
        Ok(env)
    }

    /// Verify the sender's signature against their cert and decrypt.
    /// `sender_cert` MUST already be root-verified by the caller.
    pub fn open(&self, me: &NodeIdentity, sender_cert: &NodeCert) -> Result<Vec<u8>> {
        if sender_cert.node != self.from {
            return Err(anyhow!(
                "envelope claims sender {:?} but cert names {:?}",
                self.from,
                sender_cert.node
            ));
        }
        let sig_bytes: [u8; 64] = self
            .sig
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("malformed envelope signature"))?;
        sender_cert
            .ed_key()?
            .verify(&signing_bytes(self), &Signature::from_bytes(&sig_bytes))
            .map_err(|_| anyhow!("envelope signature invalid"))?;
        let cipher = shared_cipher(&me.x_secret_key()?, &sender_cert.x_key()?);
        cipher
            .decrypt(XNonce::from_slice(&self.nonce), self.ciphertext.as_ref())
            .map_err(|_| anyhow!("decryption failed (wrong recipient or tampered)"))
            .context("opening envelope")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::MeshRoot;

    fn pair() -> (NodeIdentity, NodeIdentity, MeshRoot) {
        let root = MeshRoot::create("m");
        let mut a = NodeIdentity::create("a");
        let mut b = NodeIdentity::create("b");
        a.install_cert(root.certify(&a.join_request()).unwrap())
            .unwrap();
        b.install_cert(root.certify(&b.join_request()).unwrap())
            .unwrap();
        (a, b, root)
    }

    #[test]
    fn seal_open_roundtrip() {
        let (a, b, _) = pair();
        let env = SealedEnvelope::seal(&a, b.cert.as_ref().unwrap(), b"hello mesh").unwrap();
        let out = env.open(&b, a.cert.as_ref().unwrap()).unwrap();
        assert_eq!(out, b"hello mesh");
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let (a, b, root) = pair();
        let mut c = NodeIdentity::create("c");
        c.install_cert(root.certify(&c.join_request()).unwrap())
            .unwrap();
        let env = SealedEnvelope::seal(&a, b.cert.as_ref().unwrap(), b"secret").unwrap();
        assert!(env.open(&c, a.cert.as_ref().unwrap()).is_err());
    }

    #[test]
    fn tampering_is_detected() {
        let (a, b, _) = pair();
        let mut env = SealedEnvelope::seal(&a, b.cert.as_ref().unwrap(), b"x").unwrap();
        env.from = "impostor".into(); // signature covers from: must fail
        assert!(env.open(&b, a.cert.as_ref().unwrap()).is_err());
        let mut env2 = SealedEnvelope::seal(&a, b.cert.as_ref().unwrap(), b"x").unwrap();
        env2.ciphertext[0] ^= 1;
        assert!(env2.open(&b, a.cert.as_ref().unwrap()).is_err());
    }
}
