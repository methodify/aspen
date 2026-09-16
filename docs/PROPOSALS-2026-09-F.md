# Proposal: one console, several meshes

**Status:** approved 2026-09-15 (§7); shipped as v0.29 (F-1..F-5) and
v0.33 (§8, F-6..F-8). Reference: CONSOLE_APP.md §7.

## 0. The ask, and the decision it rests on

"Nodes belong to one mesh — I'm bought in. But the hosted console is not
a node. Why can't it hold several identities and switch between meshes?
I'll use it that way at work and at home, both meshes on the same
Cloudflare relay, though that need not be a given."

The decision underneath is MESHES.md's and stays: **a node is a member
of exactly one mesh.** Two meshes on one node means policy — what
propagates where, whose rules and plugins apply, which roster a peer
sees — and the answer we chose was "run two nodes". Nothing here touches
that.

The console is a different animal. It is a *reader and controller*, it
propagates nothing, and its membership in a mesh is a cert the mesh root
signed for its keypair. Holding two such certs and talking to one mesh
at a time is not cross-traffic; it is a person who works in two places.
The UI can be plural where the node must be singular.

## 1. Where the console stands today

The hosted console (`methodify.github.io/aspen`, CONSOLE_APP.md §3)
already has most of the pieces, each built for one mesh:

- **One identity.** `aspen.console.identity`: one keypair, one cert, one
  `known` cert table, one relay. The cert names the mesh; the relay
  challenge signs over that mesh name; a node accepts the hello only if
  the cert verifies against *its* root. A second mesh needs a second
  cert, and today there is nowhere to keep it.
- **One tunnel config.** `aspen.console.tunnel` is enabled/relay/node —
  the single relay peer the console is attached to.
- **A list of connections** (`aspen.connections`, PROPOSALS-D §3):
  direct (loopback + token) or relay (relay + node), one active,
  switching reloads the page. The list has no notion of which mesh a
  connection leads into; it does not need one while there is one mesh.
- **Unscoped browser state.** Working set, notice preferences and
  cursors, catch-up markers (`aspen.seen.<agent>`), drafts
  (`aspen.draft.<agent>`), dismissed spawn notes, the Library's repo
  and open-nodes memory, push kinds — all keyed by agent or repo names
  that are unique *within a mesh*. Two meshes each with a `hub` repo or
  an agent named `fix-ci` would trample each other.
- **One push subscription per browser** (the platform's rule: one per
  service-worker registration), registered on the node the console
  talks to under the console's name; that node's peers fan notices out
  to it. The service worker opens a notice's link as a hash route with
  no idea which mesh it belongs to.

And the relay is already plural. The Cloudflare worker keeps **one
Durable Object per mesh**, admits by that mesh's root key (`MESH_ROOTS`
JSON, or `MESH_ROOT_<name>`), and the node-hosted relay admits any mesh
its node is in. Two meshes on one worker is a configuration, not a
feature: add the second root key. A console registering in both is two
sockets to two objects, which is exactly what two nodes would be.

So the work is entirely on the console side, plus one field in the push
payload.

## 2. Product design

### 2.1 The unit is the mesh, not the connection

Today the console asks "which connection?". The question the operator
actually has is "which mesh am I looking at?". A connection is *how* you
get in; the mesh is *where* you are. So:

**A profile is one mesh as this console sees it:** the mesh name, this
console's identity in that mesh (keypair + cert + known certs), the
connections that lead into it (a relay peer or a loopback node), which
of those is preferred, and everything the browser remembers about that
mesh (working set, cursors, drafts, preferences). Profiles are named by
their mesh; a friendly label is optional ("work", "home").

Internally "profile"; in copy, always the mesh's own name. The word
"profile" does not appear in the UI.

### 2.2 The switcher

The mesh name becomes a permanent fixture of the frame: the status bar,
which the phone layout keeps (the rail becomes the bottom bar there). One mesh: it is a label
(today the console does not say which mesh you are in at all — it
should, even before this round). Two or more: it is a menu.

Each row: mesh name, the connection in use and its state (tunnel up /
direct / unreachable), and — Tier 2 — a needs-you count for the meshes
you are *not* looking at. Last row: "Connect to another mesh…".

Switching swaps the active profile and reloads, as connection switching
does today (§4 says why, and what a live switch would take). The route
is kept when it makes sense (`/now`, `/mesh`, `/search`) and dropped
when it names something mesh-specific (`/session/<agent>`).

### 2.3 Connecting to another mesh

The Connect page becomes the **Meshes** page: one card per profile
(identity status, connections, remove), and the attach flow underneath
it creates a *new* profile rather than overwriting the one identity.

