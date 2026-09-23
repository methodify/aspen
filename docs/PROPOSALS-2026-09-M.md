# Proposal M — Direct HTTPS to any node: the mesh root as a certificate authority

**Status:** accepted 2026-09-23; T-1 + T-2 shipped as v0.41 (TLS.md). Decisions (§6): (a) IP name constraints
only, DNS opt-in; (b) same port by default **and** an optional
`--tls-listen` for a separate port if the sniff misbehaves; (c) 90-day
leaves with renewal; (d) Trust from the console on a loopback node only;
(e) T-5 in; (f) D-5, R-1, P-3, D-6 in. Headliner: D-11 from the backlog.

## 0. The question

The mesh already has a root of trust: an Ed25519 root key that certifies
every node and every console identity. Can that root also sign **TLS
certificates** for nodes, so that a console can target a node that is not
on localhost over `https://` with no browser warning — and can a node
offer to put the mesh's root certificate into the computer's trust store,
on every platform we ship on?

Short answer: **yes, and it is worth doing.** The pieces are all standard
(a private CA with name constraints, ECDSA leaves, the platform trust
stores) and the payoff is larger than "no warning": today a console on a
LAN address is a second-class console. §1 explains why, §2 lays out the
browser-side facts that make it viable and the two places where it
cannot be made silent, §3 is the design, §4 the rows, §5 the rest of the
slate.

## 1. What we have today

- **Mesh PKI.** `aspen-wire::identity`: `MeshRoot` (Ed25519, held wherever
  the operator keeps `root.json`; `root_here` on the node that holds it),
  `NodeCert` over `aspen-cert-v1` signing bytes, console identities
  certified the same way (RELAY.md §11). Peers verify each other offline
  against `root_public`. The enroll → certify → join flow is paste-able
  blobs and works with no root online.
- **The listener is plain HTTP.** `api::serve` binds one TCP port. Beyond
  loopback it demands the node token on every call (`X-Aspen-Token` or
  `?token=`); CORS admits the hosted console's origin plus
  `aspen config console-origins`.
- **The hosted console is `https://`.** methodify.github.io/aspen may not
  fetch `http://anindor:7420` — mixed content, blocked before any CORS
  check, with `http://localhost` the only exemption. So from the hosted
  console the **only** road to a node that is not on this machine is the
  relay. Direct attach works only from a bundle the node itself serves
  (`http://anindor:7420/`, same origin), which is then an insecure context.
- **An insecure context loses APIs the console now relies on.** The Web
  Locks API behind C-1 (v0.40), `navigator.clipboard`, Notifications and
  Web Push (D-4), the service worker and "install as app" (D-1..D-3) are
  all secure-context only. A console opened at `http://anindor:7420` from
  a laptop on the same network gets none of them; the phone console
  (Q-1..Q-4) on the home Wi-Fi cannot be installed or receive pushes
  unless it goes through the relay.

So the goal is not cosmetic. With a trusted certificate, `https://<node>`
on the LAN is a first-class origin: every secure-context feature works,
the hosted bundle can attach directly with no relay in the path, and the
relay becomes what it was meant to be — the road for when you are away.

## 2. Is it viable? The browser-side facts

### 2.1 What a browser accepts from a private CA

- **X.509 with ECDSA P-256 (or RSA).** No browser does TLS with Ed25519
  keys. The mesh root key therefore cannot sign TLS certificates itself:
  the CA is a **second key pair, P-256, held next to the root**, and the
  Ed25519 root vouches for it (§3.1). This is a binding, not a gap: the
  mesh still has one root of trust.
- **Subject Alternative Names are mandatory** (Chrome dropped the CN
  fallback years ago); `extendedKeyUsage = serverAuth`; SHA-256.
- **Leaf validity.** Apple platforms refuse any TLS leaf longer than 825
  days, including under user-installed roots. Chrome's 398-day rule
  applies only to publicly trusted roots. We issue for **90 days** and
  renew at 60 (§3.2) — short lives keep a stolen leaf cheap and make
  renewal an exercised path, not an annual surprise.
- **Certificate Transparency is not required** for roots the user added
  themselves. Chrome, Safari and Firefox all accept a private chain
  without SCTs.
