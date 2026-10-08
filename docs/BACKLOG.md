# Backlog

Feature requests and design candidates, tagged and dated. Each entry names
the ask as the operator put it, the swath it belongs to, and where its
design lives once one exists. Shipped items are summarized here with the
tag they carried and detailed in the decisions log, DESIGN.md §14b.

Tags: `console` (the web UI), `protocol` (mesh/bus/wire), `sessions`
(a harness process and its store), `servicing`, `docs`.

## Open

| id | tag | ask | notes |
|---|---|---|---|
| H-2 | sessions | **Shared per-node app-server for Codex.** One `codex app-server` hosting every Codex thread on the node instead of one process per session; same adapter, fewer idle processes. Needs a liveness/revive model that does not equate a session with a process. | PROPOSALS-HARNESSES.md §6.1 |
| H-3 | sessions + protocol | **Codex memory convergence.** Codex memory is global (`$CODEX_HOME/memories_*.sqlite`), not per project; decide whether and how it takes part in memory sync (MEMORY.md), or is declared out of scope. | HARNESSES.md §6 |
| H-4 | sessions + console | **Codex plugins and skills in the library.** `$CODEX_HOME/plugins` and skills roots as library scopes beside Claude's marketplaces; activation per mesh / node / repo / session like PLUGINS.md. | PLUGINS.md |
| H-5 | sessions + console | **`turn/steer` as a first-class control.** Today a mid-turn message steers by default; expose "queue for next turn" vs "steer now" in the composer, for both harnesses where they can. | CODEX_RUNTIME_REFERENCE.md §4 |
| H-6 | sessions + console | **Third-party MCP elicitations.** Codex forwards a server's own form/url elicitations; Aspen declines them today. Surface as a prompt kind (`elicitation`) with a generic form card. | HARNESSES.md §1 (`PromptKind`) |
| H-8 | sessions + console | **Codex activity ledger.** `collabAgentToolCall` / `subAgentActivity` items show as tool cards; fold them into the activity ledger (ACTIVITY.md) with counts and a drawer, and read the agent threads' rollouts. | HARNESSES.md §6 |
| H-11 | sessions | **Incremental activity derive.** The ledger is rebuilt from the whole transcript whenever it changed; keep the last offset and parse only the appended tail. Cached at turn boundaries since v0.23.2, so this is cost, not correctness. | aspen-claude `activity.rs` |
| N-1 | servicing | **A supervisor for the daemon.** The `aspen up -d` parent stays as a watchdog: ping, and on silence take a thread dump and restart; peers show a deaf node as such. Parked 2026-09-09: two hangs were bugs, now fixed; revisit if it recurs. | SERVICING.md "Diagnosing a hang" |
| B-5 | console + sessions | **Tell me when a board goes quiet.** A per-board notice when its last busy member goes idle (or a member exits), so a team can be left alone and called back. | BOARDS.md §10 |
| B-6 | console | **A board's spend.** Usage rolled up per board (its members' window cost and tokens) on the board header and the Usage page's grouping. | USAGE.md |
| X-6 | console + protocol | **Chunked bundle transfer through the relay.** Download and upload an `.aspen-repo` from a console on the relay in small sealed pieces (resumable), so a phone on cellular can export or import; today a relay console works with paths on the node. | BUNDLES.md §5 |
| I-1 | node + console | **Transcript images by reference.** Inline base64 images are about half a long session's history bytes; serve them as `{media_type, ref}` with a `GET …/image` endpoint (through the tunnel as blob URLs), so a page carries text and images load as they scroll into view. | API.md transcript |
| I-2 | node + console | **Tool inputs and results on demand.** The session that hit the relay cap (@impl@plank, 2026-10-04) was 79.6 MB: 69 MB tool inputs and results over ~20,000 Bash calls, 0.3 MB images. Send each tool call's input and result trimmed (a few hundred bytes) with a ref, and fetch the full text when a card is opened, so a page holds many more turns. | API.md transcript |
| N-2 | sessions + console | **A queue per session.** Hand a session the next item when it goes idle, from a list the operator keeps or from the bus; pairs with templates and boards. | — |
| N-3 | sessions + servicing | **Scheduled sessions.** A template plus a cron: "every morning, run triage in repo X on node Y." | PLUGINS.md §templates |
| N-4 | console + sessions | **Budgets.** Usage is measured; nothing acts on it. A ceiling per repo or mesh with a notice at 80% and a stop at 100%. | USAGE.md |
| N-5 | protocol + console | **A second operator.** Everything on the bus says "operator"; with a colleague on the mesh: identity, per-person names, and a watch / spawn permission split (console-as-peer certs carry most of it). | RELAY.md §11, MESHES.md |
| D-5 | console + protocol | **Consoles as first-class + QR onboarding.** Named consoles on disk, `aspen consoles`, revoke; a QR on the Mesh panel encoding the connect deep link and a one-time enrol token. | PROPOSALS-2026-09-D.md §5 |
| D-6 | console | **Non-extractable console identity.** WebCrypto `CryptoKey`s (Ed25519/X25519) instead of raw bytes in localStorage; migrate on first run. | PROPOSALS-2026-09-D.md §3 |
| D-7 | servicing | **Relay-hosted console.** The hosted bundle served by the Cloudflare worker as its `/`. | PROPOSALS-2026-09-D.md §3 |
| D-8 | protocol + console | **Viewer-grade share links.** Observe-only console certs as the first cut of N-5. | PROPOSALS-2026-09-D.md §5 |
| D-9 | servicing | **API compatibility contract.** Minimum-version policy and a CI check against the previous minor. | CONSOLE_APP.md §3 |
| G-7 | console + sessions | **Adopt the harness's own plugins into the library.** A plugin Claude reports from its own tree, offered as "manage this in Aspen". | PROPOSALS-2026-09-E.md §3 |
| G-8 | sessions | **Hot reload of a content update** where the harness re-reads plugin dirs (`reload_plugins`), without a restart. | PROPOSALS-2026-09-E.md §3 |
| P-2 | servicing | Release signing (minisign) before `auto` policy is recommended in public. | SERVICING.md §14 |