Adding a mesh through a relay is today's three steps run again with a
fresh keypair: show the enroll blob, the mesh root runs
`aspen mesh certify`, paste the bundle. The bundle's cert names the
mesh; that names the profile. D-5's QR onboarding, when it lands, does
exactly this in one scan — a second mesh is the same gesture as the
first, which is the test that the model is right.

Adding a mesh through a direct connection (loopback node + token): the
console asks the node its mesh (`/api/mesh` answers `mesh` and
`in_mesh`) and files the connection under that profile, creating it if
new. A node in no mesh gets a profile named after the node, marked "not
in a mesh". The Meshes page shows which connections lead into which
mesh; a direct connection cannot be filed wrongly because the node says
where it is.

### 2.4 Keys: one per mesh

A console could carry one keypair certified twice. Recommended: **a
keypair per profile.** Reasons: certification is per mesh anyway, so the
operator gains nothing from sharing; separate keys mean one mesh root
never sees a public key it can correlate with another mesh's console
list (D-5 will list consoles per mesh — the work mesh should not learn
what the home mesh calls this laptop); and revoking in one mesh (D-5)
touches nothing in the other. The console name (`console-<suffix>`) is
per profile for the same reason. D-6 (non-extractable WebCrypto keys)
composes unchanged: a `CryptoKey` per profile.

### 2.5 Push across meshes

One subscription per browser, registered in each mesh: the console
subscribes on its node in every profile it holds (the Meshes card shows
"push on" per mesh; the bell's box is per mesh). Both meshes then
deliver to the same endpoint, and each mesh's fan-out (NOTIFICATIONS.md
§6) stays within its own mesh — no node learns of the other.

**The sender key has to move with the subscription.** A push
subscription is bound to the VAPID key it was made with; a second mesh's
node, holding its own key, could not deliver to it at all (the push
service answers 403). So the key becomes the *console's*: made in the
browser (WebCrypto P-256, kept browser-wide), the subscription is made
with its public half, and the private half is handed to every node the
console subscribes on, stored on the subscription row. A node with no
such key on a row (older consoles) sends with its own, as before. The
key authorizes nothing but pushes to this browser, which the node could
already do. A subscription made with a node's key is replaced the first
time the console subscribes after this change.

The push payload gains `mesh` (the node knows its own). The notification
body reads "on dev009 · work". A click opens the link *in that mesh*:
the service worker appends the mesh to the target route; the app, on
load, switches profile if the link's mesh is not the active one, then
navigates. Without this, a tap on a home notice while looking at work
would open the wrong session or a 404 — the one place two meshes would
visibly collide.

`push_subs` on the node is unchanged: the subscription row is keyed by
the console's name in that mesh, which is per profile.

### 2.6 The node-served console stays single

A console a node serves at `http://node:7420` is that node's window.
The operator's instinct is right: keep it tied. It gains the mesh label
(§2.2) and nothing else. The profile model lives in the hosted build
only (`__ASPEN_HOSTED__`), which is also where the code paths already
diverge.

### 2.7 Migration

A console that already has an identity, a tunnel config and connections
wakes up with exactly one profile, named by the cert's mesh (or by the
direct node's mesh, asked on first load; or "default" until it can ask),
holding everything it had. Unscoped keys are moved under the profile on
first run and read from either place for one release. Nothing to redo;
the switcher shows one row.

## 3. Storage

Every per-mesh key gets the profile id in it: `aspen.p.<pid>.<key>`.
Per mesh: identity, tunnel, connections, working set, notice prefs and
cursors, seen markers, drafts, spawn-note dismissals, Library repo and
open-nodes, push kinds, render mode. Browser-wide: theme, rail width,
recap-on-return, the profile list and the active profile.

`connections.ts` and `tunnel.ts` read the active profile's slice;
nothing else in the console changes its calls. One module,
`profiles.ts`, owns the list, the active id, the migration and a
`scoped(key)` helper; every existing `localStorage.getItem("aspen.…")`
for a per-mesh key goes through it. That is the bulk of the diff and it
is mechanical.

## 4. What it points to

- **The mesh label everywhere.** Even one-mesh consoles should say which
  mesh they are in. The node-served console too. Cheap, and it makes the
  switcher unsurprising when a second row appears.
- **Peek (Tier 2).** Needs-you counts for inactive meshes in the
  switcher. The tunnel is a class; a second instance per inactive
  profile, connected only to poll `/api/inbox` every minute, costs one
  WebSocket per mesh. Push already covers "something needs you" with
  the app closed; peek covers it with the app open on the other mesh.
- **Live switch (Tier 2).** Without a reload: stop the tunnel, swap the
  profile, restart the tunnel, invalidate every query. Every hook is
  already keyed by request path, so a query-client reset is most of
  it; the risk is a poll started before the swap landing after. The
  reload is honest and takes under a second; live switch is polish.
