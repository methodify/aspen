# Usage: tokens and cost, per session, repo, node, model

**Status:** reference for what is built (2026-09-07, v0.15). Proposal:
PROPOSALS-2026-09-B.md §4. Code: `crates/aspen-claude/src/usage.rs`,
`node::usage_rows`, `store::observed_cost`, the `usage` op, `GET
/api/usage` and `GET /api/agents/{name}/usage`, `ui/src/pages/Usage.tsx`,
the status-bar popover in `Session.tsx`.

## 1. Where the numbers come from

Two sources, kept apart on purpose:

- **Tokens, from the transcript.** Every assistant line carries
  `message.usage` (input, output, cache read, cache creation) and
  `message.model`. `usage::session_usage` folds them per model over the
  main transcript and every `subagents/*.jsonl`, counts real operator
  prompts as turns, and caches by the transcript's (size, mtime).
  Rehydrated assistant items carry `usage` and `model` too.
- **Money, from the harness.** Aspen does not keep a price table. The
  harness writes a `cost-state` line to the transcript with
  `totalCostUSD` and per-model `costUSD`; that is the session's
  *lifetime* figure. Separately, every turn end Aspen observes is a
  `turn` fleet event with `cost_delta`, so the store can answer "what was
  spent in this window" (`observed_cost`) across restarts, for sessions
  the harness has not priced on disk (headless sessions today).

## 2. Surfaces

- **Usage page** (`/usage`, `u`): one row per session across the estate
  (this node plus every up peer, `usage` op), grouped by session, node,
  repo or model (`1`–`4`), over today / this week / all (`t`). Columns:
  turns, calls, in, cache read, cache write, out, models (subagent count
  folded in), lifetime (harness-priced), and the window's observed spend.
- **Session status bar**: the cost figure opens a popover with tokens
  by model and the subagent share; when the harness has not priced the
  session it shows the observed spend instead and says so.
- `<synthetic>` model rows with no output are hidden.

## 3. Verified (rig, 2026-09-07)

Six sessions on three nodes listed with tokens from their transcripts
(one with three subagents folded in) and today's observed spend; the
popover on a live session showed its model split.

## 4. Not built

An estimate from a configurable price table for models the harness has
not priced; per-turn tokens on the turn-end marker; budgets and alerts.