- **The `nameConstraints` extension is honored** in path validation by
  Chrome's verifier, Apple's and Firefox's. A constrained root limits what
  a leaked CA key can vouch for (§3.1 sets it).

### 2.2 Names: what does the console type?

A hostname (`anindor`), an mDNS name (`macbook.local`), a LAN or
Tailscale address (`192.168.1.20`, `100.101.102.103`), a DNS name behind
an `advertise` URL. IP addresses in SANs are accepted by every browser,
so the leaf covers **every name the node can claim**: its hostname, the
`.local` form, the FQDN if reverse DNS gives one, every non-loopback
interface address (v4 and v6, including Tailscale's 100.64/10 and
fd7a:115c:a1e0::/48), the host of `aspen config advertise`, and any
extras from a new `aspen config tls-names`. When the set changes (a new
network, DHCP moved) the node re-requests (§3.2). WSL2 matters here:
a WSL node's own hostname is not reachable from the LAN by name at all,
its interface addresses are what the phone will type — and in mirrored
networking mode those are the Windows host's addresses, which the node
sees on its own interfaces, so the rule covers it.

No `.aspen.internal` pseudo-domain: there is no resolver for it. Names
the browser can already resolve are the only ones worth certifying.

### 2.3 Blast radius: what a trusted mesh root can vouch for

Installing a root is the strongest thing a user does to a browser. Two
mitigations, both cheap:

- **The CA key never leaves the root holder** (like `root.json`); leaves
  are issued from a CSR, so a member node holds only its own P-256 key.
- **Name constraints on the CA.** IP space is constrained to private,
  CGNAT, ULA and link-local ranges (10/8, 172.16/12, 192.168/16,
  100.64/10, 169.254/16, fc00::/7, fe80::/10): a mesh certificate can
  never validate for a public address. DNS is **not** constrained by
  default — members' hostnames are single-label and arbitrary, and a
  permitted-subtree list that has to be reissued (and re-trusted
  everywhere) whenever a node joins is worse than no constraint. An
  operator whose names all live under a suffix can add
  `aspen tls ca --dns-suffix .local --dns-suffix .example.ts.net` at CA
  creation. Decision §6(a).
- The root node polices issuance: it signs only names the requesting
  member claims, refuses public IPs and anything outside the constraints,
  and records every issue on the fleet trail (`tls_issue {node, names}`)
  the way `remote_control` is recorded today.

### 2.4 Trust stores per platform

| Platform | Store that Chrome / Edge / Safari read | How the node installs it | Prompt |
|---|---|---|---|
| Windows | CurrentUser\Root | `certutil -addstore -user Root <cer>` (no elevation) | Windows' own "install this CA?" dialog with the thumbprint |
| macOS | login keychain, trust set to Always | `security add-trusted-cert -r trustRoot -k ~/Library/Keychains/login.keychain-db <pem>` | keychain password prompt |
| Linux, Chrome/Chromium | NSS shared DB (`~/.pki/nssdb`, or `~/.local/share/pki/nssdb` on newer Chromium) | `certutil -d sql:<db> -A -t "C,," -n "Aspen mesh <name>" -i <pem>` (`libnss3-tools` / `nss-tools`) | none |
| Linux, system (curl, Python, other nodes) | `/usr/local/share/ca-certificates` + `update-ca-certificates`, or `/etc/pki/ca-trust/source/anchors` + `update-ca-trust` | needs root; the CLI prints the two commands and runs them under `sudo` only when asked | sudo |
| Firefox, any OS | its own `cert9.db` per profile; reads the OS store only when `security.enterprise_roots.enabled` is on (off by default) | `certutil -d sql:<profile> -A …` for each profile found, or print the `policies.json` snippet | none |
| **WSL2** | the browser is on Windows | the WSL node runs `/mnt/c/Windows/System32/certutil.exe -addstore -user Root` through interop — the same dialog appears on the Windows desktop | Windows dialog |
| iOS | Settings → Profile Downloaded → Install, then Settings → General → About → Certificate Trust Settings → enable full trust | cannot be automated: the console offers **Download root certificate** (`/api/tls/root.crt`, served as `application/x-x509-ca-cert`) and shows the steps | user |
| Android | Settings → Security → Encryption & credentials → Install a certificate → CA certificate; Chrome honors user CAs | same download + steps | user |

