# Proposal Q — History that scales: compactions, pages that follow them, and reading from the end

**Status:** accepted 2026-10-04 (the operator asked for the write-up as a
design reference and for the build to follow without a further review).
**Shipped as v0.49.2.** Measured on a 146 MB Claude transcript: the
newest page in 0.46 s (the whole file took 2.9 s), each earlier page
0.5–0.7 s, a delta 0.32 s; every page begins at a compaction.
Follows v0.49.1, which paged history by size so a long session's history
could cross the relay.

---

## 1. Where history stands after v0.49.1

Every history request (open, refresh, *reload transcript*, and the
console's "what is new since my copy" delta) has the node **read and
rehydrate the session's whole transcript file**, then slice it. The
console receives the newest ~8 MB (a page, cut at a turn's start) and can
load earlier pages.

What that leaves:

- **The node's cost grows with the session.** @impl@plank@lt-bryon-wsl
  took 20 s to answer over the VPN (79.6 MB of items). This session's
  Claude file is 146 MB, with 37,761 lines. The cost is paid on every
  open and every refresh, including deltas that return a handful of
  items.
- **Compactions are invisible.** The rehydrator drops the summary line
  (`isCompactSummary`) and ignores the `compact_boundary` system line. The
  history reads as one unbroken conversation, although the model's own
  context was replaced several times (five times in this session, each
  reducing ~967k tokens to ~12.5k).
- **Pages are cut by size, not by meaning.** The natural unit of "what
  the session is working with now" is everything since the last
  compaction.

## 2. The design

### Q-1 Compactions in the history

A compaction becomes a history item:

```json
{ "role": "compaction", "uuid": "<boundary line uuid>", "timestamp": "…",
  "trigger": "auto" | "manual", "pre_tokens": 967297, "post_tokens": 12535,
  "summary": "<the summary text, capped at 16 KB>" }
```

- **Claude:** from the `type: system, subtype: compact_boundary` line
  (`compactMetadata.trigger/preTokens/postTokens`). The `isCompactSummary`
  user line that follows is attached to it as `summary`; it is still
  never shown as something the operator typed.
- **Codex:** from a rollout `compacted` item (`payload.message` is the
  summary) or an `event_msg` of type `context_compacted`.
- **Console:** a divider across the transcript in both renderers (chat
  and console): "context compacted · auto · 967k → 12.5k tokens", with
  the summary behind a disclosure.

### Q-2 Pages that follow compactions

A page is **everything since the last compaction**: the compaction item
first, then what follows. *Load earlier history* steps back **one
compaction at a time**. The size budget stays as a safety net: a stretch
larger than the budget (no compaction for a long while) is cut by size,
at a turn's start, as in v0.49.1. The opposite case is handled too. A
stretch smaller than a floor (256 KB, e.g. right after a compaction) also
takes the stretch before it, so a fresh open never shows only a summary.

The cursor stays opaque to the console (`earlier`, echoed back as
`before`), so the console needs no change to page differently.

### Q-3 Reading from the end

For Claude sessions (the large ones), the node stops reading the whole
file:

- **A page** is found by reading the file **backwards** in blocks from
  the end (or from the cursor), until it passes a `compact_boundary` line
  or a raw-byte window (8× the item budget; raw JSONL runs ~4–5× its
  rehydrated size) runs out. Only that slice is rehydrated. A tool result
  sits after its call, so every call in the slice finds its result in the
  slice. The cursor for the page before is the slice's starting byte
  offset (`o:<offset>`).
- **A delta** (`after=<uuid>`) searches backwards for the line carrying
  that uuid and rehydrates from that line on. If the line is not found
  within the window, the answer is a fresh page (`after_found: false`), as
  today.
- **The input record** (inputs the transcript lacks, placed by time) is
  merged into whatever slice is returned, limited to its time span.
- **Codex** and replicas keep the whole-file read (rollouts are smaller,
  and a rollout's state builds from its start), with the Q-2 paging
  applied to the items.

## 3. Order and size

| Item | What | Size |
|---|---|---|
| Q-1 | Compaction items (Claude, Codex); divider in both renderers | S–M |
| Q-2 | Compaction-aware pages (floor and budget), node and op | S |
| Q-3 | Backwards tail read for Claude: pages, deltas, record merge | M |

All three ship together. Backlog I-1 (images by reference) and I-2 (tool
text on demand) stay separate: they shrink pages; this makes finding them
cheap and meaningful.
