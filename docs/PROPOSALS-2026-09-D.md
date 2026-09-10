# Proposal: the console as an app — installable, resilient, hosted

**Status:** Tier 1 shipped 2026-09-09 as v0.26 (decisions in §8); as built: CONSOLE_APP.md. Three ideas from the
operator (a manifest so the app installs; a service worker, if it earns
its place; the console hosted on GitHub Pages so anyone can point it at
their relay or node) taken apart, product-designed, and carved into a
slate.

## 0. Where the console stands today

The console reaches a node three ways, and the differences drive
everything below.

| how | origin | auth | what works |
|---|---|---|---|
| **served by its node** (`aspen up`, embedded in the binary via rust_embed) | the node's own `http://host:port` | same-origin; token only beyond loopback | everything, including binary bodies |
| **served by one node, addressing another** | same | mesh proxy ops | everything a peer allows (observe / control / spawn / trust per MESHES.md) |
| **console as a peer** (`/attach`, RELAY.md §11) | wherever the HTML came from | the browser's own ed25519/x25519 identity, root-certified; sealed envelopes; relay reads nothing | REST + live events over `api_req`/`sub`; observe + control, never spawn or trust; binary bodies come back base64 |

Facts that matter: the UI version is the workspace version and the
status bar already flags skew against the daemon serving it; routing is
path-based (`BrowserRouter`); there is no `manifest`, no service worker,
no CORS on the node; notices already use the `Notification` API from an
open tab (NOTIFICATIONS.md §2); the console identity lives in
`localStorage` as raw key bytes (`@noble` needs them); the Cloudflare
worker serves exactly `/healthz` and `/relay`.

## 1. Installable: the manifest

**What it buys.** A window of its own with the mark in the dock and the
task switcher, launched like an app, kept out of the tab pile. That is
the visible part. The parts that change behaviour:

- **Badging.** `navigator.setAppBadge(n)` puts the needs-you count on
  the dock icon. That is the one number the fleet view exists to
  surface, now visible without the window. Cleared when the count is.
- **Shortcuts.** The manifest's `shortcuts` give the icon a right-click
  menu: Now, Search, Boards, the last session. Cheap, and the kind of
  thing that makes an app feel like one.
- **Protocol handler.** `web+aspen://session/<agent>` registered by the
  installed app: a link in a notice, a bus message, a chat, opens the
  session in the app. Also the vehicle for onboarding (§3).
- **`display: standalone`**, `theme_color` matching the mark, icons at
  192/512 from `aspen-mark.svg` (maskable variant with padding).

**The catch: origin.** An install is per origin. Served by its node, the
console is `http://127.0.0.1:7420` on this machine, `http://192.168.1.9:7421`
for the laptop, something else through a tunnel — three installs of
"Aspen" that are three different things, and none of them survives the
node moving port. The manifest is worth shipping on the embedded console
(the local one is the common case), but the app *as an identity* only
becomes one thing with a hosted origin (§3). Note also that `http://`
origins other than loopback are not installable at all: the LAN-address
case never gets an install prompt. Another push toward §3.

## 2. A service worker: what it would be for, and what not

Honest list, with the non-benefits first.

- **Not for caching API data.** A stale fleet is worse than an empty one;
  every surface polls, and a cached `GET /api/agents` would show live
  agents that are not. No runtime caching of `/api`.
- **Not needed for the relay tunnel.** It is a WebSocket from the page;
  a worker would add nothing.

What it is for:

- **The shell survives the node.** Today, reload during `aspen update
  --restart` or a relaunch and the page is blank until the daemon is
  back; the installed app opens to an error. With the app shell
  precached, the console opens, says *node restarting… back in 8 s*
  (it already knows how to poll `/api/node`), and meanwhile the relay
  tunnel and the other nodes are still reachable. For the hosted console
  it means opening offline to the connection switcher rather than a
  browser error.
- **The UI's own update flow.** Precache by build hash; on a new build,
  the classic "a new version is ready — reload". For the embedded
  console this is what the status bar's *ui 0.24.6 — reload* already
  does by hand; for the hosted console it is the only mechanism.
- **Web Push (the real prize, §4).** Notices as OS notifications while
  no tab is open, on a phone. That needs a worker to receive them.
- **Badge while closed.** The push handler sets the badge, so the dock
  count is right even when the window is not.

Scope rule: precache the shell (HTML, JS, CSS, the mark), network-only
for `/api`, and the push/badge handlers. No `navigationPreload` tricks,
no background sync — nothing to sync.

## 3. Hosted: the console as a static site

**The idea.** `https://methodify.github.io/aspen/` (a `gh-pages` branch
built by the release workflow from the same commit as the binaries; a
separate `aspen-ui` repository is not needed). Open it, connect it to
your mesh, use it — from a machine with no node, a phone, a locked-down
laptop.

