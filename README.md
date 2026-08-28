# Grok Build GUI

A local-first desktop cockpit for Grok Build. The product architecture and safety boundaries live in [AGENTS.md](AGENTS.md).

## Current status

Phase 0 is in progress. This workspace intentionally starts with a Rust compatibility harness before the Tauri and React application shell. The harness verifies the installed Grok binary against ACP v1, records sanitized fixtures, and keeps protocol/process details behind the future `GrokRuntime` seam.

Nothing in the compatibility suite reads Grok credential files. Live authentication and session probes are explicit opt-in commands.

## Phase 0 commands

```powershell
# Read-only executable/version detection
cargo run -p grok-acp-probe -- detect

# Initialize against an empty temporary GROK_HOME; no authentication or session
cargo run -p grok-acp-probe -- initialize

# Opt-in live lifecycle using Grok-owned ambient authentication
cargo run -p grok-acp-probe -- lifecycle --workspace . --exercise-cancel

# Deterministic contract suite; no model or Grok credentials involved
cargo test --workspace
```

The live lifecycle creates and closes a persisted Grok session and sends model prompts. It does not delete the session afterward.

## Workspace layout

- `crates/grok-runtime`: reusable runtime boundary, diagnostics, and normalized events.
- `tools/grok-acp-probe`: opt-in compatibility and evidence CLI.
- `tools/fake-acp-agent`: deterministic child-process test adapter.
- `fixtures/acp`: sanitized ACP transcripts used by contract tests.
- `docs/research`: primary-source compatibility notes.