Every store that can be written without elevation is written by the
node; every one that needs the user is a printed recipe. This is exactly
mkcert's ground, which has been the developer-tooling norm for years, so
the recipes are proven.

### 2.5 What cannot be made silent — and should not be

- **Each platform prompts, by design.** Windows shows its dialog, macOS
  asks for the keychain password, Linux system anchors need sudo. The
  node never installs quietly; "offer, then do" is what the platforms
  enforce anyway. A daemon that runs where no desktop is (Windows session
  0, a headless server) cannot show the dialog; the API reports
  `needs_terminal` and the console points at `aspen tls trust`.
- **Firefox on Windows and macOS** ignores the OS store unless the
  enterprise-roots preference is on; we write its profile DBs directly
  where `certutil` exists and print the recipe otherwise.
- **A machine with no node** (a friend's laptop, a phone) trusts by
  downloading the root certificate from any node it can already reach —
  through the relay from the hosted console is fine, the bytes are public
  and the download is verified against the mesh root signature (§3.1)
  before the console offers it.
- **CA rotation re-trusts everywhere.** Rare, explicit (`aspen tls ca
  rotate`), documented; not automated.

### 2.6 Verdict

Viable, with well-worn recipes on every platform we ship, and a payoff
that includes the whole secure-context feature set on LAN addresses, not
just the missing padlock. The design below keeps the Ed25519 root as the
single source of trust, keeps the CA key with the root, keeps the leaf
key on the node, constrains the CA, and keeps every trust-store write
behind the platform's own consent.

## 3. Design

### 3.1 The mesh CA (T-1)

Created on demand, on the node that holds `root.json`, the first time a
member asks for a leaf or the operator runs `aspen tls ca`:

- `tls-ca.key` (P-256, mode 0600, beside `root.json`) and `tls-ca.crt`:
  subject `CN=Aspen mesh <mesh>` (what the trust store shows), CA:TRUE,
  pathLen 0, keyCertSign + cRLSign, ten years, name constraints per §2.3.
- **Bound to the mesh root:** `tls_ca = {der, root_sig}` where `root_sig`
  is the Ed25519 root's signature over `aspen-tls-ca-v1\0<mesh>\0<sha256 der>`.
  It rides in `mesh.json`, in rosters (so every member and every console
  learns it and can verify it against `root_public` with no root online),
  and in `GET /api/mesh` under the mesh's entry.
- Multi-mesh: one CA per mesh; a node in two meshes serves a leaf from
  its **primary** mesh's CA (one listener, one cert). Trusting the second
  mesh's CA is offered too, for consoles that reach its nodes directly.
- `aspen tls ca` prints the CA (fingerprint, names constrained, expiry);
  `aspen tls ca rotate` mints a new one and bumps the root signature.

Crate: `rcgen` (with `x509-parser` for CSRs) — pure Rust, already the
ecosystem's answer, no OpenSSL.

### 3.2 A node's leaf (T-1)

- The node generates `tls.key` (P-256, 0600, never leaves) and computes
  its SAN set (§2.2). It builds a CSR and sends
  `tls_csr {csr, names}` to the root holder over the federation link — a
  new op class `trust`? No: `control`, since only the root answers it and
  membership is already proven by the link. The root verifies the CSR
  signature, that `names` are within the constraints and plausibly the
  requester's (hostname equals the member's node name or its `advertise`
  host; addresses private), signs for 90 days, answers `tls_cert {der,
  chain}`, and records `tls_issue` on the trail.
- The root node signs its own leaf locally. A member whose root holder is
  offline keeps serving its current leaf; renewal is tried hourly from
  day 60, and the Mesh panel shows a chip when a leaf is within 7 days of
  expiry with no root reachable. **Offline path** mirrors enroll/certify:
  `aspen tls request` prints a blob, `aspen tls sign <blob>` where the
  root lives prints the cert, `aspen tls install <blob>` on the node.
- The SAN set is recomputed hourly and on link-up; a change re-requests.

### 3.3 Serving TLS (T-2)

- **Same port.** The listener peeks the first byte of each accepted
  connection: `0x16` (a TLS ClientHello) goes to `tokio-rustls` with the
  leaf and chain; anything else is the plain HTTP path, unchanged. One
  `--listen`, one `daemon.json`, no new firewall rule, and `http://` keeps
  working for the CLI's loopback calls, older consoles and anyone who has
  not trusted yet. A separate `--tls-listen` is the alternative, §6(b).
