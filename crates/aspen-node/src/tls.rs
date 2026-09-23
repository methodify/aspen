//! TLS for nodes (docs/TLS.md; PROPOSALS-2026-09-M.md).
//!
//! The mesh root holder keeps a P-256 X.509 certificate authority beside
//! `root.key` (`tls-ca.key`, `tls-ca.crt`), vouched for by the Ed25519 root
//! (`TlsCa.root_sig`) and carried to every member in rosters. Each node
//! keeps its own P-256 key (`tls.key`, never leaves) and asks the root for
//! a 90-day leaf covering every name it can claim — its hostname, `.local`
//! form, private interface addresses, advertise hosts, `aspen config
//! tls-names` — over the federation link (`tls_csr`), or by paste-able
//! blobs when the root is not online (`aspen tls request` / `sign` /
//! `install`). The leaf plus the CA (`tls.crt`) feed a live rustls
//! resolver, so a renewal needs no restart.
//!
//! The CA is name-constrained to private IP space: no mesh certificate can
//! validate for a public address, whatever the CA key's fate. DNS names
//! are unconstrained by default (members' hostnames are arbitrary single
//! labels) — `aspen tls ca --dns-suffix` at creation adds a DNS constraint.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{anyhow, bail, Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, CidrSubnet,
    DistinguishedName, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, NameConstraints, PublicKeyData as _, SanType,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use aspen_wire::identity::TlsCa;

use crate::mesh::MeshFiles;
use crate::node::NodeInner;

/// Leaf validity, and how long before expiry a renewal starts.
pub const LEAF_DAYS: i64 = 90;
pub const RENEW_BEFORE_DAYS: i64 = 30;
/// CA validity.
pub const CA_YEARS: i64 = 10;

// ------------------------------------------------------------------ files

pub fn ca_key_path(d: &Path) -> PathBuf {
    d.join("tls-ca.key")
}
pub fn ca_cert_path(d: &Path) -> PathBuf {
    d.join("tls-ca.crt")
}
pub fn leaf_key_path(d: &Path) -> PathBuf {
    d.join("tls.key")
}
pub fn leaf_cert_path(d: &Path) -> PathBuf {
    d.join("tls.crt")
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn read_opt(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

// ------------------------------------------------------------------ names

/// Is this an address a mesh certificate may cover: private (RFC 1918),
/// CGNAT (Tailscale's 100.64/10), link-local v4, ULA v6 or link-local v6?
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_private() || a.is_link_local() || (o[0] == 100 && (o[1] & 0xC0) == 64)
        }
        IpAddr::V6(a) => {
            let s = a.segments()[0];
            (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
    }
}

/// A syntactically valid hostname: labels of [A-Za-z0-9-], dots between.
pub fn is_hostname(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// One name a leaf may carry, checked and normalized; None if it is
/// nothing a mesh certificate can cover (a public address, junk).
fn admissible(name: &str) -> Option<String> {
    let n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if n.is_empty() {
        return None;
    }
    if let Ok(ip) = n.parse::<IpAddr>() {
        return is_private_ip(ip).then(|| ip.to_string());
    }
    if n == "localhost" {
        return None;
    }
    is_hostname(&n).then_some(n)
}

/// Every name this node can claim for its leaf (docs/TLS.md §2): the
/// hostname and its `.local` form, private interface addresses, the hosts
/// of `aspen config advertise` URLs, and `aspen config tls-names`.
pub fn leaf_names(inner: &NodeInner) -> Vec<String> {
    leaf_names_for(inner.data_dir.as_deref())
}

/// The same, from a data dir alone (the CLI's offline `aspen tls request`
/// must not open the node).
pub fn leaf_names_for(data_dir: Option<&Path>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        if let Some(n) = admissible(s) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    };
    if let Some(h) = crate::federation::hostname() {
        push(&h);
        if !h.contains('.') {
            push(&format!("{h}.local"));
        }
    }
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if i.is_loopback() {
                continue;
            }
            match i.ip() {
                IpAddr::V4(v4) if !v4.is_link_local() => push(&v4.to_string()),
                IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00 => push(&v6.to_string()),
                _ => {}
            }
        }
    }
    if let Some(dir) = data_dir {
        let settings = crate::settings::load(dir);
        for u in settings.advertise_urls() {
            if let Ok(url) = url::Url::parse(&u) {
                if let Some(h) = url.host_str() {
                    push(h.trim_matches(|c| c == '[' || c == ']'));
                }
            }
        }
        for n in settings.tls_names() {
            push(&n);
        }
    }
    out.sort();
    out
}