**How it connects.** The origin is `https`, which rules and shapes the
modes:

1. **Through a relay — the primary mode.** The existing console-as-peer
   flow, unchanged: `wss://` to the relay (Cloudflare or a node's own
   `/api/federation/relay` if that node has TLS in front of it), sealed
   end to end, no CORS, no mixed content. What is missing is polish:
   today `/attach` is a page; hosted, it is *the front door*.
2. **Direct to a local node.** `http://127.0.0.1:7420` is a "potentially
   trustworthy" URL, so an `https` page may fetch it; the LAN address
   (`http://192.168…`) is mixed content and blocked, full stop. So
   "direct" means *the node on this machine*, which is a real case (the
   installed app on the operator's own laptop). It needs two things on
   the node: CORS for an allow-list of console origins (`aspen config
   console-origins https://methodify.github.io`; default: the official
   host), and Chrome's Private Network Access preflight answered
   (`Access-Control-Allow-Private-Network: true`). Token as today, over
   the header.
3. **Not:** a node on the LAN by IP. Say so in the connection screen and
   point at the relay.

**Connections, plural.** The hosted console keeps *connections* — a
named list: `laptop · local node`, `home mesh · via relay → BRYON-MINI`,
`work · via Cloudflare → dev009` — with a switcher in the status bar
where *via relay → node · up* sits today. Each connection is either
`{url, token}` or `{relay, node}` bound to the console identity. Deep
links carry one: `…/aspen/#/connect?relay=wss://…&mesh=home&node=BRYON-MINI`,
which is also what the QR code in §5 encodes.

**Routing.** GitHub Pages serves `404.html` for unknown paths; either the
redirect trick or a hash router. A hash router under a build flag
(`VITE_HOSTED=1`) is the plain answer; `?at=`-style links keep working
inside the hash.

**Version skew becomes a contract.** The hosted console is always the
latest; the nodes are whatever they are. Today the UI and daemon move
in lockstep and skew is a warning. Hosted, the console must *work*
against nodes a few versions back: the API gains a version in
`GET /api/node` (it has one) and the console a minimum it supports; a
node below it gets a banner, not a broken page; features gate on
`capabilities` and on presence of a field, never on version arithmetic.
The embedded console stays: it is the one that always matches, works
offline, and needs no account with anyone. The hosted one is *the app*.

**Also possible: the relay hosts it.** Cloudflare Workers serve static
assets; the same bundle deployed with the relay makes
`https://<your-relay>/` your console, same origin as the relay, private
to you. Same build, second deploy target. Pages first; this is a
`wrangler` config change once the bundle exists.

**Security, plainly.** A hosted origin is a trust anchor: whoever can
change what Pages serves can read every connected console's keys and
tokens. Mitigations, in order of weight: the site is built by the release
workflow from a tagged commit, reproducibly, and the release notes carry
the bundle hash; anyone can self-host the same bundle (the release zip
gains `console.zip`); the console identity moves from raw bytes in
`localStorage` to WebCrypto `CryptoKey`s marked non-extractable (Ed25519
and X25519 are in WebCrypto in current Chrome, Safari and Firefox), so
even script running in the origin cannot read the private keys — it can
use them, which is the residual risk, and the node still grants a console
observe + control only, never spawn or trust. Tokens for direct
connections stay secrets in `localStorage`; the connection screen says
what that means.

## 4. Push: notices with the app closed

The node already decides what is a notice (NOTIFICATIONS.md §1) and
already sends an outbound hook. Web Push is one more outbound: the node
holds a VAPID key pair (generated once, in the data dir), each console
that opts in registers its push subscription with the node it is
connected to (`POST /api/push/subscribe`, sealed over the tunnel or
direct), and the node POSTs `needs_you` / `turn_ended` / `exited`
notices to the browser's push endpoint (Mozilla, Google, Apple — all
plain HTTPS with a VAPID JWT; a Rust daemon does this fine; the
`web-push` crate exists). The service worker shows the notification,
sets the badge, and a click opens the session through the protocol
handler or the hosted URL.

Consequences to design in: a node needs outbound internet to push (a
headless box on a LAN may not have it — then its notices push via the
node the console is attached to, which relays them over the mesh, which
the notices bus already does); per-console notice filters reuse the
existing preferences; subscriptions are per console identity, listed
and revocable on the node (`aspen consoles`, and in the Mesh panel's
consoles list).

## 5. What it points to

Once the console is a thing you install and connect, these follow:

- **Onboarding from a phone.** `aspen mesh certify` for a console is a
  copy-paste of an enroll blob today. Hosted + protocol handler + a QR
  code on the node's Mesh panel (encoding the connect deep link plus a
  one-time enrol token) makes it: scan, tap, approve on the root node,
  connected.
