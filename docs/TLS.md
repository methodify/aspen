# TLS for nodes: the mesh root as a certificate authority

**Status:** reference for what is built (2026-09-23, v0.41: T-1, T-2;
v0.42: T-3, T-4 of PROPOSALS-2026-09-M.md). Code:
`aspen-wire::identity::TlsCa`, `crates/aspen-node/src/tls.rs`,
`crates/aspen-node/src/truststore.rs`, `crates/aspen/src/tlsserve.rs`,
the `tls_csr` op and roster field in `federation.rs`, `aspen tls …` in
`main.rs`, `CertificateRow` in `ui/src/meshPanel.tsx`.

## 1. Why

The hosted console is served over `https://`, so it may not fetch
`http://<node>` on another machine — mixed content, blocked before CORS,
with only `localhost` exempt — and a console opened from a node's own
plain-http address is an insecure context: no Web Locks (the shared relay
connection), no clipboard, no Notifications or Web Push, no service
worker or "install as app". A certificate the browser trusts makes
`https://<node>` on the home network a first-class origin, and leaves the
relay for when you are away.

## 2. The CA

The mesh root key is Ed25519, which no browser accepts for TLS. So the
root holder keeps a second key pair, **P-256**, as an X.509 CA beside
`root.key`:

- `tls-ca.key` (mode 0600) and `tls-ca.crt`: subject `CN=Aspen mesh
  <name>, O=Aspen` (what a trust store shows), CA:TRUE with pathLen 0,
  keyCertSign + cRLSign, ten years.
- **Name constraints, critical:** permitted IP subtrees 10/8, 172.16/12,
  192.168/16, 100.64/10 (Tailscale), 169.254/16, fc00::/7, fe80::/10. A
  leaf naming a public address never validates under this CA, whatever
  becomes of its key. DNS names are unconstrained by default — members'
  hostnames are arbitrary single labels; `aspen tls ca --dns-suffix
  .local --dns-suffix .example.ts.net` at creation adds a DNS
  constraint for an operator whose names all live under suffixes.
- **Bound to the mesh:** `TlsCa {mesh, der, root_sig}` where `root_sig`
  is the Ed25519 root's signature over `aspen-tls-ca-v1\0<mesh>\0<sha256
  der>`. It is stored in `mesh.json` (`tls_ca`), rides every roster
  (`tls_ca`), and is verified against `root_public` before a member keeps
  it — the same trust every node cert has, with no root online.
- Minted on demand: the first leaf request, or `aspen tls ca`. `aspen tls
  ca --rotate` replaces it (every leaf is re-issued on its next check;
  every computer must trust the new CA).

One CA per mesh. A node in several meshes serves a leaf from its
**primary** mesh's CA (one listener, one certificate).

## 3. A node's leaf

- `tls.key` (P-256, 0600, never leaves the node) and `tls.crt` (the leaf
  followed by the CA).
- **Names** (`aspen tls status` → *names now*): the hostname and its
  `.local` form (when the hostname has no dot), every non-loopback
  interface address that is private (IPv4 not link-local; IPv6 ULA), the
  host of each `aspen config advertise` URL, and `aspen config tls-names`
  (comma-separated). `localhost`, loopback and public addresses are never
  included; a public address in a CSR is refused whole by the root.
- **Issuance over the link:** the node builds a CSR (subject `CN=<node>`)
  and sends `tls_csr {csr, names}` to the linked peer whose roster says
  `has_root`. The root verifies the CSR signature, checks every SAN
  (hostname syntax, private address), signs a **90-day** leaf
  (serverAuth, digitalSignature, AKI), records `tls_issue {node, names}`
  on its fleet trail, and answers `{leaf, ca}`. The member verifies the
  CA against the mesh root, stores both, and loads the chain into the
  live resolver — no restart.
- **The loop** (`tls::spawn_loop`): every 20 s, when a listener beyond
  loopback exists, renew if there is no leaf, it is within **30 days** of
  expiry, or it no longer covers the current names (a new network, DHCP
  moved). Backoff: an hour after a success or a no-op, ten minutes after
  a failure. `POST /api/tls/renew` / `aspen tls renew` forces one. The
  root node signs its own leaf locally.
- **Offline path** (mirrors enroll → certify → join): `aspen tls request`
  prints an `aspen:tls-req:` blob; `aspen tls sign <blob>` where the root
  key lives prints an `aspen:tls-cert:` bundle (leaf + CA); `aspen tls
  install <bundle>` on the node installs it and tells the daemon to
  reload.
- A member whose root is not linked keeps serving its current leaf and
  retries; `aspen tls status` shows *last renewal error*.

## 4. Serving

- **Same port by default.** The listener peeks the first byte of each
  connection: `0x16` (a TLS ClientHello) goes to rustls with the live
  chain; anything else is plain HTTP, unchanged. One `--listen`, one
  `daemon.json`, no new firewall rule; `http://` keeps working for the
  CLI's loopback calls, older consoles and anyone who has not trusted the
  CA yet. The peek and the handshake run in a task per connection, so a
  silent client never holds the accept loop (10 s peek timeout → plain;
  15 s handshake timeout → dropped).