// --------------------------------------------------------------------- CA

fn now() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc()
}

/// The name constraints every mesh CA carries: private IP space only;
/// DNS unconstrained unless suffixes are given.
fn constraints(dns_suffixes: &[String]) -> NameConstraints {
    let mut permitted = vec![
        GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([10, 0, 0, 0], 8)),
        GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([172, 16, 0, 0], 12)),
        GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([192, 168, 0, 0], 16)),
        GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([100, 64, 0, 0], 10)),
        GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([169, 254, 0, 0], 16)),
        GeneralSubtree::IpAddress(CidrSubnet::from_v6_prefix(
            [0xfc, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            7,
        )),
        GeneralSubtree::IpAddress(CidrSubnet::from_v6_prefix(
            [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            10,
        )),
    ];
    for s in dns_suffixes {
        let s = s.trim().trim_start_matches('.').to_ascii_lowercase();
        if !s.is_empty() {
            permitted.push(GeneralSubtree::DnsName(s));
        }
    }
    NameConstraints {
        permitted_subtrees: permitted,
        excluded_subtrees: Vec::new(),
    }
}

/// The CA's key and certificate PEM, if this node holds them.
pub fn ca_load(d: &Path) -> Result<Option<(KeyPair, String)>> {
    let (Some(key), Some(crt)) = (read_opt(&ca_key_path(d))?, read_opt(&ca_cert_path(d))?) else {
        return Ok(None);
    };
    let kp = KeyPair::from_pem(&key).context("parsing tls-ca.key")?;
    Ok(Some((kp, crt)))
}

fn pem_to_der(pem: &str) -> Result<Vec<u8>> {
    Ok(CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| anyhow!("parsing certificate PEM: {e}"))?
        .to_vec())
}

fn der_to_pem(der: &[u8]) -> String {
    pem_encode("CERTIFICATE", der)
}

fn pem_encode(label: &str, der: &[u8]) -> String {
    let b = aspen_wire::b64::encode(der);
    let mut s = format!("-----BEGIN {label}-----\n");
    for chunk in b.as_bytes().chunks(64) {
        s.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        s.push('\n');
    }
    s.push_str(&format!("-----END {label}-----\n"));
    s
}

/// Make sure the mesh CA exists here (root holders only) and is recorded
/// in mesh.json with the root's signature; mint it when missing.
pub fn ca_ensure(files: &MeshFiles, dns_suffixes: &[String]) -> Result<TlsCa> {
    let root = files
        .load_root()?
        .ok_or_else(|| anyhow!("no root key here — the TLS CA lives with the mesh root"))?;
    let d = files.data_dir.as_path();
    let der = match ca_load(d)? {
        Some((_, pem)) => pem_to_der(&pem)?,
        None => {
            let (key, pem) = mint_ca(&root.mesh, dns_suffixes)?;
            write_private(&ca_key_path(d), &key.serialize_pem())?;
            std::fs::write(ca_cert_path(d), &pem)?;
            tracing::info!(mesh = %root.mesh, "minted the mesh TLS CA");
            pem_to_der(&pem)?
        }
    };
    if let Some(existing) = files.tls_ca(&root.mesh)? {
        if existing.der == der && existing.verify_against(&root.root_public).is_ok() {
            return Ok(existing);
        }
    }
    let ca = root.sign_tls_ca(&der)?;
    files.set_tls_ca(&ca)?;
    Ok(ca)
}

/// Replace the CA: a new key and certificate; every leaf is re-issued on
/// its next check and every computer must trust the new root.
pub fn ca_rotate(files: &MeshFiles, dns_suffixes: &[String]) -> Result<TlsCa> {
    let d = files.data_dir.as_path();
    for p in [ca_key_path(d), ca_cert_path(d), leaf_cert_path(d)] {
        match std::fs::remove_file(&p) {
            Ok(()) | Err(_) => {}
        }
    }
    ca_ensure(files, dns_suffixes)
}

