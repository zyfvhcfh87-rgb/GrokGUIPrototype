# Grok Build GUI

A local-first desktop cockpit for Grok Build. The product architecture and safety boundaries live in [AGENTS.md](AGENTS.md).

## Current status

The Phase 0 compatibility spike is complete, and Phase 1 now has a Tauri 2, React, TypeScript, and Vite application shell plus the long-lived `GrokRuntime` service. The harness verifies the installed Grok binary against ACP v1, records sanitized fixtures, and establishes reusable process, wire-summary, normalization, and lifecycle components inside `grok-runtime`. The desktop launch flow diagnoses Grok discovery and compatibility, starts authentication only through advertised methods, selects canonical workspaces through a native picker, and keeps a bounded removable list of recent workspaces. After a workspace is selected, the left panel lists that folder's Grok sessions and can create, resume, load, or close them through `GrokRuntime` without reading session files. Changing workspace clears the selected session so another folder cannot reuse it. App restart restores the first available recent workspace and resumes its last selected session through ACP. If the Grok child exits, the cockpit shows a recoverable diagnostic and Recover restarts the runtime without attaching stale events to the restored session. The details panel also shows a read-only Git view of the selected workspace and bounded runtime health. Workspace diffs are never treated as belonging to the current session, and Recover cannot start a second Grok child. The shell exposes only narrow application commands and a bounded, sequenced event stream; React has a typed bridge and deterministic reducer/store for streamed, duplicate, out-of-order, stale-session, and restart events. After a session is selected, the center pane streams prompts, Markdown, collapsed reasoning, and in-place tool cards, while advertised model, reasoning, mode, command, and config controls appear only when the runtime supplies them. It has no broad native-access plugins. See [the application contract](docs/architecture/application-contract.md) and [the Tauri shell security note](docs/security/tauri-shell-boundary.md).

`GrokRuntime` now owns executable discovery, the contained ACP child, initialization and authentication, negotiated capabilities, session commands, interactions, cancellation, recovery, and normalized event delivery. The Tauri process keeps one service instance for the lifetime of the application. Protocol framing and IDs, credentials, executable arguments, and child-process details remain private to the runtime crate; see [the service architecture note](docs/architecture/grok-runtime-service.md). The installed-runtime unknowns retained after the spike are tracked in [the Phase 0 evidence report](docs/compatibility/phase-0-evidence.md#installed-runtime-unknowns-retained-after-phase-0). Restart, resume, and packaged vertical-slice evidence live in [the Phase 1 evidence report](docs/compatibility/phase-1-evidence.md).

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

## Desktop shell commands

Install the project-local frontend and Tauri CLI dependencies once, then run the checks or launch the Windows development application:

```powershell
npm install
npm test
npm run build
npm run tauri dev
```

`npm run tauri build -- --no-bundle` verifies the production desktop build without producing an installer. Keep the existing Rust workspace green with `cargo check --workspace --all-targets` and `cargo test --workspace --all-targets`.

The live lifecycle creates and closes a persisted Grok session and sends model prompts. The control probe creates and closes a separate persisted session with the already advertised model/reasoning values, repeats those values as a no-op, and restores session mode from `plan` to `default`; it sends no prompt and does not touch global permission policy. The managed restart probe creates one persisted zero-turn session, deliberately drops the first Job Object-contained process, then discovers, resumes, and closes that exact session from a fresh contained process. These probes never delete their closed sessions afterward. `managed-initialize` uses an isolated home and creates no session.

On Windows, the implementation routes `initialize`, `lifecycle`, and `controls` through the same Job Object-contained process adapter rather than the SDK's direct-child launcher. The dedicated managed initialize and restart commands provide installed-runtime containment evidence, and final ordinary initialize and zero-turn control runs also completed through that shared adapter with no malformed or dropped frames, stderr output, or remaining Grok processes. The authenticated prompt lifecycle transcript was not rerun solely to re-prove launcher selection after this routing change, so its containment claim rests on the shared adapter path plus the independent process-tree tests. The separate managed commands remain explicit containment and restart diagnostics.

## Workspace layout

- `crates/grok-runtime`: long-lived runtime service, process boundary, diagnostics, and normalized events.
- `src`: strict React and TypeScript interface loaded from packaged local assets.
- `src-tauri`: minimal Tauri 2 desktop crate, restrictive CSP, reviewed application DTO/command bridge, and bounded workspace preferences.
- `src/application`: shared TypeScript contract, narrow transport facade, and deterministic event reducer/store.
- `tests`: executable security-boundary checks for the desktop scaffold.
- `tools/grok-acp-probe`: opt-in compatibility and evidence CLI.
- `tools/fake-acp-agent`: deterministic child-process test adapter.
- `fixtures/acp`: sanitized ACP transcripts used by contract tests.
- `docs/research`: primary-source compatibility notes.
