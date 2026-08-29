# Grok Build GUI

A local-first desktop cockpit for Grok Build. The product architecture and safety boundaries live in [AGENTS.md](AGENTS.md).

## Current status

The Phase 0 compatibility spike is substantially complete. This workspace intentionally starts with a Rust harness before the Tauri and React application shell. The harness verifies the installed Grok binary against ACP v1, records sanitized fixtures, and establishes reusable process, wire-summary, and normalization components inside `grok-runtime`. Lifecycle orchestration remains in the probe crate; Phase 1 will assemble those components behind the long-lived `GrokRuntime` interface and expose typed Tauri commands and events. Phase 0 closes by classifying each expected behavior as installed-runtime evidence, deterministic fake coverage, version-specific behavior, unsupported behavior, or an explicit unknown. Exhaustive safety and lifecycle permutations belong to Phase 2. The installed-runtime unknowns retained after the spike are tracked in [the Phase 0 evidence report](docs/compatibility/phase-0-evidence.md#installed-runtime-unknowns-retained-after-phase-0).

Nothing in the compatibility suite reads Grok credential files. Live authentication and session probes are explicit opt-in commands.

## Phase 0 commands

```powershell
# Read-only executable/version detection
cargo run -p grok-acp-probe -- detect

# Initialize against an empty temporary GROK_HOME; no authentication or session
cargo run -p grok-acp-probe -- initialize

# Prove the Windows Job Object launch boundary without authentication or a session
cargo run -p grok-acp-probe -- managed-initialize

# Opt-in live lifecycle using Grok-owned ambient authentication
cargo run -p grok-acp-probe -- lifecycle --workspace . --exercise-cancel

# Opt-in, zero-turn control probe on a disposable session
cargo run -p grok-acp-probe -- controls --workspace .

# Opt-in, zero-turn crash/restart/resume probe through the managed Windows boundary
cargo run -p grok-acp-probe -- managed-restart --workspace .

# Deterministic contract suite; no model or Grok credentials involved
cargo test --workspace
```

The live lifecycle creates and closes a persisted Grok session and sends model prompts. The control probe creates and closes a separate persisted session with the already advertised model/reasoning values, repeats those values as a no-op, and restores session mode from `plan` to `default`; it sends no prompt and does not touch global permission policy. The managed restart probe creates one persisted zero-turn session, deliberately drops the first Job Object-contained process, then discovers, resumes, and closes that exact session from a fresh contained process. These probes never delete their closed sessions afterward. `managed-initialize` uses an isolated home and creates no session.

On Windows, the implementation routes `initialize`, `lifecycle`, and `controls` through the same Job Object-contained process adapter rather than the SDK's direct-child launcher. The dedicated managed initialize and restart commands provide installed-runtime containment evidence, and final ordinary initialize and zero-turn control runs also completed through that shared adapter with no malformed or dropped frames, stderr output, or remaining Grok processes. The authenticated prompt lifecycle transcript was not rerun solely to re-prove launcher selection after this routing change, so its containment claim rests on the shared adapter path plus the independent process-tree tests. The separate managed commands remain explicit containment and restart diagnostics.

## Workspace layout

- `crates/grok-runtime`: reusable runtime boundary, diagnostics, and normalized events.
- `tools/grok-acp-probe`: opt-in compatibility and evidence CLI.
- `tools/fake-acp-agent`: deterministic child-process test adapter.
- `fixtures/acp`: sanitized ACP transcripts used by contract tests.
- `docs/research`: primary-source compatibility notes.
