# Application contract

Issue #8 defines the reviewed interface between the native application core and React. Issue #9 adds the setup and workspace workflow without widening that interface into general native access. The Tauri layer translates narrow user intents into `GrokRuntime` commands and projects runtime output into bounded application DTOs. React never receives the generic runtime command enum or any ACP, JSON-RPC, process, credential, or stderr representation.

## Commands

The desktop exposes only these command groups:

- setup, workspace, and links: `setup_status`, `workspace_pick`, `workspace_validate`, `workspace_recent_list`, `workspace_recent_remove`, `open_external_url`;
- runtime lifecycle: `runtime_snapshot`, `runtime_start`, `runtime_stop`, `runtime_restart`;
- sessions: `session_new`, `session_list`, `session_load`, `session_resume`, `session_close`;
- turns and controls: `prompt_send`, `prompt_cancel`, `session_set_mode`, `session_set_model`, `session_set_config`;
- interactions: `permission_respond`, `elicitation_respond`.

Each command accepts one reviewed request DTO when input is required and returns one operation-specific DTO. `setup_status` reports a fixed executable state and discovery source without exposing the executable path. `workspace_pick` owns the Windows native folder dialog in Rust and returns only a canonical validated directory. Workspace validation rejects missing, non-directory, oversized, and control-bearing values before use. The versioned recent-workspace store is capped at eight entries, preserves unavailable entries for explicit removal, and contains no session or credential data. A preference read or write failure becomes a bounded warning without preventing an otherwise valid workspace selection. Runtime and preference errors become bounded application errors with a fixed code, redacted diagnostic, and recoverability flag.

The TypeScript `createApplicationBridge` module mirrors this interface without depending on Tauri internals. The application composition root supplies the official Tauri invoke/listen adapter; tests use an in-memory adapter through the same interface. The setup controller subscribes before it inspects or starts the runtime, then feeds every envelope through the shared deterministic reducer before projecting setup state. Connecting and authenticating transitions cannot be lost, duplicated, or regressed by stale delivery during launch.

The session controller binds list, new, load, resume, and close to the currently selected workspace. Changing workspace clears the selected session before the next list is requested, so a session from one folder cannot be opened against another. `GrokRuntime` also remembers session-to-workspace identity from create and list, filters listed sessions to the requested workspace, and rejects load or resume when that identity does not match. Session files are never read or written by the GUI.

The conversation controller sends prompts and control changes through the same reviewed commands. It reduces the shared event stream into a timeline of user, assistant, thought, and tool cards, then projects composer and control availability from advertised capabilities. Model, reasoning, mode, command, and config lists come from the current session and runtime catalog. They are never hard-coded. Markdown is parsed into React nodes without HTML, and only credential-free `http`/`https` URLs may be opened through `open_external_url`.

## Events and ordering

All events use the single `grok-application-event` channel:

```text
ApplicationEventEnvelope {
  generation,
  sequence,
  event
}
```

`generation` changes when a stopped runtime starts or the runtime restarts. `sequence` starts at one for each generation. Lifecycle responses wait until the corresponding runtime state has crossed the event seam, so a returned snapshot cannot advertise a generation or state that the event clock has not reached. The frontend reducer buffers a bounded sequence gap, applies contiguous events in order, ignores duplicates and older generations, and requests resynchronization rather than retaining an unbounded gap. A newer generation atomically clears session state. A closed session rejects late chunks and state changes; `GrokRuntime` emits `SessionActivated` through its ordered event stream before dispatching a requested load or resume, so previously queued events drain before the only same-generation reopening path.

Stream chunks, display text, choices, plans, commands, configuration options, model metadata, and session pages are bounded before serialization, and every complete event has a final serialized-size ceiling. Inbound identifiers, workspaces, cursors, prompts, configuration values, and elicitation content are rejected before runtime dispatch when they exceed the application boundary. Truncated stream and collection DTOs carry an explicit flag. Actionable permission paths and commands cross only when their exact values pass the boundary.

Permission requests carry the exact advertised decision set rather than a derived persistence flag. Successful permission and elicitation responses emit `InteractionResolved`, which removes the pending card from whichever session or runtime scope owns it. Automatic turn cancellation and runtime shutdown emit scoped `InteractionsCleared` events; cancelling, closed, failed, completed, and disconnected states also clear cards defensively so expired requests never remain actionable-looking.

Known Grok extensions cross the seam only as typed invalidation areas and an optional normalized session identifier. The UI can refresh the corresponding snapshot through a reviewed command. Unknown or invalid extensions collapse to a fixed classification; their raw method, session value, and payload are discarded.

## Frontend state seam

`reduceApplicationEvent(state, envelope)` is the deterministic state seam. `createApplicationStore` adds subscription and dispatch without changing reducer behavior. State is keyed by normalized session identifier so one session's activity cannot mutate another session. Messages, thoughts, tools, permissions, elicitations, plans, usage, commands, modes, configuration, and invalidation hints remain structured rather than becoming terminal-style text.

The reviewed wire inventory lives in `fixtures/application-contract-manifest.json`. Rust serialization fixtures and TypeScript contract constants both verify its command list, event tags and fields, request/response and nested DTO fields, enum wire values, and event channel. TypeScript field/value and tagged-union declarations are compile-time exhaustive over their corresponding types. Rust contract changes must update the reviewed serialization inventory and both parity gates together.

Rust tests cover every application event variant's JSON round trip, UTF-8 and collection bounds, event clock generations, workspace validation and recent persistence, request validation, advertised authentication selection, permission safety, extension collapse, and shared-manifest parity. Frontend tests cover command routing, event subscription, setup and workspace empty/error states, session list/new/load/resume/close, workspace isolation, loading/empty/selected/closed/stale/failed session states, every structured reducer state, resolved interactions, out-of-order delivery, duplicates, stale session events, explicit reactivation, restart transitions, and store publication. Fake-process tests cover session lifecycle success, close recovery, and workspace-mismatch plus list/load/resume failure transitions.