## Shipped — 2026-10-08: a relay hiccup no longer costs a minute (v0.49.4)

- **A relay hiccup no longer costs a minute** (2026-10-08). The work mesh's root node logged relay links that outlived their transport. After the relay dropped its connection ("Connection reset without closing handshake"), dev009's link stayed up until "link silent for 45s" and came back on the next presence check, about 66 s for a reconnect that took seconds. "peer left the relay; link closed" was followed 37 s later by the real close, and a relay link superseded by a direct one lingered 43 s. One cause: `start_relay_link` kept a strong copy of the link's inbound sender for the link's whole life (only to compare entries afterwards), so removing the entry (session ended, peer left, direct superseded) never closed the channel; only the silence check did. It now keeps a weak reference. Verified on the rig: a relay link superseded by a direct one closed at once, with no "link silent" 45 s later (before: one per node, 40 s on).
- Not ours to fix: the relay connection itself is reset every ~25 min by something below the worker (Cloudflare restarting the object or edge, or the network path). The node redials in 5–10 s; links now follow at once.

## Shipped — 2026-10-08: a pane shows a turn it did not start; context readings are right (v0.49.3)

- **Context readings are right, and the session menu always has one** (2026-10-08). Now's cards often read "ctx 100%" whatever the real fill. The node took the context figure from the turn's result usage, which Claude sums over every request in the turn (a turn of 40 tool calls counts the context 40 times). And with no window reported, the card assumed 200k, though the sessions run on a 1M-context model. The node now reads each request's own usage (Claude's assistant messages; Codex's `token_usage`, which also gives the window, so Codex sessions get a reading at all). The summed figure is only a fallback, and an unreported window over 200k tokens is taken as 1M. The ⋯ menu shows **context** under the name and model: a bar, the percentage and "used of window". It comes from the harness's own account when the page has it, else the node's reading, else why there is none, with *refresh*. The pane's meter stays as it was. Verified on the rig: after a seven-request turn the node reported 44,131 of 1,000,000, exactly what the harness's `/context` gave (4%), and the menu read "4% · 44.3k of 1.0M".
- **A pane shows "working" for a turn it did not start** (2026-10-07). On a board, an agent woken by another agent's bus message streamed its turn into the pane (text, running tool cards) while the status line read "last turn: success". The rail was right; it reads the node's `turn_state`. The pane's busy flag was set only by a send from that page, or once at load. Now the first event of the session's own turn (text, message, tool call or permission prompt; not a subagent's) marks it busy, and the node's `turn_state` is followed all along. Busy there is busy here. Idle there clears a busy that never got its `turn_ended` (a reconnect gap), after 10 s with no events, so it cannot undercut a send from the page. Verified on the rig: a turn started through the API, not the page, read idle → working → "last turn: success".

## Shipped — 2026-10-04: history that scales — compactions, pages that follow them, reading from the end (v0.49.2)

PROPOSALS-2026-10-Q.md Q-1..Q-3; API.md transcript.
- **Compactions show:** a divider across the transcript (both renderers, and the preview), "context compacted · auto · 967.3k → 12.5k tokens", with the summary the harness continued from behind a disclosure. Before, the summary line was dropped and the boundary ignored, so history read as one unbroken conversation. Claude: `compact_boundary` and `isCompactSummary` lines. Codex: `compacted` records and `context_compacted` events.
- **Pages follow compactions:** the newest page is everything since the last compaction (at least 256 KB, at most 8 MB), and *load earlier history* steps back one compaction at a time.
- **Claude transcripts are read from the end:** the node reads a window before the cursor or the end of the file instead of the whole file; a delta finds its line from the end. On a 146 MB transcript: newest page 0.46 s (whole file 2.9 s), earlier pages 0.5–0.7 s, a delta 0.32 s. Codex and replicas keep the whole-file read, with compaction-aware pages.
- Verified on the rig with that transcript (each page began at a compaction; the cursor chain walked back); the divider and summary checked in the browser in both themes.

## Shipped — 2026-10-04: long sessions open over the relay (v0.49.1)

- On the work mesh, opening @impl@plank (87% context) from the phone showed "history: e.entries is not a function" and "no transcript yet", in a board pane, the session view, and on *reload transcript*. A console on the relay reaches its node through the node's HTTP gateway, which read each reply with a 64 MB cap and, past it, answered **200 with an empty body**. The console turned an empty body into `{}` and seeded the transcript from it. A long session's whole history is tens of MB (this session: 32.7 MB, half inline images).
- The gateway now answers an over-cap reply with a 502 that says so. The console treats a history that is not a list as an error ("the node sent no history…"). History comes **a page at a time**: `GET …/transcript?tail_bytes=` returns the newest ~8 MB from a turn's start, with an `earlier` cursor, and **load earlier history** at the top fetches the page before. This works on the local route, the mesh `transcript` op and the replica path; older nodes answer whole, and the console accepts that.
- `scripts/transcript-size [agent…]` prints what each session's transcript costs to fetch from a node (size, items, inline images, largest items).
- Verified on the rig with a 32.6 MB history: the first page was 8.1 MB (1,174 items from a user line), the cursor chain walked back page by page, and the console loaded the newest page and then two earlier pages on request, also from its cached copy after a reload.
- Measured on the work mesh afterwards: @impl@plank's history was 79.6 MB (31,372 items; 69 MB of it tool inputs and results across ~20,000 Bash calls, 0.3 MB images), over the 64 MB cap. Not done here: tool text and images still ride inline; backlog **I-2** (tool text on demand) and **I-1** (images by reference).

## Shipped — 2026-10-02: a plugin library you chose, and every node's plugin state (v0.49.0)

PROPOSALS-2026-10-P.md L-1..L-7; PLUGINS.md §6b.
- Adding the official marketplace put 315 plugins into every plugin surface. Now the **library** (added, named by a rule, or a marketplace of 24 or fewer) is what is offered at hand; the marketplaces are a store to **browse** (search, category and publisher filters, cards) and to **find** from the session menu and template editor. Adding a marketplace asks whether to keep it as a store or shelve it whole.
- **What a plugin brings** (skills, commands, subagents, MCP and LSP servers, hooks) is counted from its files; *look inside* fetches an external one without turning it on. The session menu sums what is on.
- **A work-mesh node never got a marketplace** and *sync now* changed nothing: sync was per node and its errors invisible elsewhere. The Marketplaces tab now shows every node's state; **sync the mesh** pushes the registry to each node and reports back; git cannot prompt or hang and says when a node lacks credentials; registry pulls log failures and merge row by row; directory marketplaces say where they live; the spawn note says why a plugin was left out. Also fixed: concurrent syncs overwrote each other's catalog.
- Verified on the rig (two nodes): three marketplaces (315, 4, and one that cannot authenticate) synced on both nodes from one *sync the mesh*, with the credential message per node; library picks reached the other node; *look inside* counted an external plugin (15 skills, 1 MCP server); a session's menu turned on a plugin found by search; pages checked in both themes and at 400 px.

## Shipped — 2026-10-02: no false "not offered here" on Claude sessions (v0.48.6)

- v0.48.5 flagged every Claude session's default model "not offered here". Claude's model list names aliases (`opus`, `sonnet`), never the resolved id it reports for default, so the check could not match. The flag now applies only when the default is Codex's recorded start model (`default_model`).

## Shipped — 2026-10-02: Aspen runs the Codex the operator's shell runs (v0.48.5)

- On the operator's Windows node every turn of @writer@cloud-nebula failed: "The 'gpt-6.1-sol' model is not supported when using Codex with a ChatGPT account." The operator had picked gpt-6.1-sol in the Codex CLI, which saved it as the default in Codex's `config.toml`. That CLI is npm's codex-cli 0.160.0. Aspen was running the Codex app's bundled codex.exe 0.154.0, further down PATH, because a spawn by bare name on Windows finds only `codex.exe` and npm installs only `.cmd`/`.ps1` shims. 0.154.0's model list has no gpt-6.1-sol, and the backend refused the model to it. The node now resolves `codex` like a shell: the first PATH entry, and for an npm shim, the native `codex.exe` in its package (nested, hoisted and older vendor layouts), with the launcher's variables. `ASPEN_CODEX_BIN` also reaches the version probe now (it was applied after the probe had started).
- The model menu said "default · gpt-6-astra" (the entry Codex's list marks default) while "default" was running the config's gpt-6.1-sol. Codex sessions now record what "default" resolved to at start (`default_model`). The menu names that, and adds "not offered here" when the harness's own list lacks it.
- Verified: resolver unit tests on the npm layouts; on the rig a resumed Codex session reports `default_model` (the thread's own model with none asked for) and still lists its models. The Windows path needs anindor on v0.48.5; check `GET /api/node` there for codex 0.160.0.

## Shipped — 2026-10-02: a long Codex session's menus fill (v0.48.4)

- The operator's Codex pane still offered only "default" for model and mode after v0.48.3. Codex answers `thread/resume` with the whole thread, every turn included, and Aspen kept that answer as the session's handshake. A long session that generates images (@artist@cloud-nebula, 18 turns) came to 147 MB, held three times in the runtime info (handshake, inventory, raw), so `/api/agents/{name}/runtime` was 440 MB and took 13 s even over loopback. The console's request never finished and its menus fell back to their defaults. The node now keeps the thread's settings and the number of turns (`turnCount`), not the turns themselves (the history is read from the rollout). Rig sessions were small, so the earlier checks never saw it. Verified on the rig: a resumed Codex session's runtime response dropped from 39.8 KB to 12.6 KB, still listing all five models.

## Shipped — 2026-10-02: the model menu fills when a session comes up (v0.48.3)

- A session view asked for the runtime (model list, commands, mode) once, when it opened. Opened on a stopped session, it asked a harness that was not running and never asked again after the session was started — the operator's Codex pane kept "default" as its only model after v0.48.2. It now asks again whenever the session comes up (twice, 1.5 s and 6 s after, since the harness lists its models a moment after starting). Verified on the rig: a stopped Codex session's menu went from "default" to all six models when the session was started from outside the page.

## Shipped — 2026-10-02: Codex model picking (v0.48.2)

- A Codex session's model menu offered only "default": the console read the model list from the raw handshake, where Claude keeps it, but Codex's lives in the runtime info (`model/list` at session start). Every Codex session ran on its default (gpt-6-astra). The menu now reads either place; "default" names what it resolves to from Codex's `isDefault` entry. Verified on the rig: the six models listed, gpt-5.6-luna picked, and the next turn ran on it (per Codex's own rollout).
- A Codex turn that ended in an error (that test turn: the machine's Codex sign-in had expired) left nothing in the session's history — the rebuild from the rollout skipped `task_complete`'s error. It now shows as "⚠ Codex: <message>" where the reply would be.

## Shipped — 2026-10-01: the phone stops sliding (v0.48.1)

- A transcript never scrolls sideways: long words break, the bus line's header shrinks, code blocks and tables scroll inside themselves (a long record path made the whole conversation wider than the pane — 649 px in a 368 px pane on the rig).
- Fields are 16 px on touch screens even in a board pane (its composer's own 12.5 px beat the v0.45 rule, so iOS still zoomed on focus), and the page scale is pinned (`maximum-scale=1`).

## Shipped — 2026-10-01: a board at a glance, and small things (v0.48)

- **Board meters** (BOARDS.md §10): one bar per member, busy first, on the rail's board rows (the row's icon, narrow rail included), the Boards page's cards, a board's header, and a new *Boards* band on Now; waiting and background-work pips on board rows; **start the stopped (n)** on a board's header.
- **The tab title's count**: one number, the bell's unseen notices — a remount (mesh switch, reconnect) used to fold the old count into the base title, stacking `(81) (7) (7) …` for ever.
- **H-7**: a Codex prompt's deny is one click (Codex's approval reply carries no reason).
- **H-9**: a harness chip on Usage's session rows (Codex only; Claude is the default).

## Shipped — 2026-09-28: repo bundles — a repo with its context, as one file (X-1..X-5, v0.47)

| id | shipped as |
|---|---|
| X-1 | `repobundle.rs`: `.aspen-repo` (gzip'd tar: manifest, repo per mode, every session canonical, sidecars, memory, names, sums); Claude and Codex; one name per transcript; source times kept. |
| X-2 | Import as a new repo or a top-up: install / replace (`.bak`) / keep / fork on divergence / new id when the node has the session in another repo; memory kept beside; names land as `bare-2` when taken; plan before, report after; every checksum verified. |
| X-3 | `aspen repos export|import`; `POST /api/repos/export[/preflight]`, `GET /api/repos/export/download`, `POST /api/repos/import[/upload|/preflight]`; mesh ops for a peer's repo. |
| X-4 | Console: *export…* on each repo row (modes, sessions, options, seal, download or path), *import a repo…* on the Mesh list (upload or path, new or top-up, preview, import). |
| X-5 | Optional passphrase sealing: scrypt + XChaCha20-Poly1305 in 1 MiB chunks, truncation detected. |

Also: `scripts/relaunch` clears its detach marker before starting the daemon (sessions it revived inherited it, so a relaunch run from one of them ran in the foreground).

## Shipped — 2026-09-28: workflows you can see into (W-1..W-4, v0.46)

| id | shipped as |
|---|---|
| W-1 | The ledger keys a workflow by its Task ID (the notification's), keeps the run id, names it from the script's meta, folds resumes; the run's state file settles its end and carries totals. Runs no longer stay *running* forever. |
| W-2 | The node keeps the streamed `task_progress` per task; a running workflow's row shows its phase, agents done and tokens. |
| W-3 | `GET /api/agents/{name}/workflows/{run}` + `workflow` op (live, state file, or journal + agent transcripts); subagent transcripts found under `subagents/workflows/`. |
| W-4 | The workflow page: phase rail, agents table, reports, logs, script; phone layout. ACTIVITY.md §Workflows. |

W-5 (stop a run) and proposal O's repo bundles (X-1..X-5) are open.

## Shipped — 2026-09-25: the installed console notices a new build, and can be asked to (v0.45.1)

- An installed app rarely navigates, so the browser rarely re-checked the service worker: the console now asks on every return to the foreground and hourly. And an explicit **check for a newer console** action: the hosted bar's version chip, the More sheet's *console vX · node vY* row, and a palette command; *new console ready — reload* when one waits, with a pip on the More button. CONSOLE_APP.md §2.

## Shipped — 2026-09-24: preview a transcript, touch zoom (v0.45)

- **Preview** on every transcript row of the Mesh list and the session's transcripts panel: `/preview?repo&session&harness&node` reads the transcript from the harness store (`session_preview` helper and op; `GET /api/sessions/preview`) and shows the turns read-only — for looking through pre-Aspen repos before deciding to resume; *resume as* starts a name on it from the same page.
- **Touch:** fields are 16px on coarse pointers (iOS no longer zooms on focus), controls opt out of double-tap zoom (`touch-action: manipulation`), the sheet no longer rubber-bands sideways; pinch zoom stays.

## Shipped — 2026-09-24: dismiss on Now, the chamfer's missing edge (v0.44.1)

- Operator mail on the Now page can be **dismissed** per card (`POST /api/needs/read {ids, node}`; the `inbox_read` op takes `ids`), and a card leaves on its own once the operator answers that session (`send_operator_message` marks its pending mail `operator-replied`).
- Every chamfered surface (strips, panels, boards, the class badges) now draws its diagonal edge, so the cut corner no longer reads as a bite out of the border.

## Shipped — 2026-09-24: trust from any device, direct when there is a path (T-6, T-5, v0.44)

| id | shipped as |
|---|---|
| T-6 | Token-free CA downloads over plain http (`root.crt`, `root.mobileconfig` as an Apple profile), `ca.urls` in `GET /api/tls`; the certificate row's device panel (iPhone/Android/Mac/Windows/Linux), QR for a phone, *check* from the browser. TLS.md §8. |
| T-5 | Console tokens minted over the sealed link (`POST /api/console/token`, `/api/mesh/{node}/console-token`, `console_token` op); `x-aspen-peer` believed only from the gateway; the tunnel probes advertised https URLs, routes requests and event sockets direct with the token, falls back to the relay on failure; *direct → node · relay standing by* in the bar; *stay on the relay* on Attach. TLS.md §8. |

## Shipped — 2026-09-23: relay preference order (P-3, v0.43; v0.43.1 fixes a deadlock)

v0.43.0 deadlocked every node whose direct link to a peer dropped: the
fallback-to-relay path held the relay-sessions lock while the new
preference check took it again, and the API hung behind it. v0.43.1
decides before locking and makes the check non-blocking. Skip v0.43.0.

| id | shipped as |
|---|---|
| P-3 | `aspen mesh relay <url> --first`, proposal `relay {url, first}`, *queue prefer* on relay rows; a peer present on several relays is linked through the first by configured order, mail goes to the first relay that is up. RELAY.md §10. |

## Shipped — 2026-09-23: trust on this computer, the console side, relay-link backoff (T-3, T-4, R-1, v0.42)

| id | shipped as |
|---|---|
| T-3 | `truststore.rs`: Windows user Root (native or via WSL interop), macOS login keychain, Chrome's NSS database, Firefox profiles, Linux system anchors (terminal only); `aspen tls trust [--check|--remove|--print|--system|-y]`; `GET /api/tls` `stores[]`, `POST /api/tls/trust`. TLS.md §6. |
| T-4 | `CertificateRow` on the Meshes page (CA, https state, per-store chips, trust button on a local node, download CA / copy PEM / steps), `https ↗` chips on peer rows and this node, https accepted on Attach with certificate guidance on failure. TLS.md §7. |
| R-1 | A relay link to a peer is a dial in the reach memory: a hello that keeps timing out (a laptop asleep behind a stale registration) backs off 5 s → 10 min instead of every 27 s; a fresh presence resets it; repeats log at debug. RELAY.md §9. |

## Shipped — 2026-09-23: the mesh root as a certificate authority (T-1, T-2, v0.41)

| id | shipped as |
|---|---|
| T-1 | A P-256 CA beside the root key, bound by an Ed25519 root signature and carried in rosters; `tls_csr` over the link; 90-day leaves renewed from day 60, names re-checked every tick; `aspen tls request/sign/install` offline; `tls_issue` on the trail. TLS.md §2–§3. |
| T-2 | https on the listener: same port by first-byte sniff, `--tls-listen` for a separate one; live resolver, no restart on renewal; `https_urls` advertised; `GET /api/tls`, `POST /api/tls/renew`, `GET /api/tls/root.crt`; `aspen tls status`. TLS.md §4–§5. |

Closes D-11. T-3, T-4, T-5 above are what it left open.

## Shipped — 2026-09-23: one relay connection per browser profile (C-1, v0.40)

| id | shipped as |
|---|---|
| C-1 | `Tunnel` elects a leader per identity with the Web Locks API; followers multiplex requests and subscriptions over a BroadcastChannel; the lock and the connection pass to another tab when the leader closes. Every tab of a profile is live at once. RELAY.md §8. |

## Shipped — 2026-09-23: a replaced console stands down (v0.39.2)

| id | shipped as |
|---|---|
| relay | The flapping's real close: the relay evicts an older socket of the same console identity as `replaced`, and two views of one identity (two tabs, the app + a tab) evicted each other forever. A replaced tunnel now waits until its tab is visible and focused before reconnecting; *reconnect* on the Meshes page forces it. Close codes and reasons surface in the pill. RELAY.md §8. |

## Shipped — 2026-09-23: console links no longer flap (v0.39.1)

| id | shipped as |
|---|---|
| relay | An idle console (background tab, the peek reader's 60 s poll) tripped the node's 45 s link-silence rule because its socket ping was answered at the relay; the console now sends a sealed link-level ping every 20 s and the node ignores it. RELAY.md §8. The relay worker itself was unchanged since v0.27.3. |

## Shipped — 2026-09-22: the rest — teams told, transcripts, rename, phone boards, chat width, console view (v0.39)

| id | shipped as |
|---|---|
| K-3 | `boards::upsert_and_notify` / `delete_and_notify` on both write paths (API and peer merge): one `notice` per local agent whose board-mates changed, thread `board:<id>`. |
| S-5 | The history drawer is the name's *transcripts*: `TranscriptsPanel` over the shared `sessionRows.tsx` (rows and verbs moved out of Library.tsx); *branched from @x* in words. |
| S-6 | Adoption verbs on the row: a branch-of row's chooser offers *move @parent here* / a new name, ⋯ offers *ignore*; the bell card stays. |
| S-8 | `POST /agents/{name}/rename` (`Store::rename_agent` follows every table incl. board panes and links; refused while running); "rename this name…" in the ⋯ setup group with an inline field. |
| Q-5 | The phone board's strip: numbered pane chips with the waiting pip, one pane open; the note says layout editing is a desktop verb. |
| chat | Both parties fill the view; opposite gutters say who spoke (`--chat-gutter`). |
| console | The console render mode as a terminal: prompt column, hairline turn ends, ruled tool cards. |

## Shipped — 2026-09-22: polish — the console as a crafted studio (L, v0.38)

| id | shipped as |
|---|---|
| L | Token layer with aliases; type scale + label floor; `.btn` system, styled selects, `--surface-input`; chips by signal; `.row-item`, `.table`, `.empty`; one popover skin + motion; drift removed from every page sheet; status bar zones + presence strip + drawn bell; theme in the palette; session bar surface + busy hairline; tool cards in the bubble column; boards square, pane cluster trimmed, `pane` group in ⋯; notices inbox + settings fold; Now column + kind chips; Mesh onboarding folded; Usage segs labelled; History controls clustered; phone status bar reduced. See PROPOSALS-2026-09-L.md §6. |
| draft | One composer draft per session per console (page, any board pane); mounted views follow each other. |
| solo | `bare@repo@<hostname>` resolves as local on a node with no mesh (board panes made by hand or synced). |
| L-x (v0.38.1) | Slash autocomplete grouped by source (commands · plugins · skills); the Mesh map's agents as rail-style pills (presence dot + bare name, two per row); need-card verbs on their own line on phones; the question, answer, skip, always-allow and charter buttons on the button system's heights. |

## Shipped — 2026-09-22: start and stay; a board is a team (K-1, K-2, v0.37)

| id | shipped as |
|---|---|
| K-1 | "stay here after starting a session" on the Mesh page (per browser, per mesh profile); every start path honours it; a *started:* strip of links. |
| K-2 | `Neighborhood.boards` from the board rows; charter + `bus_status` line; in-neighborhood; bare names resolve across a board. |

## Shipped — 2026-09-21: names, transcripts and the branch verb (S-1..S-4, S-7, v0.36)

| id | shipped as |
|---|---|
| S-1 | The branch card on every entry to the verb (⎇, ⋯ row, `/branch`, `/fork`, `/branch <name>`, palette): move preselected bare, new-agent preselected with a name; `<name>-N` prefilled and selected; the button says the outcome; taken names rejected inline; `now` skips the card (`ui/src/branchCard.tsx`). |
| S-2 | `branch_agent` forks first and bookmarks after; a failed fork revives the name in place and says so; `fork_pending` on the agent → "no turn yet" in the bar; `POST /agents/{name}/branch/undo` before the first turn, offered beside the confirmation. |
| S-3 | `sessions_json` (node + federation) carries `agent`/`agents`/`state`/`label`/`bookmark_id`/`agent_live`/`branch_of`/`adoption_id`; titles cleaned of command markup; the Mesh list grouped by name with one primary verb (open / revive / resume…) and ⋯ (ignore branch, forget earlier point); two-line rows on phones. |
| S-4 | `POST /agents/{name}/move-to`; bookmark-resume without `as` folds into it; the adoption card says "move @x here"; the Mesh chooser offers "move a name here". |
| S-7 | Spawn refuses a resume onto another name's current transcript; the Mesh row never offers it; the list marks a transcript with two names. |
| v0.36.1 | A split (and move-to, bookmark-resume, undo) on a peer's agent came back with the peer's bare key, so the console opened a name this node did not have ("no agent named …", the operator's first-week error); `proxy_agent` qualifies it with the node; the rail refreshes before opening the new agent. |
| v0.36.2 | The operator's store showed three names with `fork_pending` on one transcript — branches that never took a turn because the console opened the wrong name (v0.36.1). Now the parent's Mesh row lists branches *waiting for a first turn* with links; the bar offers *undo branch* whenever a name is a pending fork (refused for a split: stop it instead). |

## Shipped — 2026-09-19: the whole console on a phone (Q-1..Q-4, v0.35)

PROPOSALS-2026-09-I.md; CONSOLE_APP.md §4. The audit: Plugins, Usage,
the palette and hover-only details were unreachable on a phone. A More
sheet in the bottom bar, a ⌘ button for the palette, a verbs group in
the session sheet, tap-to-reveal, and phone layouts for the Plugins
page. Q-5 (stacked boards) is the open design.

## Shipped — 2026-09-16: delivery that always lands, on the record (B-1..B-4, v0.34)

PROPOSALS-2026-09-H.md; DESIGN.md §4.2. Every bus message writes now,
idle or busy, as the operator's does; urgency advisory; the node records
every write and completes the transcript from it; merged lines split on
`[aspen bus end]`; a boundary guard nudges when the harness holds input
past a turn end; bus bubbles collapse to sender + first line.

## Shipped — 2026-09-15: several meshes, tier 2 (F-6..F-8, v0.33)

PROPOSALS-2026-09-F.md §8; CONSOLE_APP.md §7. Live switch (the app
remounts on the switch generation, the tunnel restarts; transcript
caches key by profile), peek (a reader per mesh not in view; the
switcher shows needs-you counts, nothing waiting, or unreachable), and
the console's name in a mesh chosen at identity creation.

## Shipped — 2026-09-15: the session menu's second wave (U-5..U-7, v0.32)

PROPOSALS-2026-09-G.md §7. Panels are real popovers (no mouse-leave
close; Esc, outside click, ×); every menu entry is a palette command
(models, modes, render, each panel); `/plugins`, `/artifacts`,
`/activity`, `/recap` in the composer. Also v0.31.1–.5: menu close,
panels beside the menu, rail activity pip, recap on demand, recap for
remote sessions.

## Shipped — 2026-09-15: the top of a session — one row, one menu (U-1..U-4, v0.31)

PROPOSALS-2026-09-G.md; SESSION_BAR.md. One session bar shared by the
page and a board pane (`ui/src/sessionBar.tsx`): presence as one glyph,
name, harness chip; the context meter and the model in use as readouts;
interrupt, branch and stop as icons; one ⋯ menu — setup, inspect, move —
with badges for what it folds away, Esc/outside-click/arrow keys, a
bottom sheet on phones. Every menu row is a palette command from the
same registry (`sessionCommands.ts`). The board's pane bar is gone for
session panes; the pane's number and layout buttons ride the bar.

## Shipped — 2026-09-15: the drain gate defends work, not idleness (v0.30)

SERVICING.md §4, §7. The quiet gate waited for every session to have
been idle five minutes and nothing spawned in that time — on a busy node
some session had always just finished, so the operator always ended up
at `aspen update --restart`. Now it waits only on a turn in flight, an
open prompt, a shell task whose process still runs, or a running
subagent. And *update fleet* no longer marks this node done at the
request: it shows as current, draining, until the updater launches.

## Shipped — 2026-09-15: one console, several meshes (F-1..F-5, v0.29)

PROPOSALS-2026-09-F.md; CONSOLE_APP.md §7. Profiles (one mesh as the
hosted console sees it; every per-mesh key scoped, transcripts too; a
one-time migration), the mesh label and switcher in the status bar, the
Meshes page (a card per mesh, a fresh identity per mesh, direct nodes
filed by the mesh they report), push in every mesh with a console-held
sender key and `mesh=` links that switch first.

## Shipped — 2026-09-14: the plugins menu (G-1..G-6, v0.28)

Design: [PROPOSALS-2026-09-E.md](PROPOSALS-2026-09-E.md); as built:
PLUGINS.md §5–6. Plugins on the roster, the harness's own plugins shown,
per-session toggles and versions in the menu, update from the menu,
sync before spawn, content-hashed cache keys. Backlog from it: adopting
the harness's own plugins into the library; per-plugin usage; hot
reload where the harness can.

## Shipped — 2026-09-10: Web Push (D-4, v0.27)

NOTIFICATIONS.md §6: VAPID keys on the node, subscriptions per browser
with per-device kinds (needs-you by default), delivery from the node to
the browser's push service, the worker showing the notice and setting the
badge, a test button. Not built: relaying a push through a peer for a
node without outbound HTTPS.

## Shipped — 2026-09-09: the console as an app (D-1, D-2, D-3, D-10, v0.26)

Design: [PROPOSALS-2026-09-D.md](PROPOSALS-2026-09-D.md); as built:
[CONSOLE_APP.md](CONSOLE_APP.md). Installable with a dock badge and
shortcuts (D-1); a service worker for the shell and the console's own
update (D-2); the console hosted on GitHub Pages with connections to a
local node or a relay, node CORS for listed origins (D-3); a phone layout
(D-10). D-4..D-9 and D-11 above are what it left open.

## Shipped — 2026-09-09: catch me up, search, auto-start (T-1, T-2, T-3, v0.25)

Design: [PROPOSALS-2026-09-C.md](PROPOSALS-2026-09-C.md). The
"since you last looked" digest and the harness recap
(CATCH_UP_AND_SEARCH.md §1–2, T-1); text search across every session the
mesh holds with a Search page and palette fall-through (§3, T-2); user-
level auto-start on Linux/WSL, macOS and Windows with supervisor-aware
down/restart/update (AUTOSTART.md, T-3). The live-elsewhere note reads as
written and is dismissable.

## Shipped — 2026-09-08: MCP surface, session processes, mesh-wide runtime defaults (M-1, M-2, M-3, v0.24)

Design: [PROPOSALS-MCP.md](PROPOSALS-MCP.md). One round: the session's
MCP servers with status, error, tools and the harness's controls
(reconnect, enable/disable, authenticate, add), `/mcp` opening it, a
refresh that asks the harness now, notices and a fleet chip (M-1); the
status line's activity count, the details view (status, runtime,
script, output) with a stop that terminates the process, and the
session's child processes listed with a stop (M-2); harness defaults as
a synced table with per-node overrides and an effective-value panel on
the Mesh list view (M-3).

## Shipped — 2026-09-07: Codex as a second harness (H-1, v0.20–v0.23)

Design first ([PROPOSALS-HARNESSES.md](PROPOSALS-HARNESSES.md)), then
four rounds, each verified on the rig with the Claude flows re-run:
v0.20 the seam (HARNESSES.md §1–5), v0.21 the Codex adapter
(`aspen-codex`, `aspen mcp`; HARNESSES.md §6, CODEX_RUNTIME_REFERENCE.md),
v0.22 the console and the gates (HARNESSES.md §7), v0.23 the mesh
features (HARNESSES.md §8, MIGRATION.md "Harnesses"). H-2..H-9 above are
what it left open.

## Shipped — 2026-09-07 slate (S-1..S-12, v0.15–v0.19)

Design: [PROPOSALS-2026-09-B.md](PROPOSALS-2026-09-B.md).

| id | tag | ask | round |
|---|---|---|---|
| S-1 | protocol + sessions | Transcript replication, opt-in (REPLICATION.md). | v0.16 |
| S-2 | console | Fleet-wide activity in Now. | v0.15 |
| S-3 | console + protocol | Notifications: toasts, bell, webhook and command hook (NOTIFICATIONS.md). | v0.15 |
| S-4 | console + sessions | Cost and usage roll-up (USAGE.md). | v0.15 |
| S-5 | protocol + sessions | Memory convergence with a stored base and 3-way merge (MEMORY.md). | v0.16 |
| S-6 | protocol | Hybrid logical clock for mesh-wide rows (SYNC.md §3). | v0.16 |
| S-7 | sessions + console | Session templates (PLUGINS.md §templates). | v0.17 |
| S-8 | servicing + console | Evacuate a node, bring it here, preflight readout (MIGRATION.md). | v0.17 |
| S-9 | protocol | Multi-mesh membership for one node (MESHES.md). Closes P-1. | v0.18 |
| S-10 | protocol + console | Console through the relay (RELAY.md §11). Closes P-4 and P-5 (`hint: wsl-nat`). | v0.18 |
| S-11 | console + protocol | Pair mode on boards (BOARDS.md §8). | v0.19 |
| S-12 | console | Task notifications collapsed to summary cards. | v0.15 |

S-1, S-5 and S-8 together close B-5b (migration phases 3–4). H-10
(direct-link keepalive, from the 2026-09-08 hang audit) shipped in
v0.23.3 as link liveness for every link kind (RELAY.md §8.1).

## Shipped — 2026-09-06/07: the console round (B-1..B-8, v0.10–v0.14)

Design: [PROPOSALS-2026-09.md](PROPOSALS-2026-09.md).

| id | tag | ask | round |
|---|---|---|---|
| B-1 | console | Composer drafts persist. | v0.10 |
| B-2 | console | Tool calls live while running, summary after, click to expand. | v0.10 |
| B-3 | console + protocol | Open what the agent points at, from any node (the viewer). | v0.10 |
| B-4 | console + sessions | Paste attachments inline. | v0.10 |
| B-5 | protocol + sessions | Session migration over the mesh, phases 1–2 (MIGRATION.md). | v0.11 |
| B-6 | console | Boards (BOARDS.md). | v0.12 |
| B-7 | protocol + sessions + console | Plugins: a mesh-managed library, activated by scope (PLUGINS.md). | v0.13 |
| B-8 | sessions + console | Activity: tasks, monitors, subagents, workflows (ACTIVITY.md). | v0.14 |
