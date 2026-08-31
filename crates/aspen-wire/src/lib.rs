//! aspen-wire — mesh identity and the sealed envelope.
//!
//! Security model (DESIGN.md §8): a mesh is rooted in a user-held keypair;
//! each node holds its own keys, certified by the root; every inter-node
//! envelope is signed by its sender and encrypted to its recipient. The
//! transport (tailnet, LAN, rendezvous relay) is never trusted: a fully
//! compromised relay yields metadata and denial of service, not command
//! and control.
//!
//! Primitives: ed25519 (identity, signatures), x25519 (key agreement),
//! XChaCha20-Poly1305 (payload encryption). Everything serializes as JSON
//! with base64 fields — boring, greppable, versioned.

pub mod envelope;
pub mod identity;
pub mod relay;

pub use envelope::SealedEnvelope;
pub use identity::{JoinRequest, MeshRoot, NodeCert, NodeIdentity};
pub use relay::{Challenge, Register, RelayFrame};

pub mod b64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn encode(bytes: &[u8]) -> String {
        STANDARD.encode(bytes)
    }
    pub fn decode(s: &str) -> anyhow::Result<Vec<u8>> {
        Ok(STANDARD.decode(s)?)
    }

    // serde's with-module contract hands us &Vec here; the slice lint
    // doesn't apply to a signature we don't control.
    #[allow(clippy::ptr_arg)]
    pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        decode(&s).map_err(serde::de::Error::custom)
    }
}