- **`--tls-listen <addr>`** (or `aspen config tls-listen`) serves https
  on a separate port instead; the main port is then plain only. For when
  the sniff misbehaves with some client, or a port must be TLS-only.
  `daemon.json` carries `tls_listen` / `tls_requested`; `aspen restart`
  keeps it.
- **https is on only when a listener is beyond loopback** — a
  `127.0.0.1` node has nothing to certify and `GET /api/tls` says
  `https_port: null`. The node token rule is unchanged: beyond loopback,
  every call still carries it, over https as over http. TLS says which
  node this is; the token still says who may ask.
- HTTP/1.1 only over TLS (ALPN): the console's WebSocket upgrades need
  it. Session event streams work at `wss://` for free.
- Node-to-node federation links stay `ws://` + sealed envelopes: they
  are end-to-end sealed already, and a link must not fail because a leaf
  expired.
- The node's advertisement (rosters, `GET /api/mesh`
  `identity.advertised.https_urls`) lists one `https://<name>:<port>` per
  name the leaf covers; `aspen status` and the node-up line say whether
  a certificate is served.

## 5. Surfaces

- `GET /api/tls` → `{mesh, root_here, ca{mesh, fingerprint, not_after,
  root_sig_ok, here, pem}, leaf{names, not_before, not_after,
  fingerprint, issuer, covers_current_names, serving}, names,
  https_port, separate_port, last_error, last_attempt, last_ok}`.
- `POST /api/tls/renew` → `{ok, summary}` (400 with `error` when the root
  is not reachable, say).
- `GET /api/tls/root.crt` → the CA as `application/x-x509-ca-cert`, a
  download for a device that trusts it by hand.
- `GET /api/mesh` `meshes[].tls_ca {fingerprint, not_after}`.
- `aspen tls status | ca [--dns-suffix …] [--rotate] | renew | request |
  sign <blob> | install <bundle>`; `aspen config tls-listen`, `tls-names`;
  `aspen up --tls-listen`.
- Fleet trail: `tls_issue {node, names, self?}` on the root.

## 6. Trusting the CA on a computer (v0.42, T-3)

A browser needs the CA in a store it reads, once per computer. The node
knows the stores of the platform it runs on (`truststore.rs`), whether
the CA is in each, and writes the ones it can — **behind the platform's
own consent**, never silently, never escalating:

