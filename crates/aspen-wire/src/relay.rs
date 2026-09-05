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
    /// `host` names the node hosting this relay when it is a node (the
    /// embedded relay), so a client reaching the same relay under two
    /// addresses can tell and keep one session.
    Welcome {
        peers: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
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

    // ---- the mailbox (bounded, TTL'd spool at the BUS layer) ----
    // Live links are sessions; a frame from one cannot be replayed into a
    // later session. Bus envelopes are sealed to the recipient's static
    // keys, so they can wait. A node with pending mail for a peer it has no
    // live link to hands the relay a sealed envelope; the relay delivers
    // it when the peer is present (immediately if it already is). The
    // recipient acks through the same path; the origin keeps the row
    // pending until that ack — at-least-once, end to end.
    /// node → relay: keep this for `to` (replacing any item with the same
    /// `id`, so retries don't pile up). `data` is a sealed envelope whose
    /// payload is a bus frame or a bus_ack.
    Store {
        to: String,
        id: String,
        data: String,
    },
    /// relay → node: an envelope that waited (or was handed over while you
    /// were present). Delivered once; a lost one is re-sent by the origin.
    Mail {
        from: String,
        id: String,
        data: String,
    },
    /// relay → node: the mailbox for `to` is full; the origin keeps the row
    /// pending and tries again later.
    MailboxFull { to: String },
}

/// Mailbox limits, shared by every relay implementation.
pub const MAILBOX_MAX_ITEMS: usize = 200;
pub const MAILBOX_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const MAILBOX_TTL_SECS: u64 = 7 * 24 * 3600;

/// The relay-side registration check, shared by every relay host (the
/// standalone `aspen-relay` and the one embedded in each node): the node
/// claims this mesh, its cert is signed by the mesh root, and the challenge
/// signature is by the cert's key.
pub fn verify_register(
    mesh: &str,
    root_pubkey: &[u8],
    reg: &Register,
    nonce: &[u8],
) -> std::result::Result<(), String> {
    if reg.mesh != mesh {
        return Err(format!(
            "wrong mesh: relay serves '{mesh}', node claims '{}'",
            reg.mesh
        ));
    }
    if reg.cert.node != reg.node {
        return Err("cert node name does not match register".into());
    }
    reg.cert
        .verify_against(root_pubkey)
        .map_err(|e| format!("cert not valid for this mesh: {e}"))?;
    let ed = reg
        .cert
        .ed_key()
        .map_err(|e| format!("bad node key: {e}"))?;
    let sig_bytes: [u8; 64] = reg
        .challenge_sig
        .as_slice()
        .try_into()
        .map_err(|_| "malformed challenge signature".to_string())?;
    let ctx = challenge_context(&reg.mesh, &reg.node, nonce);
    use ed25519_dalek::Verifier;
    ed.verify(&ctx, &ed25519_dalek::Signature::from_bytes(&sig_bytes))
        .map_err(|_| "challenge signature invalid".to_string())
}

/// An in-memory mailbox for relay hosts written in Rust (the standalone
/// binary and the one inside every node). Bounded per recipient and by
/// TTL; the Cloudflare port keeps the same limits in Durable Object storage.
#[derive(Default)]
pub struct Mailbox {
    /// recipient → items in arrival order.
    boxes: std::collections::HashMap<String, std::collections::VecDeque<MailItem>>,
}

#[derive(Clone)]
pub struct MailItem {
    pub from: String,
    pub id: String,
    pub data: String,
    pub stored_at: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Mailbox {
    /// Keep an item for `to`; an item with the same id from the same
    /// sender replaces the earlier one. False when the box is full.
    pub fn store(&mut self, to: &str, from: &str, id: &str, data: String) -> bool {
        self.sweep();
        let b = self.boxes.entry(to.to_owned()).or_default();
        b.retain(|m| !(m.from == from && m.id == id));
        let bytes: usize = b.iter().map(|m| m.data.len()).sum();
        if b.len() >= MAILBOX_MAX_ITEMS || bytes + data.len() > MAILBOX_MAX_BYTES {
            return false;
        }
        b.push_back(MailItem {
            from: from.to_owned(),
            id: id.to_owned(),
            data,
            stored_at: now_secs(),
        });
        true
    }

    /// Everything waiting for `to`, oldest first; the box is emptied.
    pub fn drain(&mut self, to: &str) -> Vec<MailItem> {
        self.sweep();
        self.boxes
            .remove(to)
            .map(|b| b.into_iter().collect())
            .unwrap_or_default()
    }

    /// Drop expired items.
    pub fn sweep(&mut self) {
        let cutoff = now_secs().saturating_sub(MAILBOX_TTL_SECS);
        for b in self.boxes.values_mut() {
            b.retain(|m| m.stored_at >= cutoff);
        }
        self.boxes.retain(|_, b| !b.is_empty());
    }

    pub fn waiting(&self) -> std::collections::HashMap<String, usize> {
        self.boxes
            .iter()
            .map(|(k, v)| (k.clone(), v.len()))
            .collect()
    }
}

#[cfg(test)]
mod mailbox_tests {
    use super::*;

    #[test]
    fn store_replace_drain() {
        let mut m = Mailbox::default();
        assert!(m.store("b", "a", "1", "x".into()));
        assert!(m.store("b", "a", "1", "y".into())); // replaces
        assert!(m.store("b", "c", "1", "z".into())); // different sender, same id: kept
        let got = m.drain("b");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].data, "y");
        assert_eq!(got[1].data, "z");
        assert!(m.drain("b").is_empty());
    }

    #[test]
    fn bounded() {
        let mut m = Mailbox::default();
        for i in 0..MAILBOX_MAX_ITEMS {
            assert!(m.store("b", "a", &i.to_string(), "d".into()));
        }
        assert!(!m.store("b", "a", "overflow", "d".into()));
        let mut m2 = Mailbox::default();
        assert!(!m2.store("b", "a", "big", "x".repeat(MAILBOX_MAX_BYTES + 1)));
    }
}