- The node token rule is unchanged: beyond loopback, every call still
  carries it, over `https` as over `http`. TLS gives confidentiality and
  authenticity of the node; the token still says who may ask.
- `aspen status` and the "node up" line print both URLs. The node's mesh
  advertisement gains `https: ["https://anindor:7420", …]` (one per
  certified name that is not an interface address, plus the addresses)
  so the console can offer direct attach (T-4).
- Session event streams work at `wss://` for free (same upgrade path).
  Node-to-node federation links stay on `ws://` + sealed envelopes; moving
  them to `wss` is a later choice with no security gain (they are already
  end-to-end sealed) and a real cost (a link to a node whose leaf expired
  would fail).
- The certificate reloads on renewal without a restart (rustls
  `ResolvesServerCert` over an `ArcSwap`).

### 3.4 Trust on this computer (T-3)

- `aspen tls trust` — interactive. Verifies the CA against the mesh root
  signature, prints the fingerprint and the exact stores it will touch on
  this platform, asks once, then runs the recipes of §2.4 in order:
  OS store (Windows user Root / macOS login keychain / Linux NSS for
  Chrome), Firefox profiles it can find, then prints the system-anchor
  commands (Linux) rather than escalating. On WSL it targets the Windows
  store through interop and, if `certutil` exists in WSL, the Linux NSS
  DB as well. `--check` reports per store; `--remove` reverses what it
  did; `--print` writes the PEM for hand installation. `aspen tls root
  --out mesh.crt` exports.
- `GET /api/tls` → `{ca: {fingerprint, subject, not_after, root_sig_ok},
  leaf: {names, not_after, issued_by}, stores: [{name, installed,
  writable, needs_terminal}]}`; `POST /api/tls/trust` runs the same
  installer for the stores marked writable and answers what happened
  (loopback or token, like every other call; the platform dialog is the
  consent). `GET /api/tls/root.crt` serves the CA as a download.
- Servicing: `aspen up` on a member that has a leaf but whose machine has
  never run `tls trust` says so once in `aspen status` (a hint, like the
  firewall hint in RELAY.md §11).

### 3.5 The console (T-4)

- **Meshes page, per mesh:** a *Certificate* row — "Trusted on this
  computer" with the fingerprint, or **Trust** (calls `POST /api/tls/trust`
  on the node the console is attached to when that node is this computer,
  i.e. the attached URL is loopback; otherwise **Download root
  certificate** with the platform steps folded under it). The row is
  computed from `GET /api/tls`, so it reads the same over the relay.
- **Attach page:** the URL field accepts `https://`; when a direct
  attach fails and the URL is `https://`, the error card says the browser
  does not trust the certificate yet and offers the two actions above.
  (The browser gives JS no reason for a TLS failure, only a `TypeError`;
  the guidance is conditional on the scheme, not on a diagnosis.)
- **Mesh list, node rows:** the advertised `https` addresses as chips;
  "open directly" when the console is a secure context and the row's
  node is trusted (a HEAD to `/api/health` succeeds).
- `api.ts`: `apiBase` and the events URL already derive scheme from the
  attached URL; verify `wss`.

### 3.6 Direct first, relay when away (T-5, stretch)

With advertised `https` addresses and a trusted CA, a console attached
through the relay can probe a node's direct addresses (HEAD
`/api/health`, 1 s) and, on success, switch its requests for that node
to direct while keeping the relay registration for presence and for the
nodes it cannot reach. Fall back on the first failure. This is the
"LAN when home, relay when away" behavior a phone wants. It touches the
tunnel's per-node routing and deserves its own round after T-1..T-4 are
in use; §6(e).

### 3.7 Docs and verification

- New `docs/TLS.md` (reference: the CA, leaves, stores per platform,
  the offline blobs, rotation); pointers from RELAY.md §11, MESHES.md
  §1/§5, AUTOSTART.md (service context cannot show the Windows dialog),
  API.md rows, DESIGN.md log.
