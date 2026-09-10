//! Web Push (PROPOSALS-2026-09-D.md §4): notices as OS notifications
//! with the console closed. The node holds a VAPID key pair (generated
//! once, in the data dir); each console that opts in registers its push
//! subscription here with the kinds it wants; when a notice of such a
//! kind is raised the node encrypts it (RFC 8291, `aes128gcm`) and POSTs
//! it to the browser's push service (Mozilla, Google, Apple — plain HTTPS
//! with a VAPID JWT). No third party besides the browser's own service.
//! A subscription the service reports gone (404/410) is dropped.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

use crate::node::NodeInner;

/// Kinds a fresh subscription asks for when it names none: the two that
/// need the operator (decision 2026-09-09: `needs_you` by default).
pub const DEFAULT_KINDS: &[&str] = &["question", "permission"];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Vapid {
    /// Raw 32-byte P-256 private scalar, base64url without padding.
    pub private_key: String,
    /// Uncompressed 65-byte P-256 public point, base64url without padding
    /// — what `pushManager.subscribe` takes as `applicationServerKey`.
    pub public_key: String,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The node's VAPID key pair, made on first use.
pub fn vapid(inner: &Arc<NodeInner>) -> Result<Vapid> {
    let dd = inner
        .data_dir
        .as_deref()
        .ok_or_else(|| anyhow!("no data dir"))?;
    let path = dd.join("push_vapid.json");
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Vapid>(&s) {
            return Ok(v);
        }
    }
    let key = jwt_simple::algorithms::ES256KeyPair::generate();
    // The uncompressed point is the tail of the SubjectPublicKeyInfo DER
    // (the crate's `to_bytes` is the compressed form, which
    // `applicationServerKey` does not accept).
    let der = key
        .public_key()
        .to_der()
        .map_err(|e| anyhow!("public key: {e}"))?;
    if der.len() < 65 {
        return Err(anyhow!("public key DER too short"));
    }
    let uncompressed = &der[der.len() - 65..];
    let v = Vapid {
        private_key: b64url(&key.to_bytes()),
        public_key: b64url(uncompressed),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&v)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(v)
}

/// Fan a notice out to every subscription that asked for its kind. Runs
/// off the caller's thread; failures are logged and, for a subscription
/// the push service says is gone, forgotten.
pub fn send_for_notice(inner: &Arc<NodeInner>, notice: &Value) {
    let kind = notice
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("")
        .to_owned();
    let subs = match inner.store.push_subs() {
        Ok(s) => s,
        Err(_) => return,
    };
    let wanted: Vec<_> = subs
        .into_iter()
        .filter(|s| s.kinds.iter().any(|k| k == "*" || k == &kind))
        .collect();
    if wanted.is_empty() {
        return;
    }
    let Ok(vapid) = vapid(inner) else { return };
    let payload = json!({
        "title": notice.get("title"),
        "body": notice.get("body"),
        "kind": kind,
        "agent": notice.get("agent"),
        "node": notice.get("node"),
        "link": notice.get("link"),
        "ts": notice.get("ts"),
    })
    .to_string();
    for sub in wanted {
        let inner = inner.clone();
        let vapid = vapid.clone();
        let payload = payload.clone();
        tokio::spawn(async move {
            let endpoint = sub.endpoint.clone();
            let r = tokio::task::spawn_blocking(move || {
                deliver(&vapid, &sub.endpoint, &sub.p256dh, &sub.auth, &payload)
            })
            .await;
            match r {
                Ok(Ok(())) => {
                    let _ = inner.store.push_sub_outcome(&endpoint, true, None);
                }
                Ok(Err(Delivery::Gone(code))) => {
                    tracing::info!(%endpoint, code, "push: subscription gone; forgetting it");
                    let _ = inner.store.push_sub_delete(&endpoint);
                }
                Ok(Err(Delivery::Failed(e))) => {
                    tracing::warn!(%endpoint, error = %e, "push: delivery failed");
                    let _ = inner.store.push_sub_outcome(&endpoint, false, Some(&e));
                }
                Err(e) => tracing::warn!(%e, "push: task"),
            }
        });
    }
}

/// A test notification to one endpoint, right now, on the caller's thread.
pub fn send_test(inner: &Arc<NodeInner>, endpoint: &str) -> Result<()> {
    let sub = inner
        .store
        .push_subs()?
        .into_iter()
        .find(|s| s.endpoint == endpoint)
        .ok_or_else(|| anyhow!("no such subscription"))?;
    let vapid = vapid(inner)?;
    let node = inner.mesh().map(|m| m.identity.node.clone());
    let payload = json!({
        "title": "Aspen can reach this device",
        "body": format!("push from {}", node.as_deref().unwrap_or("this node")),
        "kind": "test",
        "node": node,
        "link": "/",
        "ts": crate::store::now_epoch(),
    })
    .to_string();
    match deliver(&vapid, &sub.endpoint, &sub.p256dh, &sub.auth, &payload) {
        Ok(()) => {
            let _ = inner.store.push_sub_outcome(endpoint, true, None);
            Ok(())
        }
        Err(Delivery::Gone(code)) => {
            let _ = inner.store.push_sub_delete(endpoint);
            Err(anyhow!(
                "the push service says this subscription is gone ({code}); subscribe again"
            ))
        }
        Err(Delivery::Failed(e)) => {
            let _ = inner.store.push_sub_outcome(endpoint, false, Some(&e));
            Err(anyhow!("{e}"))
        }
    }
}

enum Delivery {
    Gone(u16),
    Failed(String),
}

fn deliver(
    vapid: &Vapid,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
    payload: &str,
) -> std::result::Result<(), Delivery> {
    use web_push::{
        ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder,
    };
    let info = SubscriptionInfo::new(endpoint, p256dh, auth);
    let mut sig =
        VapidSignatureBuilder::from_base64(&vapid.private_key, base64_013::URL_SAFE_NO_PAD, &info)
            .map_err(|e| Delivery::Failed(format!("vapid key: {e}")))?;
    // A contact the push service may use about this sender: required by
    // some services, so always present.
    sig.add_claim("sub", "https://github.com/methodify/aspen");
    let sig = sig
        .build()
        .map_err(|e| Delivery::Failed(format!("vapid signature: {e}")))?;
    let mut b = WebPushMessageBuilder::new(&info);
    b.set_ttl(24 * 3600);
    b.set_payload(ContentEncoding::Aes128Gcm, payload.as_bytes());
    b.set_vapid_signature(sig);
    let msg = b
        .build()
        .map_err(|e| Delivery::Failed(format!("payload: {e}")))?;
    let url = msg.endpoint.to_string();
    let mut req = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(10))
        .set("TTL", &msg.ttl.to_string());
    let body = match msg.payload {
        Some(p) => {
            req = req
                .set("Content-Encoding", p.content_encoding.to_str())
                .set("Content-Type", "application/octet-stream");
            for (k, v) in &p.crypto_headers {
                req = req.set(k, v);
            }
            p.content
        }
        None => Vec::new(),
    };
    match req.send_bytes(&body) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(404 | 410, _)) => Err(Delivery::Gone(410)),
        Err(ureq::Error::Status(code, r)) => Err(Delivery::Failed(format!(
            "{code}: {}",
            r.into_string().unwrap_or_default().trim()
        ))),
        Err(e) => Err(Delivery::Failed(e.to_string())),
    }
}
