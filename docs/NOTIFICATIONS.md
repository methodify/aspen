# Notifications: notices, the bell, the outbound hook

**Status:** reference for what is built (2026-09-07, v0.15). Proposal:
PROPOSALS-2026-09-B.md §3. Code: `crates/aspen-node/src/notify.rs`
(raising, the hook), `store.rs` (`notices` table), the `notices` op in
`federation.rs`, `GET /api/notices` in `api.rs`, `ui/src/notices.tsx`
(provider, toasts, bell).

## 1. What a notice is

A moment the operator may want told about, raised by the node that
owns the session:

| kind | raised when | body |
|---|---|---|
| `turn_ended` | the session's turn ends | the reply's first 200 chars |
| `question` | an `AskUserQuestion` prompt opens | the first question |
| `permission` | any other permission prompt opens | the command or path |
| `activity_settled` | something that was running at the previous turn end is not running now (ACTIVITY.md ledger diff) | the activity's label |
| `exited` | the process died with a non-zero code or a signal, outside a daemon shutdown | the code |
| `inbox` | an agent sends to `@operator` on the bus | the message's first 200 chars |

Each is `Notice {id, ts, node, agent, kind, title, body, link}`; `link`
is the console route to act on it. Notices live in the `notices` table
for seven days. A notice is not a need: *Needs you* still lists what is
waiting on a decision; the bell lists what happened.

## 2. The console

`GET /api/notices?since=node=id,node=id` returns this node's notices
after its cursor and, by default, every up peer's after theirs (a
`notices` op each, list timeout), sorted by time, with the new cursors.
A node absent from `since` answers with its head and no rows, so a fresh
browser learns where "now" is instead of replaying a week. The console
polls every 3 s and keeps the cursors in `localStorage`.

What it does with them, all per browser (`aspen.notices.prefs`):
- **Toasts** bottom right, dismissible, click opens the link, aged out
  after twelve seconds.
- **Tab title** carries the unseen count until the bell is opened.
- **Browser notifications** while the tab is hidden, when turned on
  (asks the browser's permission; the toggle reads "blocked" if denied).
- **The bell** (status bar): unseen count, the recent list, toggles per
  kind and per node, and the outbound hook form.

Elapsed times use the server clock (`Date` header on every response) so
a browser whose clock drifts from the node still shows honest ages.

## 3. The outbound hook

Node settings (`settings.json`, editable in the bell):

```json
"notify": { "webhook": "https://…", "command": "curl -d @- ntfy.sh/me", "kinds": ["question","permission","exited"] }
```

The raising node fires both that are set, at most once per notice, with
a five-second timeout: the webhook receives the notice as JSON by POST;
the command runs under `sh -c` (`cmd /C` on Windows) with the JSON on
stdin. `kinds` defaults to question, permission and exited; `"*"` fires
everything. Failures are logged, never retried.

## 4. Verified (rig, 2026-09-07)

A session asked for a background task raised `turn_ended`, then
`turn_ended` and `activity_settled` when the task finished; the bell
counted and listed them from a peer node; a command hook on the node
appended each notice's JSON to a file.

## 5. Not built

Digest mode (one summary per quiet period); per-session mute; a
`needs` notice when a bus message goes unanswered; sound.

## 6. Web Push (v0.27, PROPOSALS-2026-09-D.md §4)

Notices as OS notifications with the console closed — a phone in a
pocket. The node holds a VAPID key pair (`push_vapid.json` in the data
dir, made on first use); a console that turns on **push to this device**
in the bell's panel asks the browser's permission, subscribes with the
node's public key, and registers the subscription on the node it talks
to (`POST /api/push/subscribe`, through the tunnel when attached). When
a notice of a subscribed kind is raised, `push.rs` encrypts it (RFC 8291,
`aes128gcm`, the `web-push` crate's builders) and POSTs it to the
browser's push service with a VAPID JWT — Google, Mozilla or Apple,
plain HTTPS, no other party. The service worker (`ui/src/sw.ts`) shows
the notification, sets the dock badge, and a click focuses or opens the
console at the notice's link.

Defaults: off; when on, `question` and `permission` (the needs-you
kinds), the rest opt-in per device. One subscription per endpoint,
re-registered when the kinds change. A subscription the push service
reports gone (404/410) is forgotten; other failures are kept on the row
(`last_error`) and shown by `GET /api/push/subscriptions`. **test** sends
one now. Pushes come from the node subscribed on: subscribe on each node
whose notices you want, or on the one you attach to through a relay.
The node needs outbound HTTPS; relaying a push through a peer that has it
is not built.
