//! The rendezvous relay protocol — the minimal cloud piece.
//!
//! The relay does four things and knows nothing else: authenticate a node
//! to a mesh (verify its cert against the mesh root public key + a
//! challenge signature), route frames between nodes by name, report
//! presence, and (optionally, bounded) spool. It holds only the mesh ROOT
//! PUBLIC key — enough to verify membership, never to forge it — and every
//! routed frame is a `SealedEnvelope` it cannot read. A fully compromised
//! relay yields metadata and denial of service, not command and control.
//!
//! Muxing: nodes reach each other over one relay socket each. Frames carry
//! `to`/`from` node names; on top of that routing, each node pair runs the
//! ordinary end-to-end federation handshake (`run_link`), so the relay is a
//! dumb pipe between two mutually-authenticating peers.

use serde::{Deserialize, Serialize};

use crate::b64;

/// Node → relay, first frame: prove membership + identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    pub mesh: String,
    pub node: String,
    /// The node's root-signed cert (relay verifies against its root pubkey).
    pub cert: crate::identity::NodeCert,
    /// Signature by the node's ed key over the relay's challenge.
    #[serde(with = "b64")]
    pub challenge_sig: Vec<u8>,
}

/// Relay → node on connect, before Register: a nonce to sign.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Challenge {
    #[serde(with = "b64")]
    pub nonce: Vec<u8>,
}

/// The signed-challenge context string — fixed and versioned.
pub fn challenge_context(mesh: &str, node: &str, nonce: &[u8]) -> Vec<u8> {
    let mut v = b"aspen-relay-challenge-v1\0".to_vec();
    v.extend_from_slice(mesh.as_bytes());
    v.push(0);
    v.extend_from_slice(node.as_bytes());
    v.push(0);
    v.extend_from_slice(nonce);
    v
}

/// Frames on an established relay connection, both directions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RelayFrame {
    /// Registration accepted; here is who else is present right now.
    Welcome { peers: Vec<String> },
    /// A peer came online / went offline (presence push).
    Presence { node: String, online: bool },
    /// Route `data` to node `to` (node→relay) / from node `from`
    /// (relay→node). `data` is a serialized federation frame (itself a
    /// sealed envelope after the handshake).
    Route {
        #[serde(skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        data: String,
    },
    /// The relay could not deliver to `to` (offline).
    Undeliverable { to: String },
    /// Registration rejected.
    Rejected { reason: String },
}