| store | read by | how the node writes it | prompt |
|---|---|---|---|
| Windows, this user's Root | Chrome, Edge (Firefox with enterprise roots on) | `certutil -addstore -user Root` | Windows' own "install this certificate?" dialog with the thumbprint |
| the same, from a **WSL** node | the Windows browser | `/mnt/c/Windows/System32/certutil.exe` through interop, the PEM as a UNC path via `wslpath -w` | the same dialog, on the Windows desktop |
| macOS login keychain, trust *always* | Safari, Chrome | `security add-trusted-cert -r trustRoot -k ~/Library/Keychains/login.keychain-db` | keychain password |
| Chrome / Chromium on Linux (NSS) | Chrome, Chromium | `certutil -d sql:~/.pki/nssdb -A -t "C,," -n "Aspen mesh <name>"` (`~/.local/share/pki/nssdb` when that is the one in use; created if missing) | none; needs `libnss3-tools` / `nss-tools`, else the command is printed |
| Firefox profiles | Firefox | the same `certutil` per `cert9.db` (Linux; macOS with Homebrew nss) | none; without `certutil`: the about:config switch or the Authorities import, printed |
| Linux system anchors | curl, Python, other tools | `/usr/local/share/ca-certificates/aspen-mesh-<name>.crt` + `update-ca-certificates`, or `/etc/pki/ca-trust/source/anchors` + `update-ca-trust` | `sudo` in a terminal only (`aspen tls trust --system`); never from the API |
| iOS, Android, any device with no node | its browser | download the `.crt`, install it by hand (iOS: also *Certificate Trust Settings* → full trust) | the device's own flow |

- `aspen tls trust` lists every store with ✓ / – / ?, says exactly what
  it will write and which prompts to expect, asks once, runs, then
  prints the by-hand recipes for what is left (and the phone steps).
  `--check` reports only; `--remove` reverses; `--print` writes the PEM
  (also at `<data-dir>/mesh-ca.crt`); `--system` includes the Linux
  anchors through sudo; `-y` skips the question.
- `GET /api/tls` carries `stores: [{id, label, installed, writable,
  needs_terminal, detail, command}]` (probed off the runtime's workers,
  cached 30 s); `POST /api/tls/trust {stores?, remove?}` writes every
  writable store (or the named ones) and answers `{ok, results[{id, ok,
  detail}], stores}`. A write waits at most five minutes for the
  platform's dialog: a daemon with no desktop (Windows session 0) cannot
  show it, and the answer says to run `aspen tls trust` in a terminal on
  that desktop.
- The CA is identified in Windows by its SHA-1 thumbprint, in the
  keychain and NSS by the nickname `Aspen mesh <name>` (NSS entries are
  compared by body, so a rotated CA replaces the old one).

## 7. The console (v0.42, T-4)

- **Meshes → certificate row** (`CertificateRow`): the mesh CA's
  fingerprint and expiry, this node's https (port, same or separate,
  serving or not, names in the title), and — when the console is
  attached to a node on **this computer** (loopback, not through the
  relay) — one chip per trust store with its state and a **trust on this
  computer** button that calls `POST /api/tls/trust` and shows each
  store's answer. Attached from elsewhere, the row says to run `aspen tls
  trust` on that computer instead. Always: **download CA** (a `.crt` for
  a phone or a hand install, built from the PEM in the page so it works
  through the relay too), **copy PEM**, and **steps** (iPhone, Android,
  Windows, macOS, Firefox, Linux, plus whatever this node's stores still
  need a terminal for).
- **Peer rows and this node** show an `https ↗` chip when the node
  advertises https URLs; the link opens the first.
- **Attach**: `https://<node>:<port>` is accepted as a direct address; a
  failed https attach explains that the browser most likely does not
  trust the mesh certificate yet and where to fix that (the browser
  itself reports only "failed to fetch").

Verified (rig, 2026-09-23): `aspen tls trust --check` on a WSL node
found the Windows user store through interop (not yet), Chrome's NSS
database without `certutil` (unknown, tool named), the Linux anchors
(not yet, sudo command); the console's certificate row rendered the same
three with the trust button; the download produced
`aspen-mesh-rigmesh.crt`. The Windows install itself needs the operator
at the desktop to accept the dialog.

Verified (rig, 2026-09-23): the root node on `0.0.0.0` minted the CA and
its own leaf within a loop tick; TLS 1.3 with `Verify return code: 0`
against the CA, http and https on one port; a member joined offline,
learned the CA from the first roster and had its 90-day leaf over the
link 17 s after start; the offline blobs issued a second leaf; the
member restarted with `--tls-listen` served plain on 7696 and TLS on
7697, advertising the 7697 URLs.