fn mint_ca(mesh: &str, dns_suffixes: &[String]) -> Result<(KeyPair, String)> {
    let key = KeyPair::generate().context("generating the CA key")?;
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, format!("Aspen mesh {mesh}"));
    dn.push(DnType::OrganizationName, "Aspen");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.not_before = now() - time::Duration::days(1);
    params.not_after = now() + time::Duration::days(365 * CA_YEARS + 2);
    params.name_constraints = Some(constraints(dns_suffixes));
    let cert = params
        .self_signed(&key)
        .context("signing the CA certificate")?;
    Ok((key, cert.pem()))
}

// ------------------------------------------------------------------- leaf

/// This node's leaf key, created on first use.
fn leaf_key(d: &Path) -> Result<KeyPair> {
    if let Some(pem) = read_opt(&leaf_key_path(d))? {
        return KeyPair::from_pem(&pem).context("parsing tls.key");
    }
    let key = KeyPair::generate().context("generating the node's TLS key")?;
    write_private(&leaf_key_path(d), &key.serialize_pem())?;
    Ok(key)
}

/// A certificate signing request for `names`, signed by the node's key.
pub fn build_csr(d: &Path, node: &str, names: &[String]) -> Result<Vec<u8>> {
    if names.is_empty() {
        bail!("no names to certify (no hostname, private address, advertise host or tls-names)");
    }
    let key = leaf_key(d)?;
    let mut params = CertificateParams::new(names.to_vec()).context("leaf names")?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, node);
    dn.push(DnType::OrganizationName, "Aspen");
    params.distinguished_name = dn;
    let csr = params.serialize_request(&key).context("building the CSR")?;
    Ok(csr.der().to_vec())
}

/// What a signed leaf covers, for the trail and the operator.
pub fn csr_names(csr_der: &[u8]) -> Result<Vec<String>> {
    let req = CertificateSigningRequestParams::from_der(&csr_der.to_vec().into())
        .map_err(|e| anyhow!("parsing the CSR: {e}"))?;
    Ok(san_strings(&req.params.subject_alt_names))
}

fn san_strings(sans: &[SanType]) -> Vec<String> {
    sans.iter()
        .filter_map(|s| match s {
            SanType::DnsName(n) => Some(n.as_str().to_owned()),
            SanType::IpAddress(ip) => Some(ip.to_string()),
            _ => None,
        })
        .collect()
}

/// Sign a member's CSR with the mesh CA (root holders only): every name
/// must be admissible — a hostname or a private address — or the request
/// is refused whole. Returns the leaf DER.
pub fn sign_csr(files: &MeshFiles, csr_der: &[u8]) -> Result<Vec<u8>> {
    let d = files.data_dir.as_path();
    let ca = ca_ensure(files, &[])?;
    let (ca_key, ca_pem) = ca_load(d)?.ok_or_else(|| anyhow!("no TLS CA here"))?;
    let _ = ca;
    let issuer = Issuer::from_ca_cert_pem(&ca_pem, ca_key)
        .map_err(|e| anyhow!("loading the CA certificate: {e}"))?;
    let mut req = CertificateSigningRequestParams::from_der(&csr_der.to_vec().into())
        .map_err(|e| anyhow!("parsing the CSR: {e}"))?;
    let names = san_strings(&req.params.subject_alt_names);
    if names.is_empty() {
        bail!("the CSR names nothing");
    }
    for n in &names {
        if admissible(n).as_deref() != Some(n.as_str()) {
            bail!("refusing to certify {n:?}: not a hostname or a private address");
        }
    }
    let p = &mut req.params;
    p.is_ca = IsCa::NoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.use_authority_key_identifier_extension = true;
    p.not_before = now() - time::Duration::hours(1);
    p.not_after = now() + time::Duration::days(LEAF_DAYS);
    p.name_constraints = None;
    p.custom_extensions.clear();
    let cert = req
        .signed_by(&issuer)
        .map_err(|e| anyhow!("signing the leaf: {e}"))?;
    Ok(cert.der().to_vec())
}

