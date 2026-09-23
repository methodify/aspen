//! Trusting the mesh CA on this computer (docs/TLS.md §6): the stores a
//! browser here reads, whether the CA is in each, and putting it there
//! with the platform's own tool — behind the platform's own consent
//! (Windows' certificate dialog, the macOS keychain prompt, sudo for
//! Linux system anchors). Nothing is installed silently, and nothing here
//! escalates: a store that needs root is reported with the command to run.
//!
//! Stores, by platform:
//! - Windows: the current user's Root store (`certutil -addstore -user
//!   Root`) — Chrome, Edge; Firefox only with enterprise roots on.
//! - WSL: the same Windows store, through `certutil.exe` interop — the
//!   browser is on the Windows side.
//! - macOS: the login keychain, trust set to always (`security
//!   add-trusted-cert -r trustRoot`) — Safari, Chrome.
//! - Linux: Chrome/Chromium's NSS database (`certutil -d sql:…`, from
//!   libnss3-tools / nss-tools); the system anchors (needs root; printed).
//! - Firefox anywhere: its profile databases, with NSS `certutil` where
//!   it exists; else the about:config switch is printed.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use aspen_wire::identity::TlsCa;

/// One trust store on this computer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub id: String,
    pub label: String,
    /// Some(true/false) when the check could run; None when it could not.
    pub installed: Option<bool>,
    /// The node can write it from here (with the platform's consent).
    pub writable: bool,
    /// Needs a terminal (sudo, or a tool this machine lacks).
    pub needs_terminal: bool,
    /// What to do, or why not.
    pub detail: String,
    /// The command an operator would run by hand.
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub ok: bool,
    pub detail: String,
}

fn nick(ca: &TlsCa) -> String {
    format!("Aspen mesh {}", ca.mesh)
}

