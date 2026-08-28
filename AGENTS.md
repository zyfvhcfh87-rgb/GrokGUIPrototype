# Grok Build GUI

## Project intent

Build a local-first desktop GUI for the installed Grok Build coding agent. The product should be a clear, safe, polished agent cockpit rather than a terminal emulator or a replacement code editor.

Optimize for Windows first while preserving a cross-platform architecture. Grok Build remains the agent runtime and source of truth for authentication, tools, sessions, plans, extensions, and model access. The GUI owns presentation, interaction, and its own small set of preferences.

## Approved decisions

- **Tauri 2 is approved by the user** as the desktop framework (decision recorded 2026-08-29).
- Use React, TypeScript, and Vite for the interface.
- Use Rust and Tokio for the native application core.
- Integrate through Agent Client Protocol v1 over `grok agent stdio` using JSON-RPC on stdin/stdout.
- Prefer the official Rust `agent-client-protocol` SDK when it is compatible with the installed Grok version. Isolate any Grok-specific extensions behind one adapter.
- Render models, reasoning effort, modes, commands, and optional features from negotiated capabilities instead of hard-coding them.
- Target one excellent project/session workflow before adding dashboard and extension-management features.

These are settled project choices. Revisit them only when current source or a working compatibility spike proves a blocker; record the evidence before proposing a change.

## Current evidence

The following was observed locally on 2026-08-28 and must be rechecked when implementation begins:

- `grok.exe` was installed at `%USERPROFILE%\.grok\bin\grok.exe`.
- The installed version was Grok Build `1.0.5` stable.
- A live ACP `initialize` probe reported protocol v1, cached-token/API-key/Grok authentication, dynamic model metadata, and session list/resume/close capabilities.
- The directory was empty and was not a Git repository.

Useful current sources:

- Grok Build overview: <https://docs.x.ai/build/overview>
- Headless and ACP integration: <https://docs.x.ai/build/cli/headless-scripting>
- Grok Build source: <https://github.com/xai-org/grok-build>
- ACP specification and updates: <https://agentclientprotocol.com/>
- Tauri capabilities: <https://v2.tauri.app/security/capabilities/>

Grok Build and ACP change quickly. Verify the installed binary, official documentation, and negotiated runtime capabilities rather than treating version observations in this file as current facts.

## Product shape

The primary window should have three regions:

```text
Projects and sessions | Conversation and composer | Plan, activity, and changes
```

The center is the main experience. Side panels must be collapsible and support focus rather than permanently reducing the conversation to a narrow column.

The interface should make these states immediately legible:

- connecting, authenticating, ready, working, waiting for input, failed, and disconnected;
- normal, plan, auto, and always-approve modes when the runtime exposes them;
- running and completed tools, terminal commands, background tasks, and subagents;
- pending permission or elicitation requests;
- streaming assistant messages and collapsed reasoning;
- saved plan state and workspace changes.

## Architecture

```text
React interface
    <-> typed Tauri commands and events
Rust application core
    <-> GrokRuntime interface
ACP v1 JSON-RPC transport
    <-> grok agent stdio
```

### `GrokRuntime` module

Keep protocol and child-process complexity behind one deep module. Its interface should cover:

- locating and validating the Grok executable;
- starting, monitoring, restarting, and stopping the ACP child process;
- initialization, authentication, and capability negotiation;
- session list, new, load/resume, prompt, cancel, close, and supported metadata changes;
- config-option discovery and updates;
- permission and elicitation responses;
- conversion of ACP and `x.ai/*` messages into normalized application events;
- diagnostics that are safe to display or persist.

React must not know JSON-RPC method names, request IDs, raw transport framing, credential locations, or child-process details.

Normalize runtime output into domain events such as:

- `RuntimeStateChanged`
- `SessionStateChanged`
- `MessageChunkReceived`
- `ThoughtChunkReceived`
- `ToolCallChanged`
- `PermissionRequested`
- `ElicitationRequested`
- `PlanChanged`
- `UsageChanged`
- `RuntimeFailed`

Use recorded, sanitized ACP fixtures for deterministic tests. A fake ACP process is the test adapter at the `GrokRuntime` seam.

### State ownership

- Grok owns conversation persistence and session identifiers.
- Grok owns credentials. Never parse, copy, log, or expose `~/.grok/auth.json`.
- Grok owns tools, MCP servers, skills, plugins, hooks, memory, and model execution.
- The GUI may store recent workspaces, window layout, theme, and presentation preferences.
- Prefer ACP lifecycle methods over reading or mutating Grok session files directly.
- Workspace Git status and diffs are read-only views of the current workspace, not automatically attributable to one agent session.