/// Install a signed leaf and its CA as this node's chain (`tls.crt`).
pub fn install_leaf(d: &Path, leaf_der: &[u8], ca_der: &[u8]) -> Result<()> {
    if read_opt(&leaf_key_path(d))?.is_none() {
        bail!("no tls.key here — this leaf was requested from another node");
    }
    // The leaf must match our key: reject a chain for someone else early.
    let (_, x) = x509_parser::parse_x509_certificate(leaf_der)
        .map_err(|e| anyhow!("parsing the leaf: {e}"))?;
    let key = leaf_key(d)?;
    if x.public_key().raw != key.subject_public_key_info().as_slice() {
        bail!("the leaf was issued for a different key than this node's tls.key");
    }
    let mut pem = der_to_pem(leaf_der);
    pem.push_str(&der_to_pem(ca_der));
    std::fs::write(leaf_cert_path(d), pem).context("writing tls.crt")?;
    Ok(())
}

/// What the installed leaf says.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeafInfo {
    pub names: Vec<String>,
    pub not_before: i64,
    pub not_after: i64,
    pub fingerprint: String,
    pub issuer: String,
}

pub fn leaf_info(d: &Path) -> Result<Option<LeafInfo>> {
    let Some(pem) = read_opt(&leaf_cert_path(d))? else {
        return Ok(None);
    };
    let ders: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| anyhow!("parsing tls.crt: {e}"))?;
    let Some(leaf) = ders.first() else {
        return Ok(None);
    };
    cert_info(leaf).map(Some)
}