- **Consoles as first-class.** Name them ("Bryon's phone", "living-room
  laptop"), see them in the Mesh panel with last-seen, revoke one. The
  cert list exists in memory; it needs a row on disk and a CLI verb.
- **Viewer-grade links.** A cert with an *observe-only* grant (MESHES.md
  §4 already classes ops) is a share link for a colleague: watch a
  session, never touch it. This is N-5 (second operator) approached
  from the console side, and probably the right first step for it.
- **The install as the answer to "how do I get Aspen on this machine".**
  For a machine that runs agents: install the binary, autostart. For a
  machine that only *watches*: install the app. Two sentences on the
  README front page.

## 6. The slate

Tier 1 — one round, v0.26:

| id | ask | notes |
|---|---|---|
| D-1 | **Installable console.** Manifest, icons from the mark, standalone display, shortcuts, `web+aspen://` handler, dock badge = needs-you count. Served embedded and hosted alike. | §1 |
| D-2 | **Service worker: shell and update.** Precached shell, `/api` network-only, "new version — reload", the shell opens while the node restarts. Foundation for D-4. | §2 |
| D-3 | **Hosted console.** `gh-pages` from the release workflow (`VITE_HOSTED`, hash router, `console.zip` in the release); connections list and switcher; `/attach` becomes the front door; node CORS + Private Network Access for direct-to-local; minimum node version banner. | §3 |
| D-10 | **A phone layout.** The console on a phone: the sidebar becomes a bottom bar, the mesh column folds away, Now / Session / Search / notices / the needs-you flow work one-handed; boards, the map and the library stay desktop and say so. Without it the hosted and installed console is a desktop story only. | §3, §5 |

Tier 2 — v0.27:

| id | ask | notes |
|---|---|---|
| D-4 | **Web Push.** VAPID keys on the node, subscriptions per console, notices as OS notifications with the app closed, badge from the worker, relay-through-the-mesh for nodes without outbound internet. | §4 |
| D-5 | **Consoles as first-class + QR onboarding.** Named consoles on disk, `aspen consoles`, revoke; QR on the Mesh panel encoding the connect deep link and a one-time enrol token. | §5 |
| D-6 | **Non-extractable identity.** Console keys as WebCrypto `CryptoKey`s; migration from the `@noble` bytes on first run. | §3 security |

Tier 3 — backlog:

| id | ask | notes |
|---|---|---|
| D-7 | **Relay-hosted console.** The same bundle served by the Cloudflare worker as its `/`. | §3 |
| D-8 | **Viewer-grade share links.** Observe-only console certs as the first cut of N-5. | §5 |
| D-9 | **API compatibility contract.** Written minimum-version policy and a compatibility check in CI against the previous minor. | §3 skew |
| D-11 | **TLS for nodes.** So a hosted console can reach a LAN node directly. The mesh root already certifies nodes; a root acting as a CA for TLS certs is the natural thought, with the catch that browsers trust no private CA until it is installed on the device (a one-time step per device, which the QR onboarding of D-5 could carry). The alternative is ACME through a tailnet or public name. Watch for the approach as D-3/D-5 land. | §3 |

**Recommendation.** Do Tier 1 as one round in the order D-2, D-1, D-3:
the worker first because the manifest's install experience is hollow
without a shell that survives a restart, and hosting last because it
depends on both and on the node's CORS. D-4 is the feature the operator
will feel most, and it is a clean second round once subscriptions have
somewhere to live.

## 7. Open questions for the discussion

1. **Pages vs. relay-hosted first?** Pages is public and shared; the
   relay-hosted one is private to a mesh. The proposal says Pages first
   because it exists without a Cloudflare account. Agree?
2. **How much should the hosted console do without a relay?** Direct to
   `127.0.0.1` only. Is a TLS story for nodes (Let's Encrypt via a
   tailnet name, or a self-signed cert the console pins) worth a row, or
   is "use the relay" the answer?
3. **Push scope.** `needs_you` only by default, everything else opt-in?
4. **Name.** The installed app is "Aspen"; the hosted path is
   `/aspen/`. `aspen-ui` as a repository name suggests the UI is a
   separate product; the proposal keeps it in this repository and this
   release train.

## 8. Decisions (2026-09-09)

- **Pages first.** A hosted place anyone can launch, pointed at their
  own relay. Relay-hosted (D-7) stays backlog.
- **Progressive on connectivity.** First shipment: localhost direct and
  a cloud relay. TLS for nodes is D-11 in the backlog, with the
  root-as-CA thought recorded; an approach may pop out as D-3 and D-5
  are built.
- **Push defaults to `needs_you`**, everything else opt-in.
- **Same repository, `/aspen/` on Pages.** The compiled UI is published
  from a `gh-pages` branch by the release workflow; no second repo.
- **A phone layout is in scope for this push** (D-10, Tier 1).