fn sha1_hex(der: &[u8]) -> String {
    use sha1::Digest as _;
    sha1::Sha1::digest(der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn sha256_hex(der: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

fn pem_of(ca: &TlsCa) -> String {
    let b = aspen_wire::b64::encode(&ca.der);
    let mut s = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b.as_bytes().chunks(64) {
        s.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        s.push('\n');
    }
    s.push_str("-----END CERTIFICATE-----\n");
    s
}

/// The CA as a file the platform tools can read: `<data_dir>/mesh-ca.crt`
/// (a member holds the CA only inside mesh.json).
pub fn materialize(data_dir: &Path, ca: &TlsCa) -> Result<PathBuf> {
    let p = data_dir.join("mesh-ca.crt");
    let pem = pem_of(ca);
    if std::fs::read_to_string(&p).ok().as_deref() != Some(pem.as_str()) {
        std::fs::write(&p, pem).with_context(|| format!("writing {}", p.display()))?;
    }
    Ok(p)
}

fn run(mut cmd: std::process::Command) -> Result<(bool, String)> {
    let out = cmd
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok((out.status.success(), text.trim().to_owned()))
}

/// Run with a ceiling: an install that waits on a consent dialog nobody
/// can see (a daemon with no desktop) must not hang the caller for ever.
fn run_timeout(mut cmd: std::process::Command, secs: u64) -> Result<(bool, String)> {
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    let start = std::time::Instant::now();
    loop {
        if let Some(st) = child.try_wait()? {
            let out = child.wait_with_output()?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return Ok((st.success(), text.trim().to_owned()));
        }
        if start.elapsed().as_secs() > secs {
            let _ = child.kill();
            bail!("no answer in {secs}s — the consent dialog may be where no one can see it; run `aspen tls trust` in a terminal on that desktop");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------- Windows

/// Windows' certutil — native, or through WSL interop.
fn win_certutil() -> Option<PathBuf> {
    if cfg!(windows) {
        return Some(PathBuf::from("certutil.exe"));
    }
    if is_wsl() {
        let p = PathBuf::from("/mnt/c/Windows/System32/certutil.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// A path as Windows' certutil must see it (UNC through WSL).
fn win_path(p: &Path) -> Result<String> {
    if cfg!(windows) {
        return Ok(p.to_string_lossy().into_owned());
    }
    let (ok, out) = run({
        let mut c = aspen_core::quiet_command("wslpath");
        c.arg("-w").arg(p);
        c
    })?;
    if !ok || out.is_empty() {
        bail!("wslpath could not translate {}", p.display());
    }
    Ok(out)
}

fn windows_store(ca: &TlsCa) -> Option<Store> {
    let exe = win_certutil()?;
    let sha1 = sha1_hex(&ca.der);
    let installed = run({
        let mut c = aspen_core::quiet_command(&exe);
        c.args(["-user", "-store", "Root", &sha1]);
        c
    })
    .ok()
    .map(|(ok, _)| ok);
    let wsl = !cfg!(windows);
    Some(Store {
        id: "windows-user-root".into(),
        label: if wsl {
            "Windows: this user's trusted roots (Chrome, Edge — via WSL)".into()
        } else {
            "Windows: this user's trusted roots (Chrome, Edge)".into()
        },
        installed,
        writable: true,
        needs_terminal: false,
        detail: "Windows shows its own \"install this certificate?\" dialog with the thumbprint; no administrator rights needed. Firefox reads this store only with security.enterprise_roots.enabled set to true in about:config.".into(),
        command: Some(format!(
            "certutil -addstore -user Root <mesh-ca.crt>   (remove: certutil -delstore -user Root {sha1})"
        )),
    })
}

fn windows_install(data_dir: &Path, ca: &TlsCa, remove: bool) -> Result<String> {
    let exe = win_certutil().ok_or_else(|| anyhow!("no certutil.exe reachable"))?;
    let mut c = aspen_core::quiet_command(&exe);
    if remove {
        c.args(["-delstore", "-user", "Root", &sha1_hex(&ca.der)]);
    } else {
        let pem = materialize(data_dir, ca)?;
        let wp = win_path(&pem)?;
        c.args(["-addstore", "-user", "Root", &wp]);
    }
    let (ok, out) = run_timeout(c, 300)?;
    if !ok {
        bail!(
            "certutil: {}",
            out.lines()
                .last()
                .unwrap_or("failed (the dialog may have been declined)")
        );
    }
    Ok(if remove {
        "removed from the Windows user Root store".into()
    } else {
        "in the Windows user Root store".into()
    })
}

// ------------------------------------------------------------------ macOS

fn login_keychain() -> Option<PathBuf> {
    let p = home()?.join("Library/Keychains/login.keychain-db");
    p.is_file().then_some(p)
}

fn macos_store(ca: &TlsCa) -> Option<Store> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let kc = login_keychain()?;
    let want = sha256_hex(&ca.der);
    let installed = run({
        let mut c = aspen_core::quiet_command("security");
        c.args(["find-certificate", "-a", "-c", &nick(ca), "-Z"])
            .arg(&kc);
        c
    })
    .ok()
    .map(|(_, out)| out.to_uppercase().contains(&want));
    Some(Store {
        id: "macos-login-keychain".into(),
        label: "macOS: login keychain, trusted as a root (Safari, Chrome)".into(),
        installed,
        writable: true,
        needs_terminal: false,
        detail: "macOS asks for your login password to change trust settings. Firefox reads the keychain only with security.enterprise_roots.enabled set to true in about:config.".into(),
        command: Some(format!(
            "security add-trusted-cert -r trustRoot -k {} <mesh-ca.crt>",
            kc.display()
        )),
    })
}

fn macos_install(data_dir: &Path, ca: &TlsCa, remove: bool) -> Result<String> {
    let kc = login_keychain().ok_or_else(|| anyhow!("no login keychain"))?;
    let mut c = aspen_core::quiet_command("security");
    if remove {
        c.args(["delete-certificate", "-Z", &sha256_hex(&ca.der)])
            .arg(&kc);
    } else {
        let pem = materialize(data_dir, ca)?;
        c.args(["add-trusted-cert", "-r", "trustRoot", "-k"])
            .arg(&kc)
            .arg(&pem);
    }
    let (ok, out) = run_timeout(c, 300)?;
    if !ok {
        bail!("security: {}", out.lines().last().unwrap_or("failed"));
    }
    Ok(if remove {
        "removed from the login keychain".into()
    } else {
        "in the login keychain, trusted as a root".into()
    })
}

// -------------------------------------------------------------- NSS (Linux)

fn nss_certutil() -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    which("certutil").or_else(|| {
        [
            "/opt/homebrew/opt/nss/bin/certutil",
            "/usr/local/opt/nss/bin/certutil",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
    })
}

/// Chrome/Chromium's shared NSS database on Linux.
fn chrome_nssdb() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let h = home()?;
    let a = h.join(".pki/nssdb");
    let b = h.join(".local/share/pki/nssdb");
    if a.join("cert9.db").is_file() {
        Some(a)
    } else if b.join("cert9.db").is_file() {
        Some(b)
    } else {
        Some(a)
    }
}

fn nss_has(certutil: &Path, db: &Path, ca: &TlsCa) -> Option<bool> {
    let (ok, out) = run({
        let mut c = aspen_core::quiet_command(certutil);
        c.args(["-L", "-d"])
            .arg(format!("sql:{}", db.display()))
            .args(["-n", &nick(ca), "-a"]);
        c
    })
    .ok()?;
    if !ok {
        return Some(false);
    }
    // The nickname may hold an older CA (after a rotate): compare bodies.
    let ours: String = pem_of(ca).chars().filter(|c| !c.is_whitespace()).collect();
    let theirs: String = out.chars().filter(|c| !c.is_whitespace()).collect();
    Some(theirs.contains(&ours))
}

fn nss_put(certutil: &Path, db: &Path, pem: &Path, ca: &TlsCa, remove: bool) -> Result<()> {
    let sql = format!("sql:{}", db.display());
    if remove {
        let (ok, out) = run({
            let mut c = aspen_core::quiet_command(certutil);
            c.args(["-D", "-d", &sql, "-n", &nick(ca)]);
            c
        })?;
        if !ok && !out.contains("could not find") {
            bail!("certutil: {out}");
        }
        return Ok(());
    }
    if !db.join("cert9.db").is_file() {
        std::fs::create_dir_all(db)?;
        let (ok, out) = run({
            let mut c = aspen_core::quiet_command(certutil);
            c.args(["-N", "--empty-password", "-d", &sql]);
            c
        })?;
        if !ok {
            bail!("certutil -N: {out}");
        }
    }
    // Replace an older CA under the same nickname.
    let _ = run({
        let mut c = aspen_core::quiet_command(certutil);
        c.args(["-D", "-d", &sql, "-n", &nick(ca)]);
        c
    });
    let (ok, out) = run({
        let mut c = aspen_core::quiet_command(certutil);
        c.args(["-A", "-d", &sql, "-t", "C,,", "-n", &nick(ca), "-i"])
            .arg(pem);
        c
    })?;
    if !ok {
        bail!("certutil -A: {out}");
    }
    Ok(())
}

fn chrome_store(ca: &TlsCa) -> Option<Store> {
    let db = chrome_nssdb()?;
    match nss_certutil() {
        Some(cu) => Some(Store {
            id: "chrome-nss".into(),
            label: "Chrome / Chromium (NSS database)".into(),
            installed: nss_has(&cu, &db, ca),
            writable: true,
            needs_terminal: false,
            detail: format!("{} — written without a prompt.", db.display()),
            command: Some(format!(
                "certutil -d sql:{} -A -t \"C,,\" -n \"{}\" -i <mesh-ca.crt>",
                db.display(),
                nick(ca)
            )),
        }),
        None => Some(Store {
            id: "chrome-nss".into(),
            label: "Chrome / Chromium (NSS database)".into(),
            installed: None,
            writable: false,
            needs_terminal: true,
            detail: "needs the NSS `certutil` tool: apt install libnss3-tools (Debian/Ubuntu) or dnf install nss-tools (Fedora), then run `aspen tls trust` again.".into(),
            command: Some(format!(
                "certutil -d sql:{} -A -t \"C,,\" -n \"{}\" -i <mesh-ca.crt>",
                db.display(),
                nick(ca)
            )),
        }),
    }
}

// ---------------------------------------------------------------- Firefox

fn firefox_profiles() -> Vec<PathBuf> {
    let Some(h) = home() else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        roots.push(h.join("Library/Application Support/Firefox/Profiles"));
    } else if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            roots.push(PathBuf::from(a).join("Mozilla/Firefox/Profiles"));
        }
    } else {
        roots.push(h.join(".mozilla/firefox"));
        roots.push(h.join("snap/firefox/common/.mozilla/firefox"));
        roots.push(h.join(".var/app/org.mozilla.firefox/.mozilla/firefox"));
    }
    let mut out = Vec::new();
    for r in roots {
        let Ok(rd) = std::fs::read_dir(&r) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.join("cert9.db").is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn firefox_store(ca: &TlsCa) -> Option<Store> {
    let profiles = firefox_profiles();
    if profiles.is_empty() {
        return None;
    }
    let n = profiles.len();
    match nss_certutil() {
        Some(cu) => {
            let states: Vec<Option<bool>> = profiles.iter().map(|p| nss_has(&cu, p, ca)).collect();
            let installed = if states.iter().any(|s| s.is_none()) {
                None
            } else {
                Some(states.iter().all(|s| *s == Some(true)))
            };
            Some(Store {
                id: "firefox".into(),
                label: format!("Firefox ({n} profile{})", if n == 1 { "" } else { "s" }),
                installed,
                writable: true,
                needs_terminal: false,
                detail: "each profile's own certificate database — written without a prompt; restart Firefox afterwards.".into(),
                command: Some(format!(
                    "certutil -d sql:<profile> -A -t \"C,,\" -n \"{}\" -i <mesh-ca.crt>",
                    nick(ca)
                )),
            })
        }
        None => Some(Store {
            id: "firefox".into(),
            label: format!("Firefox ({n} profile{})", if n == 1 { "" } else { "s" }),
            installed: None,
            writable: false,
            needs_terminal: true,
            detail: if cfg!(any(windows, target_os = "macos")) {
                "Firefox keeps its own store. Either set security.enterprise_roots.enabled to true in about:config (it then reads the system store above), or import mesh-ca.crt under Settings → Privacy & Security → Certificates → View Certificates → Authorities.".into()
            } else {
                "needs the NSS `certutil` tool (libnss3-tools / nss-tools) to write each profile; or import mesh-ca.crt under Settings → Privacy & Security → Certificates → Authorities.".into()
            },
            command: None,
        }),
    }
}

// ------------------------------------------------------------ Linux system

fn system_anchor(ca: &TlsCa) -> Option<(PathBuf, &'static str)> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let file = format!("aspen-mesh-{}.crt", ca.mesh);
    if which("update-ca-certificates").is_some()
        || Path::new("/usr/sbin/update-ca-certificates").is_file()
    {
        return Some((
            PathBuf::from("/usr/local/share/ca-certificates").join(file),
            "update-ca-certificates",
        ));
    }
    if which("update-ca-trust").is_some() || Path::new("/usr/bin/update-ca-trust").is_file() {
        return Some((
            PathBuf::from("/etc/pki/ca-trust/source/anchors").join(file),
            "update-ca-trust",
        ));
    }
    None
}

fn system_store(ca: &TlsCa) -> Option<Store> {
    let (path, tool) = system_anchor(ca)?;
    let installed =
        Some(std::fs::read_to_string(&path).ok().as_deref() == Some(pem_of(ca).as_str()));
    let root = is_root();
    Some(Store {
        id: "linux-system".into(),
        label: "Linux system anchors (curl, Python, other tools)".into(),
        installed,
        writable: root,
        needs_terminal: !root,
        detail: if root {
            "written as root.".into()
        } else {
            "needs root; not required for a browser — run it in a terminal if command-line tools should trust the mesh too.".into()
        },
        command: Some(format!(
            "sudo cp <mesh-ca.crt> {} && sudo {tool}",
            path.display()
        )),
    })
}

fn is_root() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn system_install(data_dir: &Path, ca: &TlsCa, remove: bool) -> Result<String> {
    let (path, tool) =
        system_anchor(ca).ok_or_else(|| anyhow!("no system anchor directory known"))?;
    if remove {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
        }
    } else {
        let pem = materialize(data_dir, ca)?;
        std::fs::copy(&pem, &path).with_context(|| format!("copying to {}", path.display()))?;
    }
    let (ok, out) = run(aspen_core::quiet_command(tool))?;
    if !ok {
        bail!("{tool}: {out}");
    }
    Ok(if remove {
        "removed from the system anchors".into()
    } else {
        "in the system anchors".into()
    })
}

// ------------------------------------------------------------------ public

/// Every store this computer has, with the CA's state in each. Runs the
/// platform tools (fast reads); call off the runtime's workers.
pub fn stores(ca: &TlsCa) -> Vec<Store> {
    let mut v = Vec::new();
    if let Some(s) = windows_store(ca) {
        v.push(s);
    }
    if let Some(s) = macos_store(ca) {
        v.push(s);
    }
    if let Some(s) = chrome_store(ca) {
        v.push(s);
    }
    if let Some(s) = firefox_store(ca) {
        v.push(s);
    }
    if let Some(s) = system_store(ca) {
        v.push(s);
    }
    v
}

/// Install (or remove) the CA in the named stores — every writable one
/// when `ids` is empty. Each store answers for itself.
pub fn apply(data_dir: &Path, ca: &TlsCa, ids: &[String], remove: bool) -> Vec<Outcome> {
    let all = stores(ca);
    let mut out = Vec::new();
    for s in all {
        if !ids.is_empty() && !ids.iter().any(|i| i == &s.id) {
            continue;
        }
        if ids.is_empty() && (!s.writable || s.needs_terminal) {
            continue;
        }
        let r: Result<String> = match s.id.as_str() {
            "windows-user-root" => windows_install(data_dir, ca, remove),
            "macos-login-keychain" => macos_install(data_dir, ca, remove),
            "chrome-nss" => (|| {
                let cu =
                    nss_certutil().ok_or_else(|| anyhow!("no NSS certutil on this machine"))?;
                let db = chrome_nssdb().ok_or_else(|| anyhow!("no NSS database path"))?;
                let pem = materialize(data_dir, ca)?;
                nss_put(&cu, &db, &pem, ca, remove)?;
                Ok(if remove {
                    "removed from Chrome's NSS database".into()
                } else {
                    "in Chrome's NSS database (restart Chrome if it is open)".into()
                })
            })(),
            "firefox" => (|| {
                let cu =
                    nss_certutil().ok_or_else(|| anyhow!("no NSS certutil on this machine"))?;
                let pem = materialize(data_dir, ca)?;
                let profiles = firefox_profiles();
                for p in &profiles {
                    nss_put(&cu, p, &pem, ca, remove)?;
                }
                Ok(format!(
                    "{} {} Firefox profile{}",
                    if remove { "removed from" } else { "in" },
                    profiles.len(),
                    if profiles.len() == 1 { "" } else { "s" }
                ))
            })(),
            "linux-system" => system_install(data_dir, ca, remove),
            other => Err(anyhow!("unknown store {other:?}")),
        };
        out.push(match r {
            Ok(d) => Outcome {
                id: s.id,
                ok: true,
                detail: d,
            },
            Err(e) => Outcome {
                id: s.id,
                ok: false,
                detail: format!("{e:#}"),
            },
        });
    }
    out
}