fn cert_info(der: &[u8]) -> Result<LeafInfo> {
    use sha2::Digest as _;
    let (_, x) = x509_parser::parse_x509_certificate(der)
        .map_err(|e| anyhow!("parsing certificate: {e}"))?;
    let mut names = Vec::new();
    if let Ok(Some(san)) = x.subject_alternative_name() {
        for g in &san.value.general_names {
            match g {
                x509_parser::extensions::GeneralName::DNSName(n) => names.push(n.to_string()),
                x509_parser::extensions::GeneralName::IPAddress(b) => match b.len() {
                    4 => names.push(std::net::Ipv4Addr::new(b[0], b[1], b[2], b[3]).to_string()),
                    16 => {
                        let mut o = [0u8; 16];
                        o.copy_from_slice(b);
                        names.push(std::net::Ipv6Addr::from(o).to_string());
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    names.sort();
    Ok(LeafInfo {
        names,
        not_before: x.validity().not_before.timestamp(),
        not_after: x.validity().not_after.timestamp(),
        fingerprint: sha2::Sha256::digest(der)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
        issuer: x.issuer().to_string(),
    })
}

/// The CA certificate's validity end, from its DER.
pub fn ca_not_after(der: &[u8]) -> Option<i64> {
    x509_parser::parse_x509_certificate(der)
        .ok()
        .map(|(_, x)| x.validity().not_after.timestamp())
}

// ----------------------------------------------------------------- serving

/// The live server certificate: swapped on renewal, read per handshake.
#[derive(Debug, Default)]
pub struct Resolver {
    key: RwLock<Option<Arc<rustls::sign::CertifiedKey>>>,
}

impl rustls::server::ResolvesServerCert for Resolver {
    fn resolve(
        &self,
        _hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.key.read().unwrap().clone()
    }
}

impl Resolver {
    pub fn has_cert(&self) -> bool {
        self.key.read().unwrap().is_some()
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    static P: std::sync::OnceLock<Arc<rustls::crypto::CryptoProvider>> = std::sync::OnceLock::new();
    P.get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// A rustls server config over the live resolver. HTTP/1.1 only: the
/// console's WebSocket upgrades need it, and HTTP/2 buys nothing here.
pub fn server_config(resolver: Arc<Resolver>) -> Result<Arc<rustls::ServerConfig>> {
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .context("rustls protocol versions")?
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

/// (Re)load `tls.key` + `tls.crt` into the resolver. Ok(false): no chain
/// on disk yet.
pub fn reload_resolver(d: &Path, resolver: &Resolver) -> Result<bool> {
    let (Some(key), Some(crt)) = (read_opt(&leaf_key_path(d))?, read_opt(&leaf_cert_path(d))?)
    else {
        return Ok(false);
    };
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(crt.as_bytes())
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| anyhow!("parsing tls.crt: {e}"))?;
    if chain.is_empty() {
        return Ok(false);
    }
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes())
        .map_err(|e| anyhow!("parsing tls.key: {e}"))?;
    let signing = provider()
        .key_provider
        .load_private_key(key)
        .map_err(|e| anyhow!("loading tls.key: {e}"))?;
    let ck = rustls::sign::CertifiedKey::new(chain, signing);
    *resolver.key.write().unwrap() = Some(Arc::new(ck));
    Ok(true)
}

// ------------------------------------------------------------------ state

/// Runtime TLS state on the node.
#[derive(Debug, Default)]
pub struct TlsState {
    pub resolver: Arc<Resolver>,
    /// The port that serves https, once the listener is bound: Some when a
    /// listener beyond loopback exists (a leaf is wanted), None when every
    /// listener is loopback-only (nothing to certify).
    pub https_port: std::sync::OnceLock<Option<u16>>,
    /// Whether https is on the main port (sniffed) or a separate one.
    pub separate_port: std::sync::atomic::AtomicBool,
    pub last_error: Mutex<Option<String>>,
    pub last_attempt: Mutex<Option<f64>>,
    pub last_ok: Mutex<Option<f64>>,
    /// Epoch before which the loop does not try again.
    pub next_try: Mutex<f64>,
    /// Trust-store probe cache: (when, stores) — the probes run tools.
    pub stores_cache: Mutex<Option<(f64, Vec<crate::truststore::Store>)>>,
}

impl TlsState {
    pub fn https_port(&self) -> Option<u16> {
        self.https_port.get().copied().flatten()
    }
}

/// Does the installed leaf need replacing for `names`?
fn needs_renewal(d: &Path, names: &[String]) -> (bool, &'static str) {
    match leaf_info(d) {
        Ok(Some(info)) => {
            let left = info.not_after - crate::store::now_epoch() as i64;
            if left < RENEW_BEFORE_DAYS * 86_400 {
                (true, "leaf within the renewal window")
            } else if info.names != names {
                (true, "the node's names changed")
            } else {
                (false, "")
            }
        }
        Ok(None) => (true, "no leaf yet"),
        Err(_) => (true, "leaf unreadable"),
    }
}

/// Which peer holds the root and is linked right now.
fn linked_root(inner: &NodeInner) -> Option<String> {
    let mesh = inner.mesh()?;
    let links = mesh.links.lock().unwrap();
    let health = mesh.health.lock().unwrap();
    health
        .iter()
        .find(|(n, h)| h.has_root == Some(true) && links.contains_key(*n))
        .map(|(n, _)| n.clone())
}

/// Ask for (or, on the root, mint) a leaf for this node's names and load
/// it. Returns a one-line summary.
pub async fn renew(inner: &Arc<NodeInner>) -> Result<String> {
    let d = inner
        .data_dir
        .clone()
        .ok_or_else(|| anyhow!("node has no data dir"))?;
    let mesh = inner.mesh().ok_or_else(|| {
        anyhow!("this node is not in a mesh; TLS certificates come from the mesh root")
    })?;
    let files = MeshFiles::new(&d);
    let names = leaf_names(inner);
    let node = mesh.identity.node.clone();
    let csr = build_csr(&d, &node, &names)?;
    let (leaf, ca) = if files.load_root()?.is_some() {
        let ca = ca_ensure(&files, &[])?;
        mesh.set_tls_ca(&ca);
        let leaf = sign_csr(&files, &csr)?;
        let _ = inner.store.record_event(
            "node",
            "tls_issue",
            json!({ "node": node, "names": names, "self": true }),
        );
        (leaf, ca)
    } else {
        let root = linked_root(inner).ok_or_else(|| {
            anyhow!("the mesh root is not linked right now; the leaf is requested again later (or use `aspen tls request` / `sign` / `install`)")
        })?;
        let res = mesh
            .api_call(
                &root,
                "tls_csr",
                "",
                json!({ "csr": aspen_wire::b64::encode(&csr), "names": names }),
                std::time::Duration::from_secs(30),
            )
            .await?;
        let leaf = aspen_wire::b64::decode(res["leaf"].as_str().unwrap_or(""))
            .context("leaf from root")?;
        let ca: TlsCa = serde_json::from_value(res["ca"].clone()).context("CA from root")?;
        ca.verify_against(&mesh.root_public())
            .context("the root sent a CA the mesh root did not sign")?;
        files.set_tls_ca(&ca)?;
        mesh.set_tls_ca(&ca);
        (leaf, ca)
    };
    install_leaf(&d, &leaf, &ca.der)?;
    reload_resolver(&d, &inner.tls.resolver)?;
    let info = leaf_info(&d)?.ok_or_else(|| anyhow!("leaf not readable after install"))?;
    Ok(format!(
        "certificate for {} name{} installed, valid until {}",
        info.names.len(),
        if info.names.len() == 1 { "" } else { "s" },
        chrono::DateTime::from_timestamp(info.not_after, 0)
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default()
    ))
}

/// The renewal loop: once a minute, when a listener beyond loopback
/// exists, renew if the leaf is missing, within 30 days of expiry, or
/// names it no longer covers; back off ten minutes after a failure.
pub fn spawn_loop(inner: Arc<NodeInner>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            if inner
                .shutting_down
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return;
            }
            let Some(_port) = inner.tls.https_port() else {
                continue;
            };
            let Some(d) = inner.data_dir.clone() else {
                continue;
            };
            let now = crate::store::now_epoch();
            if now < *inner.tls.next_try.lock().unwrap() {
                continue;
            }
            let names = leaf_names(&inner);
            let (due, why) = needs_renewal(&d, &names);
            if !due {
                *inner.tls.next_try.lock().unwrap() = now + 3600.0;
                continue;
            }
            *inner.tls.last_attempt.lock().unwrap() = Some(now);
            match renew(&inner).await {
                Ok(s) => {
                    tracing::info!(why, "tls: {s}");
                    *inner.tls.last_error.lock().unwrap() = None;
                    *inner.tls.last_ok.lock().unwrap() = Some(now);
                    *inner.tls.next_try.lock().unwrap() = now + 3600.0;
                }
                Err(e) => {
                    tracing::warn!(why, "tls: renewal failed: {e:#}");
                    *inner.tls.last_error.lock().unwrap() = Some(format!("{e:#}"));
                    *inner.tls.next_try.lock().unwrap() = now + 600.0;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(40)).await;
        }
    });
}

/// The CA of this node's primary mesh, as known here.
pub fn known_ca(inner: &NodeInner) -> Option<TlsCa> {
    let mesh = inner.mesh()?;
    let files = MeshFiles::new(inner.data_dir.as_deref()?);
    files.tls_ca(&mesh.mesh_name()).ok().flatten()
}

/// This computer's trust stores and the CA's state in each, cached for
/// 30 s (the probes run platform tools). Blocking.
pub fn stores_cached(inner: &NodeInner, force: bool) -> Vec<crate::truststore::Store> {
    let now = crate::store::now_epoch();
    if !force {
        if let Some((at, v)) = inner.tls.stores_cache.lock().unwrap().as_ref() {
            if now - at < 30.0 {
                return v.clone();
            }
        }
    }
    let v = match known_ca(inner) {
        Some(ca) => crate::truststore::stores(&ca),
        None => Vec::new(),
    };
    *inner.tls.stores_cache.lock().unwrap() = Some((now, v.clone()));
    v
}

/// `GET /api/tls`: the CA as known here, the leaf, the names, the loop.
pub fn status_json(inner: &NodeInner) -> Value {
    let d = inner.data_dir.clone();
    let files = d.as_deref().map(MeshFiles::new);
    let mesh = inner.mesh();
    let mesh_name = mesh.as_ref().map(|m| m.mesh_name());
    let root_here = files
        .as_ref()
        .and_then(|f| f.load_root().ok().flatten())
        .is_some();
    let ca = mesh_name
        .as_deref()
        .and_then(|m| files.as_ref().and_then(|f| f.tls_ca(m).ok().flatten()));
    let ca_json = ca.as_ref().map(|c| {
        json!({
            "mesh": c.mesh,
            "fingerprint": c.fingerprint(),
            "not_after": ca_not_after(&c.der),
            "root_sig_ok": mesh.as_ref().map(|m| c.verify_against(&m.root_public()).is_ok()),
            "here": root_here,
            "pem": der_to_pem(&c.der),
        })
    });
    let leaf = d.as_deref().and_then(|d| leaf_info(d).ok().flatten());
    let names = leaf_names(inner);
    let leaf_json = leaf.as_ref().map(|l| {
        json!({
            "names": l.names,
            "not_before": l.not_before,
            "not_after": l.not_after,
            "fingerprint": l.fingerprint,
            "issuer": l.issuer,
            "covers_current_names": l.names == names,
            "serving": inner.tls.resolver.has_cert(),
        })
    });
    json!({
        "mesh": mesh_name,
        "root_here": root_here,
        "ca": ca_json,
        "leaf": leaf_json,
        "names": names,
        "https_port": inner.tls.https_port(),
        "separate_port": inner.tls.separate_port.load(std::sync::atomic::Ordering::Relaxed),
        "last_error": inner.tls.last_error.lock().unwrap().clone(),
        "last_attempt": *inner.tls.last_attempt.lock().unwrap(),
        "last_ok": *inner.tls.last_ok.lock().unwrap(),
    })
}

/// The offline path's blobs (`aspen tls request` / `sign` / `install`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsRequest {
    pub node: String,
    pub mesh: String,
    #[serde(with = "aspen_wire::b64")]
    pub csr: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsBundle {
    pub node: String,
    #[serde(with = "aspen_wire::b64")]
    pub leaf: Vec<u8>,
    pub ca: TlsCa,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admissible_names() {
        assert_eq!(admissible("Anindor").as_deref(), Some("anindor"));
        assert_eq!(
            admissible("macbook.local.").as_deref(),
            Some("macbook.local")
        );
        assert_eq!(admissible("192.168.1.20").as_deref(), Some("192.168.1.20"));
        assert_eq!(
            admissible("100.101.102.103").as_deref(),
            Some("100.101.102.103")
        );
        assert_eq!(admissible("8.8.8.8"), None);
        assert_eq!(admissible("localhost"), None);
        assert_eq!(admissible("bad_name"), None);
        assert_eq!(
            admissible("fd7a:115c:a1e0::1").as_deref(),
            Some("fd7a:115c:a1e0::1")
        );
        assert_eq!(admissible("2001:db8::1"), None);
    }

    #[test]
    fn ca_signs_a_leaf_that_rustls_loads() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let files = MeshFiles::new(d);
        let root = aspen_wire::identity::MeshRoot::create("t");
        files.save_root(&root).unwrap();
        files
            .save_mesh(&crate::mesh::MeshConfig {
                mesh: "t".into(),
                root_public: root.root_public.clone(),
                peers: vec![],
                relay: None,
                relays: vec![],
                policy: None,
                tls_ca: None,
            })
            .unwrap();
        let ca = ca_ensure(&files, &[]).unwrap();
        ca.verify_against(&root.root_public).unwrap();
        assert!(files.tls_ca("t").unwrap().is_some());
        // Same CA on a second ensure.
        assert_eq!(ca_ensure(&files, &[]).unwrap().der, ca.der);

        let names = vec!["10.0.0.5".to_string(), "anindor".to_string()];
        let csr = build_csr(d, "anindor", &names).unwrap();
        assert_eq!(csr_names(&csr).unwrap().len(), 2);
        let leaf = sign_csr(&files, &csr).unwrap();
        install_leaf(d, &leaf, &ca.der).unwrap();
        let info = leaf_info(d).unwrap().unwrap();
        assert_eq!(info.names, names);
        assert!(info.not_after - info.not_before > 89 * 86_400);
        let r = Resolver::default();
        assert!(reload_resolver(d, &r).unwrap());
        assert!(r.has_cert());
        let _cfg = server_config(Arc::new(r)).unwrap();

        // A public address is refused whole.
        let csr = build_csr(d, "anindor", &["8.8.8.8".to_string()]).unwrap();
        assert!(sign_csr(&files, &csr).is_err());
    }
}
