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

/// One push, RFC 8030 + RFC 8291 + RFC 8292, in pure Rust (the `web-push`
/// crate's encryption backend needs OpenSSL, which neither the Windows
/// build nor the glibc-2.28 cross build has):
///
/// 1. VAPID: an ES256 JWT over `{aud: scheme://host, exp: +12h, sub}`,
///    sent as `Authorization: vapid t=<jwt>, k=<public key>`.
/// 2. Content: ECDH between a fresh P-256 key and the subscription's
///    `p256dh`, HKDF with the subscription's `auth` secret into a
///    content key and nonce, AES-128-GCM over the payload plus the
///    0x02 delimiter, framed as one `aes128gcm` record with the salt,
///    record size and our public key in the header.
fn deliver(
    vapid: &Vapid,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
    payload: &str,
) -> std::result::Result<(), Delivery> {
    let failed = |m: String| Delivery::Failed(m);
    let url: url::Url = endpoint
        .parse()
        .map_err(|e| failed(format!("endpoint: {e}")))?;
    let aud = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));

    // 1. VAPID.
    let jwt = {
        use jwt_simple::prelude::*;
        let key_bytes = b64d(&vapid.private_key).map_err(|e| failed(format!("vapid key: {e}")))?;
        let key =
            ES256KeyPair::from_bytes(&key_bytes).map_err(|e| failed(format!("vapid key: {e}")))?;
        let mut custom = std::collections::BTreeMap::<String, serde_json::Value>::new();
        custom.insert(
            "sub".into(),
            serde_json::Value::String("https://github.com/methodify/aspen".into()),
        );
        let claims =
            Claims::with_custom_claims(custom, Duration::from_hours(12)).with_audience(aud);
        key.sign(claims)
            .map_err(|e| failed(format!("vapid sign: {e}")))?
    };
    let authorization = format!("vapid t={jwt}, k={}", vapid.public_key);

    // 2. Content encryption.
    let ua_public = b64d(p256dh).map_err(|e| failed(format!("p256dh: {e}")))?;
    let auth_secret = b64d(auth).map_err(|e| failed(format!("auth: {e}")))?;
    let ua_key =
        p256::PublicKey::from_sec1_bytes(&ua_public).map_err(|e| failed(format!("p256dh: {e}")))?;
    let as_secret = p256::ecdh::EphemeralSecret::random(&mut rand_core::OsRng);
    let as_public = p256::EncodedPoint::from(as_secret.public_key())
        .as_bytes()
        .to_vec();
    let shared = as_secret.diffie_hellman(&ua_key);
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(&ua_public);
    key_info.extend_from_slice(&as_public);
    let mut ikm = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(Some(&auth_secret), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .map_err(|_| failed("hkdf ikm".into()))?;
    let mut salt = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut salt);
    let prk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &ikm);
    let mut cek = [0u8; 16];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| failed("hkdf cek".into()))?;
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| failed("hkdf nonce".into()))?;
    let mut plain = payload.as_bytes().to_vec();
    plain.push(0x02);
    let ciphertext = {
        use aes_gcm::aead::{Aead, KeyInit};
        let cipher =
            aes_gcm::Aes128Gcm::new_from_slice(&cek).map_err(|_| failed("aes key".into()))?;
        cipher
            .encrypt(aes_gcm::Nonce::from_slice(&nonce), plain.as_slice())
            .map_err(|_| failed("aes-gcm".into()))?
    };
    const RS: u32 = 4096;
    let mut body = Vec::with_capacity(16 + 4 + 1 + 65 + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RS.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&ciphertext);
    if body.len() > RS as usize {
        return Err(failed("payload too large for one record".into()));
    }

    let req = ureq::post(endpoint)
        .timeout(std::time::Duration::from_secs(10))
        .set("TTL", "86400")
        .set("Content-Encoding", "aes128gcm")
        .set("Content-Type", "application/octet-stream")
        .set("Authorization", &authorization);
    match req.send_bytes(&body) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(404 | 410, _)) => Err(Delivery::Gone(410)),
        Err(ureq::Error::Status(code, r)) => Err(failed(format!(
            "{code}: {}",
            r.into_string().unwrap_or_default().trim()
        ))),
        Err(e) => Err(failed(e.to_string())),
    }
}

fn b64d(s: &str) -> Result<Vec<u8>> {
    let s = s.trim().trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(s))
        .map_err(|e| anyhow!("{e}"))
}

#[cfg(test)]
mod tests {

    /// The encryption round-trips against a receiver written from the
    /// RFCs, so a push service will open it too.
    #[test]
    fn aes128gcm_round_trip() {
        use aes_gcm::aead::{Aead, KeyInit};
        let ua_secret = p256::ecdh::EphemeralSecret::random(&mut rand_core::OsRng);
        let ua_public = p256::EncodedPoint::from(ua_secret.public_key())
            .as_bytes()
            .to_vec();
        let mut auth = [0u8; 16];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut auth);
        let payload = r#"{"title":"hi"}"#;
        // Sender side, captured instead of posted.
        let as_secret = p256::ecdh::EphemeralSecret::random(&mut rand_core::OsRng);
        let as_public = p256::EncodedPoint::from(as_secret.public_key())
            .as_bytes()
            .to_vec();
        let shared =
            as_secret.diffie_hellman(&p256::PublicKey::from_sec1_bytes(&ua_public).unwrap());
        let mut key_info = b"WebPush: info\0".to_vec();
        key_info.extend_from_slice(&ua_public);
        key_info.extend_from_slice(&as_public);
        let mut ikm = [0u8; 32];
        hkdf::Hkdf::<sha2::Sha256>::new(Some(&auth), shared.raw_secret_bytes())
            .expand(&key_info, &mut ikm)
            .unwrap();
        let salt = [7u8; 16];
        let prk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &ikm);
        let mut cek = [0u8; 16];
        prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
            .unwrap();
        let mut nonce = [0u8; 12];
        prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
            .unwrap();
        let mut plain = payload.as_bytes().to_vec();
        plain.push(0x02);
        let ct = aes_gcm::Aes128Gcm::new_from_slice(&cek)
            .unwrap()
            .encrypt(aes_gcm::Nonce::from_slice(&nonce), plain.as_slice())
            .unwrap();
        // Receiver side: the same derivation from the other key.
        let shared2 =
            ua_secret.diffie_hellman(&p256::PublicKey::from_sec1_bytes(&as_public).unwrap());
        let mut ikm2 = [0u8; 32];
        hkdf::Hkdf::<sha2::Sha256>::new(Some(&auth), shared2.raw_secret_bytes())
            .expand(&key_info, &mut ikm2)
            .unwrap();
        assert_eq!(ikm, ikm2);
        let prk2 = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &ikm2);
        let mut cek2 = [0u8; 16];
        prk2.expand(b"Content-Encoding: aes128gcm\0", &mut cek2)
            .unwrap();
        let mut nonce2 = [0u8; 12];
        prk2.expand(b"Content-Encoding: nonce\0", &mut nonce2)
            .unwrap();
        let pt = aes_gcm::Aes128Gcm::new_from_slice(&cek2)
            .unwrap()
            .decrypt(aes_gcm::Nonce::from_slice(&nonce2), ct.as_slice())
            .unwrap();
        assert_eq!(&pt[..pt.len() - 1], payload.as_bytes());
        assert_eq!(pt[pt.len() - 1], 0x02);
    }
}