## MVP

The first useful release includes:

1. Detect and validate the Grok executable and show actionable setup diagnostics.
2. Authenticate through Grok's advertised authentication methods without handling credential contents.
3. Pick a workspace and remember recent workspaces.
4. Create, list, load/resume, close, and display sessions.
5. Send prompts and stream Markdown responses.
6. Render reasoning, tools, terminal output, and errors as structured cards.
7. Cancel or interrupt an active turn when supported.
8. Render model, reasoning, and mode controls from negotiated configuration.
9. Handle permission and elicitation requests with exact scope and consequence.
10. Display and approve or revise plans when supported.
11. Display current workspace changes and diffs without editing them.
12. Recover cleanly after an app restart or Grok process failure.

Accessibility, keyboard operation, empty/loading/error states, and safe failure behavior are MVP requirements rather than later polish.

## Deferred scope

Defer these until the MVP is complete and verified:

- a general-purpose terminal emulator;
- an integrated source editor;
- automatic Grok installation, updating, or binary bundling;
- plugin, MCP, skill, hook, or marketplace management;
- worktree and branch creation;
- a multi-agent dashboard;
- memory management;
- remote, web, or mobile access;
- account-wide quota reporting without a stable machine-readable Grok interface.

Session context or cost may be displayed when supplied through ACP usage updates. Do not infer account quotas from scraped TUI text or private files.

## Build sequence

### Phase 0: compatibility spike

Build a minimal Rust harness before scaffolding the complete interface. Exercise this real lifecycle against the installed Grok binary:

```text
spawn -> initialize -> authenticate -> session/new
      -> prompt -> receive streamed updates -> cancel
      -> list/resume -> close -> terminate cleanly
```

Explicitly test permission requests, elicitation, plan updates, tool events, config options, malformed messages, stderr output, process crashes, and restart/reconnect behavior.

Phase 0 is complete only when sanitized fixtures and an evidence table identify which expected behaviors are confirmed, unsupported, version-specific, or still unknown. Do not design critical safety interactions from documentation alone.

### Phase 1: vertical slice

Deliver one end-to-end path:

```text
launch -> diagnose/authenticate -> select workspace -> new session
       -> send prompt -> stream response/tool event -> restart app -> resume session
```

Phase 1 is complete when the path works from a packaged Tauri development build and has automated coverage through the `GrokRuntime` interface.

### Phase 2: safety workflow

Add permission and elicitation surfaces, plan review, cancellation, diff inspection, process diagnostics, and recovery. Verify destructive-looking commands and denied requests with explicit contract tests.

### Phase 3: product polish

Add session navigation, keyboard shortcuts, accessible themes, onboarding, compatibility reporting, packaging, signing, and platform-specific QA.

### Phase 4: power features

Consider subagents, worktrees, task dashboards, extension management, command palettes, memory, and richer usage information based on actual user needs and current protocol support.

## Safety requirements

- Spawn the resolved Grok executable directly with an argument array; never construct a shell command string.
- Canonicalize and validate every selected workspace.
- Keep filesystem, process, and credential access in Rust behind narrowly scoped Tauri commands.
- Ship a restrictive Tauri capability set and Content Security Policy.
- Load packaged local interface code; treat remote content and links as untrusted.
- Sanitize rendered Markdown and validate external URLs before opening them.
- Show the exact command, working directory, affected path, and persistence choice for permission requests.
- Make deny the safe default when a request is malformed, orphaned, expired, or unsupported.
- Keep stdout reserved for ACP framing and treat stderr as diagnostic data that may contain sensitive paths.
- Shut down child processes gracefully and prevent orphaned Grok runtimes.

## Verification expectations

- Unit-test transport framing, request correlation, state reduction, redaction, and path validation.
- Contract-test against a fake ACP process using recorded fixtures.
- Keep an opt-in compatibility suite for the locally installed Grok binary.
- Component-test every streamed event and approval state.
- Exercise the packaged desktop application for the critical launch, prompt, permission, cancel, crash, and resume flows.
- Record the Grok version and negotiated capability snapshot with compatibility test results, excluding credentials and private workspace contents.

## Working agreement for future agents

- Read this file before planning or implementing project work.
- Recheck repository state and preserve any dirty or untracked user work.
- Recheck current Grok and ACP behavior when it affects a decision.
- Start implementation with Phase 0 unless the user explicitly changes priority.
- Keep the `GrokRuntime` seam small and hide protocol complexity inside it.
- Keep scope aligned with the MVP and surface proposed expansions before implementing them.
- Do not commit, push, publish, install, sign, or release unless the user explicitly requests that action.