- Rig: j2 (root, `rigmesh`) mints the CA; a member requests a leaf over
  the link; `curl --cacert` succeeds on the WSL `eth0` address; the
  hosted bundle in the `relaytest` Chrome profile attaches directly to
  `https://<eth0>:7695` after `certutil` into that profile's NSS DB;
  `navigator.locks` is defined there (secure context); a session's events
  stream over `wss`; expire the leaf artificially and watch renewal;
  stop the root and watch the "no root reachable" chip. Windows and macOS
  recipes verified on the real mesh (anindor, macbook) before tagging.

## 4. Rows

| # | Row | Scope |
|---|---|---|
| T-1 | **The mesh CA and node leaves.** P-256 CA beside the root, bound by an Ed25519 signature and carried in rosters; CSR over the link, 90-day leaves, hourly SAN check, renewal, offline blobs, `tls_issue` on the trail. | aspen-wire, aspen-node (mesh, federation, store), aspen CLI |
| T-2 | **TLS on the listener.** Same-port sniff, rustls with live reload, `https` in advertisement, status and node-up lines, `wss` events. | aspen (api), aspen-node |
| T-3 | **Trust on this computer.** `aspen tls trust/--check/--remove/--print`, `GET /api/tls`, `POST /api/tls/trust`, `/api/tls/root.crt`, recipes for Windows, macOS, Linux (NSS, system, Firefox), WSL → Windows; the status hint. | aspen CLI, aspen (api), aspen-node (platform) |
| T-4 | **The console side.** Certificate row per mesh, https attach guidance, root download with phone steps, direct-address chips, `wss` verified. | console |
| T-5 | *(stretch)* **Direct first, relay when away.** | console tunnel |

Order: T-1 + T-2 together (a leaf nobody serves is unverifiable; a
listener with nothing to serve is inert) as one release; T-3 + T-4 the
next; T-5 its own.

## 5. The rest of the slate

Rows still open after v0.40, ranked by how well they ride with the
headliner:

- **D-5 Consoles as first-class + QR onboarding.** The natural partner:
  the QR on the Mesh panel can carry the root certificate's fingerprint
  and the direct `https` address, so a phone's onboarding is *scan → trust
  → attach* with no relay for the home network. Recommended for this
  slate, after T-3/T-4.
- **R-1 The macbook relay-dial noise** (anindor-wsl dialing macbook via
  relay and timing out on hello whenever the Mac's direct link drops;
  logged, not on the backlog). Small, and the mesh's own logs are noisy
  because of it. Recommended.
- **P-3 Relay preference ordering.** Small; consumes the ordered list
  the config already keeps. Optional filler.
- **D-6 Non-extractable console identity.** Security hardening that
  pairs with trusting a root in the browser; medium. Optional.
- **N-5 / D-8 A second operator, viewer-grade links.** Larger; its
  identity questions are better asked once direct https exists (the
  console's mesh identity could authenticate direct calls instead of the
  node token). Defer to its own slate.
- **N-2, N-3, N-4** (queues, schedules, budgets), **G-7/G-8**, **H-2**,
  **D-7**, **D-9**, **P-2** — unrelated to this headliner; stay open.

Suggested shape: **v0.41** T-1 + T-2 · **v0.42** T-3 + T-4 + R-1 ·
**v0.43** D-5 (+ P-3) · T-5 when the first three have been lived with.

## 6. Decisions for the operator

- **(a) Name constraints.** Recommended: IP ranges constrained to private
  space, DNS unconstrained by default, `--dns-suffix` opt-in at CA
  creation. Alternative: also constrain DNS to `.local` + the tailnet
  suffix, which means bare hostnames (`anindor`) will not validate.
- **(b) Port.** Recommended: same port, first-byte sniff. Alternative: a
  second `--tls-listen` port (simpler code, one more thing to forward
  and to allow through the Windows firewall).
- **(c) Leaf life.** Recommended 90 days, renew from day 60, offline
  blobs as the fallback. Alternative: 1 year, no renewal machinery in the
  first cut.
- **(d) Trust from the console.** Recommended: `POST /api/tls/trust`
  exists, but the console only offers the button when it is attached to
  a loopback node (this computer); otherwise it offers the download and
  steps. Alternative: CLI only.
- **(e) T-5** in this slate as the fourth release, or parked.
- **(f) The rest of the slate:** D-5 and R-1 in, P-3 filler, D-6 optional.