- **D-5 (consoles first-class).** "Name this console" belongs on the
  Meshes card, per mesh, and the root's `aspen consoles` list shows it.
  Per-profile keys are what make revoke-in-one-mesh clean.
- **D-8 (viewer links).** A viewer-grade cert is a profile with an
  observe-only grant; a colleague opening a share link gets a profile
  for that mesh alongside their own. The model already fits.
- **A second relay.** The operator expects both meshes on one worker;
  the profile records its own relay, so two workers is the same code.
  Worth one sentence in RELAY.md: one worker serves as many meshes as
  it has root keys for.

## 5. The slate

| id | ask | notes |
|---|---|---|
| F-1 | **Profiles.** `profiles.ts`: list, active, per-profile identity/tunnel/connections, scoped storage for every per-mesh key, one-time migration of the single-identity console. | §2.1, §2.7, §3 |
| F-2 | **The switcher and the mesh label.** Mesh name in the rail head and the phone bar; a menu when there is more than one; route kept or dropped on switch. | §2.2 |
| F-3 | **Meshes page.** Connect becomes Meshes: a card per profile, attach flow creates a new profile with its own keypair, direct connections filed by the node's answer. | §2.3, §2.4 |
| F-4 | **Push in every mesh.** Subscribe per profile; `mesh` in the payload; the service worker and the app open a notice in its mesh, switching first. | §2.5 |
| F-5 | **Docs.** CONSOLE_APP.md §7; RELAY.md: one worker, many meshes; NOTIFICATIONS.md §6 note. | |

Tier 2, after the round settles:

| id | ask | notes |
|---|---|---|
| F-6 | **Peek.** Needs-you counts for inactive meshes via a background tunnel per profile. | §4 |
| F-7 | **Live switch.** No reload. | §4 |
| F-8 | **Name this console** per mesh, ahead of D-5. | §4 |

F-1..F-5 in one round. F-1 is the only risky piece (every storage read
in the console moves); the rest is small. (F-, not M-: M-1..M-3 were the
MCP round, v0.24.)

## 6. Open questions

1. **Keys per mesh** (recommended, §2.4) or one keypair certified in
   each? Per mesh costs one extra enroll per mesh, which the operator
   does once.
2. **Reload on switch** for this round, live switch later? Recommended
   yes; the reload is what connection switching does today.
3. **Node-served console stays single** (§2.6)? Recommended yes, per
   the operator's own read.
4. **Should peek (F-6) ride in this round?** It is the feature that
   makes two meshes feel like one console rather than two tabs. It is
   also a second always-on socket per mesh from a phone. Recommended:
   next round, after living with the switcher for a week.

## 7. Decisions (2026-09-15)

1. Keys per mesh — yes.
2. Reload on switch this round; live switch later (F-7).
3. The node-served console stays single — yes.
4. Peek rides the Tier 2 round with live switch.

And the operator's own addition, which is §3 made a rule: every stored
item that is about a mesh — localStorage, the IndexedDB transcript
cache, anything later — carries the profile in its key, so nothing
leaks between meshes whether the switch reloads (now) or not (F-7).
Browser-wide keys are the short list in `profiles.ts`; everything else
is scoped by construction.

Found while building: the VAPID binding (§2.5, second paragraph), which
was not in the draft. And a notification tapped while the console is
already open is a hash change, not a load, so the app follows a link's
mesh on `hashchange` as well as at start.

## 8. Tier 2 (2026-09-15, v0.33)

- **F-7 live switch.** No reload. `switchTo` sets the active profile,
  bumps a switch generation, and `main.tsx` keys the whole app on it:
  every poll and every piece of React state starts over against the new
  mesh, and the tunnel restarts with that profile's identity and relay.
  Module-level caches key by profile themselves — the transcript cache
  (in memory and IndexedDB) does — which is the scoping rule from §7
  earning its keep. A push link's `mesh=` switches the same way.
- **F-6 peek.** `peek.ts`: every profile *not* in view gets a reader —
  a fetch to its direct node, or a second `Tunnel` into its relay with
  that profile's identity — polled every minute for the operator inbox.
  The switcher row shows the count as a badge, *nothing waiting*, or
  *unreachable: why*. One socket per mesh while the console is open; push
  answers the same question while it is closed.
- **F-8 name this console.** The identity's name is chosen when it is
  made — `console-<slug>` from a field beside *create identity*
  ("bryons-phone"), random when blank — because the cert carries it. That
  is what the mesh's nodes list the console as (`/api/mesh` `consoles`),
  and what D-5 will name, revoke and show last-seen for. A profile's
  label stays a local nickname.
