# Aspen

**A control plane for a fleet of coding agents across all your repos and all
your machines.** One node daemon per machine that runs agent sessions over
their native headless protocols; one bus that lets agents (and you) talk
across repos and machines; one minimal cloud rendezvous; one operator
console.

- Product design: [`docs/DESIGN.md`](docs/DESIGN.md)
- Protocol ground truth: [`docs/reference/CLAUDE_RUNTIME_REFERENCE.md`](docs/reference/CLAUDE_RUNTIME_REFERENCE.md)
  (field-verified) and [`docs/reference/HOST_PROTOCOL.md`](docs/reference/HOST_PROTOCOL.md)
  (source-level receipts)

## Layout

| Crate | What |
|---|---|
| `crates/aspen-core` | Domain vocabulary: ids, bus semantics, normalized session events, the adapter seam |
| `crates/aspen-claude` | The Claude Code adapter: process host, NDJSON protocol client, in-process MCP server |
| `crates/aspen` | The `aspen` binary: node daemon + CLI (P0: dev harness commands) |

## Dev harness

```bash
cargo build
# one prompt, streamed, then clean shutdown:
target/debug/aspen dev oneshot --repo /path/to/repo --prompt "hello"
# interactive REPL (/quit, /int):
target/debug/aspen dev chat --repo /path/to/repo
```

Status: P0 walking skeleton — spawn/handshake/stream/permissions/in-process
MCP verified live against claude 2.1.251.
